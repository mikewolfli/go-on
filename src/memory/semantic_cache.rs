//! Semantic response cache — cache LLM responses by request hash + similarity
//!
//! Provides:
//! - TTL-based entry expiration
//! - LRU eviction when max entries exceeded
//! - Semantic similarity matching for near-duplicate requests using the
//!   canonical minhash embedding (`embedding_provider::local_hash_embed`) and
//!   shared cosine similarity — the same embedding the vector store, token
//!   cache L2, and memory summarization use.
//! - Cache warm-up on startup
//!
//! F-GAP-49: Module now wired into production chat pipeline.
//!
//! # Responsibility boundary (do not merge with the vector store)
//!
//! This cache answers *near-duplicate questions* by short-circuiting the LLM
//! and replaying the stored answer (TTL 1h, LRU, in-memory). The vector store
//! (`memory/vector.rs`) is *persistent RAG memory* that injects retrieved
//! context into the prompt and still calls the LLM. The two are deliberately
//! different layers (answer-cache vs context-memory); merging either direction
//! is a behavior regression, not a performance question — see
//! docs/log/log-20260811-6.md (debt #1 verdict: keep + document).

use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

// ── Semantic cache (canonical minhash embedding + cosine similarity) ─────────

/// Cache entry with metadata
#[derive(Debug, Clone)]
struct CacheEntry {
    /// Response content
    response: Value,
    /// Full request text — exact matching compares the full request, not just
    /// the (possibly truncated) bucket hash, so multi-turn conversations whose
    /// first 1024 chars are identical never hit an earlier turn's entry.
    request: String,
    /// When this entry was created
    created_at: Instant,
    /// Time-to-live
    ttl: Duration,
    /// When this entry was last accessed (for LRU eviction)
    last_accessed: Instant,
    /// Precomputed canonical embedding (avoids re-embedding on every get).
    embedding: Option<Vec<f32>>,
}

/// Semantic response cache configuration
#[derive(Debug, Clone)]
pub struct SemanticCacheConfig {
    /// Maximum number of entries
    pub max_entries: usize,
    /// Default TTL in seconds
    pub default_ttl_seconds: u64,
    /// Embedding similarity threshold (0.0-1.0)
    pub similarity_threshold: f64,
    /// Maximum request length for hash
    pub max_request_hash_len: usize,
    /// Background cleanup interval in seconds
    pub background_cleanup_interval: Duration,
}

impl Default for SemanticCacheConfig {
    fn default() -> Self {
        // Design constants (not config-driven, matching the token-cache L2
        // sibling): 1000 entries / 1h TTL / 0.95 cosine threshold / 1024-char
        // hash / 5-min cleanup. 0.95 deliberately sits above the vector-store
        // min_similarity (0.70–0.85) because this cache replays answers on
        // near-duplicate questions and must not return stale matches for
        // merely-similar ones. See the module doc for the responsibility
        // boundary vs the persistent vector store.
        Self {
            max_entries: 1000,
            default_ttl_seconds: 3600, // 1 hour
            similarity_threshold: 0.95,
            max_request_hash_len: 1024,
            background_cleanup_interval: Duration::from_secs(300),
        }
    }
}

/// Simple hash based on request content
fn simple_request_hash(request: &str, max_len: usize) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let truncated = if request.len() > max_len {
        // Use floor_char_boundary to avoid cutting in the middle of a UTF-8 char
        &request[..request.floor_char_boundary(max_len)]
    } else {
        request
    };
    truncated.hash(&mut hasher);
    hasher.finish()
}

/// Embed a request with the canonical minhash embedding (single source shared
/// with the vector store, token cache L2, and memory summarization).
fn request_embedding(request: &str) -> Vec<f32> {
    crate::memory::embedding_provider::local_hash_embed(request, 128)
}

/// Semantic response cache
#[derive(Debug)]
pub struct SemanticResponseCache {
    entries: Arc<RwLock<HashMap<u64, Vec<CacheEntry>>>>,
    config: SemanticCacheConfig,
    total_hits: AtomicU64,
    total_misses: AtomicU64,
    // Arc so the background cleanup task shares the counter with the live
    // cache and keeps `expired_count` accurate (purge logic is shared with
    // `purge_expired` via `purge_expired_entries`).
    expired_count: Arc<AtomicU64>,
}

impl SemanticResponseCache {
    pub fn new(config: SemanticCacheConfig) -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            config,
            total_hits: AtomicU64::new(0),
            total_misses: AtomicU64::new(0),
            expired_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Get a cached response if available
    pub fn get(&self, request: &str) -> Option<Value> {
        let hash = simple_request_hash(request, self.config.max_request_hash_len);
        let now = Instant::now();

        // Find the matching entry index under a read lock; the lock is dropped
        // before the best-effort LRU touch below. The query embedding is
        // computed lazily — only when a similarity scan is actually needed.
        let match_idx = {
            let guard = self.entries.read().expect("SemanticCache entries poisoned");
            guard.get(&hash).and_then(|bucket| {
                // Find matching entry index — try exact match first, then
                // similarity. Exact match requires the full request text to
                // be equal (the bucket hash alone is not enough: it is
                // truncated to max_request_hash_len, so distinct requests
                // sharing a long prefix would collide). Expired entry removal
                // is handled by the background cleanup task.
                bucket
                    .iter()
                    .position(|entry| {
                        entry.request == request && now.duration_since(entry.created_at) < entry.ttl
                    })
                    .or_else(|| {
                        // Similarity lookup must respect TTL. Uses the same
                        // canonical embedding + cosine similarity as every
                        // other similarity consumer.
                        let query_vec = request_embedding(request);
                        bucket.iter().position(|entry| {
                            now.duration_since(entry.created_at) < entry.ttl
                                && entry
                                    .embedding
                                    .as_ref()
                                    .map(|stored| {
                                        crate::shared::math::cosine_similarity_f32(
                                            &query_vec, stored,
                                        )
                                    })
                                    .map(|s| s >= self.config.similarity_threshold as f32)
                                    .unwrap_or(false)
                        })
                    })
            })
        };

        match match_idx {
            Some(idx) => {
                self.total_hits.fetch_add(1, Ordering::Relaxed);
                // Best-effort LRU touch + read under a single write lock so the
                // returned entry matches the one found above (a read-after-
                // read could otherwise read a shifted bucket index if a
                // concurrent put evicted an earlier entry in the same bucket).
                if let Ok(mut guard) = self.entries.try_write() {
                    let hit = guard.get_mut(&hash).and_then(|bucket| {
                        bucket.get_mut(idx).map(|entry| {
                            entry.last_accessed = Instant::now();
                            entry.response.clone()
                        })
                    });
                    hit
                } else {
                    // Write lock contended — fall back to a fresh read that
                    // re-verifies by request text (not index).
                    let guard = self.entries.read().expect("SemanticCache entries poisoned");
                    guard.get(&hash).and_then(|bucket| {
                        bucket
                            .iter()
                            .find(|entry| {
                                entry.request == request
                                    && now.duration_since(entry.created_at) < entry.ttl
                            })
                            .map(|e| e.response.clone())
                    })
                }
            }
            None => {
                self.total_misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    /// Cache a response
    pub fn put(&self, request: &str, response: Value) {
        self.put_inner(request, response, self.config.default_ttl_seconds);
    }

    /// Cache a response with an explicit per-entry TTL (seconds).
    ///
    /// Used when a phase-level `cache_ttl_seconds` override is configured.
    /// `put` goes through the same `put_inner` path with the global default
    /// TTL; this variant supplies an explicit TTL instead.
    pub fn put_with_ttl(&self, request: &str, response: Value, ttl_seconds: u64) {
        self.put_inner(request, response, ttl_seconds.max(1));
    }

    /// Shared insert path with an explicit per-entry TTL.
    fn put_inner(&self, request: &str, response: Value, ttl_seconds: u64) {
        let hash = simple_request_hash(request, self.config.max_request_hash_len);
        let now = Instant::now();

        let mut guard = self
            .entries
            .write()
            .expect("SemanticCache entries poisoned");

        // LRU eviction if over max entries — evict the entry with oldest
        // last_accessed from the bucket with the most entries (avoids O(n)
        // scan of ALL entries).
        if guard.len() >= self.config.max_entries {
            if let Some(largest_bucket_key) =
                guard.iter().max_by_key(|(_, b)| b.len()).map(|(k, _)| *k)
            {
                if let Some(bucket) = guard.get_mut(&largest_bucket_key) {
                    // Remove oldest entry in the largest bucket (constant per-bucket time)
                    if let Some(oldest_idx) = bucket
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, e)| e.last_accessed)
                        .map(|(i, _)| i)
                    {
                        bucket.remove(oldest_idx);
                        if bucket.is_empty() {
                            guard.remove(&largest_bucket_key);
                        }
                    }
                }
            }
        }

        let entry = CacheEntry {
            response,
            request: request.to_string(),
            created_at: now,
            ttl: Duration::from_secs(ttl_seconds),
            last_accessed: now,
            embedding: Some(request_embedding(request)),
        };

        guard.entry(hash).or_default().push(entry);
    }

    /// Clear all entries
    pub fn clear(&self) {
        self.entries
            .write()
            .expect("SemanticCache entries poisoned")
            .clear();
        self.total_hits.store(0, Ordering::Relaxed);
        self.total_misses.store(0, Ordering::Relaxed);
    }

    /// Remove all expired entries and return the number removed.
    pub fn purge_expired(&self) -> usize {
        purge_expired_entries(&self.entries, &self.expired_count)
    }

    /// Total number of live (non-expired) entries.
    pub fn len(&self) -> usize {
        let now = Instant::now();
        self.entries
            .read()
            .expect("SemanticCache entries poisoned")
            .values()
            .map(|bucket| {
                bucket
                    .iter()
                    .filter(|e| now.duration_since(e.created_at) < e.ttl)
                    .count()
            })
            .sum()
    }

    /// Return `true` if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get cache statistics
    pub fn stats(&self) -> SemanticCacheStats {
        let total_hits = self.total_hits.load(Ordering::Relaxed);
        let total_misses = self.total_misses.load(Ordering::Relaxed);
        let expired_count = self.expired_count.load(Ordering::Relaxed);
        let total = total_hits + total_misses;
        let total_entries: u64 = self
            .entries
            .read()
            .expect("SemanticCache entries poisoned")
            .values()
            .map(|v| v.len() as u64)
            .sum();
        SemanticCacheStats {
            entries: total_entries,
            total_hits,
            total_misses,
            hit_ratio: if total == 0 {
                0.0
            } else {
                total_hits as f64 / total as f64
            },
            expired_count,
        }
    }

    /// Start background cleanup task that periodically removes expired entries.
    pub fn start_background_cleanup(&mut self) -> CancellationToken {
        let token = CancellationToken::new();
        let token_clone = token.clone();
        let interval = self.config.background_cleanup_interval;

        // Clone the Arcs so the background task shares the same entries map and
        // counter (the task runs `purge_expired` on the live cache).
        let entries = self.entries.clone();
        let expired_count = self.expired_count.clone();

        tokio::spawn(async move {
            let mut interval_timer = tokio::time::interval(interval);
            loop {
                tokio::select! {
                    _ = interval_timer.tick() => {
                        let entries = entries.clone();
                        let expired_count = expired_count.clone();
                        // Use spawn_blocking to avoid blocking the async runtime
                        // with the std::sync::RwLock write lock. Reuses the
                        // exact same purge logic as `purge_expired()` (single
                        // implementation) so the background cleanup also keeps
                        // `expired_count` accurate — the old inline retain
                        // copy silently dropped the expired accounting.
                        tokio::task::spawn_blocking(move || {
                            purge_expired_entries(&entries, &expired_count);
                        })
                        .await
                        .ok();
                    }
                    _ = token_clone.cancelled() => {
                        break;
                    }
                }
            }
        });

        // The caller owns the returned token; the task runs until cancelled
        // or the process exits (no stop method is exposed).
        token
    }
}

/// Shared purge implementation used by both `purge_expired()` and the
/// background cleanup task: removes expired entries from `entries` and accounts
/// for them in `expired_count`. Returns the number removed.
fn purge_expired_entries(
    entries: &RwLock<HashMap<u64, Vec<CacheEntry>>>,
    expired_count: &AtomicU64,
) -> usize {
    let now = Instant::now();
    let mut guard = entries.write().expect("SemanticCache entries poisoned");
    let mut removed = 0usize;
    guard.retain(|_, bucket| {
        let before = bucket.len();
        bucket.retain(|e| now.duration_since(e.created_at) < e.ttl);
        removed += before - bucket.len();
        !bucket.is_empty()
    });
    expired_count.fetch_add(removed as u64, Ordering::Relaxed);
    removed
}

#[derive(Debug, Clone, Serialize)]
pub struct SemanticCacheStats {
    pub entries: u64,
    pub total_hits: u64,
    pub total_misses: u64,
    pub hit_ratio: f64,
    pub expired_count: u64,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── Existing bigram cache tests ──────────────────────────────────────────

    #[test]
    fn test_exact_match() {
        let cache = SemanticResponseCache::new(SemanticCacheConfig::default());
        cache.put("hello world", json!({"response": "hi"}));
        let result = cache.get("hello world");
        assert!(result.is_some());
        assert_eq!(
            result.expect("exact match should return Some")["response"],
            "hi"
        );
    }

    #[test]
    fn test_cache_miss() {
        let cache = SemanticResponseCache::new(SemanticCacheConfig::default());
        assert!(cache.get("never cached").is_none());
    }

    #[test]
    fn test_truncated_hash_collision_does_not_exact_match() {
        // Regression (P1): the bucket hash truncates to max_request_hash_len,
        // so two requests sharing a long prefix land in the same bucket.
        // Exact matching must compare the full request text, not just the
        // bucket hash — otherwise a later turn of a conversation would receive
        // an earlier turn's cached answer. The two requests below share their
        // entire first 1024 chars but ask different questions, so the exact
        // branch must NOT return turn 1's answer.
        let cache = SemanticResponseCache::new(SemanticCacheConfig {
            max_request_hash_len: 1024,
            // Cosine similarity is bounded by 1.0; a threshold of 2.0 makes
            // the similarity branch unreachable so the test exercises ONLY the
            // exact-match branch.
            similarity_threshold: 2.0,
            ..Default::default()
        });
        let mut turn1 = "system: you are a coding assistant\nuser: explain closures".to_string();
        while turn1.len() < 2048 {
            turn1.push_str("\nuser: more context padding");
        }
        let turn2 = format!(
            "{}\nassistant: here is a long explanation of closures.\nuser: now explain traits instead",
            turn1
        );

        cache.put(&turn1, json!("turn-1 answer"));
        // Exact match on the identical request still hits.
        assert_eq!(cache.get(&turn1).unwrap(), "turn-1 answer");
        // A later turn with the same prefix must NOT hit turn 1's entry.
        assert!(
            cache.get(&turn2).is_none(),
            "prefix collision must not exact-match"
        );
    }

    #[test]
    fn test_ttl_expiry() {
        let cache = SemanticResponseCache::new(SemanticCacheConfig {
            default_ttl_seconds: 0, // Immediate expiry
            ..Default::default()
        });
        cache.put("hello", json!("world"));
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert!(cache.get("hello").is_none());
    }

    #[test]
    fn test_lru_eviction() {
        let cache = SemanticResponseCache::new(SemanticCacheConfig {
            max_entries: 2,
            ..Default::default()
        });
        cache.put("a", json!("1"));
        cache.put("b", json!("2"));
        cache.put("c", json!("3"));
        assert_eq!(
            cache
                .entries
                .read()
                .expect("lock should not be poisoned")
                .len(),
            2
        );
    }

    #[test]
    fn test_stats() {
        let cache = SemanticResponseCache::new(SemanticCacheConfig::default());
        cache.put("test", json!("value"));
        let _ = cache.get("test");
        let _ = cache.get("nope");
        let stats = cache.stats();
        assert_eq!(stats.total_hits, 1);
        assert_eq!(stats.total_misses, 1);
        assert!((stats.hit_ratio - 0.5).abs() < 0.001);
    }
}
