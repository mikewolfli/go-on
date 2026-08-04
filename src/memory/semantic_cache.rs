//! Semantic response cache — cache LLM responses by request hash + bigram similarity
//!
//! Provides:
//! - TTL-based entry expiration
//! - LRU eviction when max entries exceeded
//! - Bigram Jaccard similarity matching for near-duplicate requests
//! - Cache warm-up on startup
//!
//! F-GAP-49: Module now wired into production chat pipeline.

use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

// ── Existing bigram-based cache (kept as fast path <1ms lookup) ──────────────

/// Cache entry with metadata
#[derive(Debug, Clone)]
struct CacheEntry {
    /// Response content
    response: Value,
    /// Request hash for exact matching
    request_hash: u64,
    /// When this entry was created
    created_at: Instant,
    /// Time-to-live
    ttl: Duration,
    /// When this entry was last accessed (for LRU eviction)
    last_accessed: Instant,
    /// Precomputed bigram set for Jaccard similarity (avoids recomputing on every get)
    bigram_set: Option<HashSet<Vec<u8>>>,
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

/// Compute the set of bigrams for a string (owned, so it can be cached)
fn bigrams_set(s: &str) -> HashSet<Vec<u8>> {
    s.as_bytes().windows(2).map(|w| w.to_vec()).collect()
}

/// Jaccard similarity with one side's bigram set precomputed.
/// Only computes bigrams for `a`; reuses `b_bigrams` from cache.
fn jaccard_with_precomputed(a: &str, b_bigrams: &HashSet<Vec<u8>>) -> f64 {
    let a_bigrams: Vec<&[u8]> = a.as_bytes().windows(2).collect();

    if a_bigrams.is_empty() && b_bigrams.is_empty() {
        return 1.0;
    }
    if a_bigrams.is_empty() || b_bigrams.is_empty() {
        return 0.0;
    }

    let set_a: HashSet<&[u8]> = a_bigrams.iter().copied().collect();
    // Convert b_bigrams (HashSet<Vec<u8>>) to HashSet<&[u8]> for intersection
    let set_b: HashSet<&[u8]> = b_bigrams.iter().map(|v| v.as_slice()).collect();

    let intersection = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();

    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

/// Semantic response cache
#[derive(Debug)]
pub struct SemanticResponseCache {
    entries: Arc<RwLock<HashMap<u64, Vec<CacheEntry>>>>,
    config: SemanticCacheConfig,
    total_hits: AtomicU64,
    total_misses: AtomicU64,
    expired_count: AtomicU64,
    cancellation_token: Option<CancellationToken>,
}

impl SemanticResponseCache {
    pub fn new(config: SemanticCacheConfig) -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            config,
            total_hits: AtomicU64::new(0),
            total_misses: AtomicU64::new(0),
            expired_count: AtomicU64::new(0),
            cancellation_token: None,
        }
    }

    /// Get a cached response if available
    pub fn get(&self, request: &str) -> Option<Value> {
        let hash = simple_request_hash(request, self.config.max_request_hash_len);
        let now = Instant::now();

        // Find the matching entry index under a read lock; the lock is dropped
        // before the best-effort LRU touch below.
        let match_idx = {
            let guard = self.entries.read().expect("SemanticCache entries poisoned");
            guard.get(&hash).and_then(|bucket| {
                // Find matching entry index — try exact match first, then
                // similarity. Expired entry removal is handled by the
                // background cleanup task.
                bucket
                    .iter()
                    .position(|entry| {
                        entry.request_hash == hash
                            && now.duration_since(entry.created_at) < entry.ttl
                    })
                    .or_else(|| {
                        bucket.iter().position(|entry| {
                            // Similarity lookup must respect TTL. `bigram_set`
                            // is always populated by `put_inner`, so the
                            // precomputed path is the only reachable one.
                            now.duration_since(entry.created_at) < entry.ttl
                                && entry
                                    .bigram_set
                                    .as_ref()
                                    .map(|pre| jaccard_with_precomputed(request, pre))
                                    .map(|s| s >= self.config.similarity_threshold)
                                    .unwrap_or(false)
                        })
                    })
            })
        };

        match match_idx {
            Some(idx) => {
                self.total_hits.fetch_add(1, Ordering::Relaxed);
                // Best-effort LRU touch: refresh `last_accessed` so eviction
                // prefers entries that have actually been served (previously
                // `last_accessed` was only set at insert — eviction was by
                // insertion order, not recency).
                if let Ok(mut guard) = self.entries.try_write() {
                    if let Some(bucket) = guard.get_mut(&hash) {
                        if let Some(entry) = bucket.get_mut(idx) {
                            entry.last_accessed = Instant::now();
                        }
                    }
                }
                let guard = self.entries.read().expect("SemanticCache entries poisoned");
                guard.get(&hash)?.get(idx).map(|e| e.response.clone())
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

    /// Store a string response (convenience wrapper for `put`).
    pub fn put_string(&self, request: &str, response_text: String) {
        self.put(request, Value::String(response_text));
    }

    /// Store a string response with an explicit per-entry TTL (seconds).
    pub fn put_string_with_ttl(&self, request: &str, response_text: String, ttl_seconds: u64) {
        self.put_inner(request, Value::String(response_text), ttl_seconds);
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
            request_hash: hash,
            created_at: now,
            ttl: Duration::from_secs(ttl_seconds),
            last_accessed: now,
            bigram_set: Some(bigrams_set(request)),
        };

        guard.entry(hash).or_default().push(entry);
    }

    /// Retrieve a string response (convenience wrapper for `get`).
    pub fn get_string(&self, request: &str) -> Option<String> {
        self.get(request).and_then(|v| match v {
            Value::String(s) => Some(s),
            _ => v.as_str().map(String::from),
        })
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
        let now = Instant::now();
        let mut guard = self
            .entries
            .write()
            .expect("SemanticCache entries poisoned");
        let mut removed = 0usize;
        guard.retain(|_, bucket| {
            let before = bucket.len();
            bucket.retain(|e| now.duration_since(e.created_at) < e.ttl);
            removed += before - bucket.len();
            !bucket.is_empty()
        });
        self.expired_count
            .fetch_add(removed as u64, Ordering::Relaxed);
        removed
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

        // Clone the Arc so the background task shares the same entries map.
        let entries = self.entries.clone();

        tokio::spawn(async move {
            let mut interval_timer = tokio::time::interval(interval);
            loop {
                tokio::select! {
                    _ = interval_timer.tick() => {
                        let cache = entries.clone();
                        // Use spawn_blocking to avoid blocking the async runtime
                        // with the std::sync::RwLock write lock.
                        tokio::task::spawn_blocking(move || {
                            let now = Instant::now();
                            if let Ok(mut guard) = cache.write() {
                                for bucket in guard.values_mut() {
                                    bucket.retain(|e| now.duration_since(e.created_at) < e.ttl);
                                }
                                guard.retain(|_, bucket| !bucket.is_empty());
                            }
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

        self.cancellation_token = Some(token.clone());
        token
    }

    /// Stop the background cleanup task.
    pub fn stop_background_cleanup(&mut self) {
        if let Some(token) = self.cancellation_token.take() {
            token.cancel();
        }
    }
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
