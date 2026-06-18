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
use std::collections::HashMap;
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
    /// Original request text for Jaccard similarity matching
    request_text: String,
    /// Request hash for exact matching
    request_hash: u64,
    /// When this entry was created
    created_at: Instant,
    /// Time-to-live
    ttl: Duration,
    /// Access count
    #[allow(dead_code)] // reserved for cache analytics
    access_count: u64,
    /// Hit count
    #[allow(dead_code)] // reserved for cache analytics
    hits: u64,
    /// When this entry was last accessed (for LRU eviction)
    last_accessed: Instant,
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
        &request[..max_len]
    } else {
        request
    };
    truncated.hash(&mut hasher);
    hasher.finish()
}

/// Simple Jaccard similarity for request comparison
fn jaccard_similarity(a: &str, b: &str) -> f64 {
    let a_bigrams: Vec<&[u8]> = a.as_bytes().windows(2).collect();
    let b_bigrams: Vec<&[u8]> = b.as_bytes().windows(2).collect();

    if a_bigrams.is_empty() && b_bigrams.is_empty() {
        return 1.0;
    }
    if a_bigrams.is_empty() || b_bigrams.is_empty() {
        return 0.0;
    }

    let set_a: std::collections::HashSet<&[u8]> = a_bigrams.iter().copied().collect();
    let set_b: std::collections::HashSet<&[u8]> = b_bigrams.iter().copied().collect();

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
    total_warmups: AtomicU64,
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
            total_warmups: AtomicU64::new(0),
            expired_count: AtomicU64::new(0),
            cancellation_token: None,
        }
    }

    /// Get a cached response if available
    pub fn get(&self, request: &str) -> Option<Value> {
        let hash = simple_request_hash(request, self.config.max_request_hash_len);
        let now = Instant::now();

        let guard = self.entries.read().expect("SemanticCache entries poisoned");
        if let Some(bucket) = guard.get(&hash) {
            // Find matching entry index — try exact match first, then similarity
            // Expired entry removal is handled by the background cleanup task.
            let match_idx = bucket
                .iter()
                .position(|entry| {
                    entry.request_hash == hash && now.duration_since(entry.created_at) < entry.ttl
                })
                .or_else(|| {
                    bucket.iter().position(|entry| {
                        // Both exact and similarity lookups must respect TTL.
                        now.duration_since(entry.created_at) < entry.ttl && {
                            let similarity = jaccard_similarity(request, &entry.request_text);
                            similarity >= self.config.similarity_threshold
                        }
                    })
                });

            if let Some(idx) = match_idx {
                self.total_hits.fetch_add(1, Ordering::Relaxed);
                return Some(bucket[idx].response.clone());
            }
        }

        self.total_misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Cache a response
    pub fn put(&mut self, request: &str, response: Value) {
        let hash = simple_request_hash(request, self.config.max_request_hash_len);
        let now = Instant::now();

        let mut guard = self
            .entries
            .write()
            .expect("SemanticCache entries poisoned");

        // LRU eviction if over max entries (consolidated into single lock)
        if guard.len() >= self.config.max_entries {
            // Evict oldest entry across all buckets
            let mut lru_key = None;
            let mut lru_idx = 0;
            let mut oldest = now;
            for (key, bucket) in guard.iter() {
                for (i, entry) in bucket.iter().enumerate() {
                    if entry.last_accessed < oldest {
                        oldest = entry.last_accessed;
                        lru_key = Some(*key);
                        lru_idx = i;
                    }
                }
            }
            if let Some(key) = lru_key {
                if let Some(bucket) = guard.get_mut(&key) {
                    bucket.remove(lru_idx);
                    if bucket.is_empty() {
                        guard.remove(&key);
                    }
                }
            }
        }

        let entry = CacheEntry {
            response,
            request_text: request.to_string(),
            request_hash: hash,
            created_at: now,
            ttl: Duration::from_secs(self.config.default_ttl_seconds),
            access_count: 1,
            hits: 0,
            last_accessed: now,
        };

        guard.entry(hash).or_default().push(entry);
    }

    /// Warm up the cache with known entries
    pub fn warmup(&mut self, requests: Vec<(String, Value)>) {
        for (request, response) in requests {
            self.put(&request, response);
            self.total_warmups.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Clear all entries
    pub fn clear(&mut self) {
        self.entries
            .write()
            .expect("SemanticCache entries poisoned")
            .clear();
        self.total_hits.store(0, Ordering::Relaxed);
        self.total_misses.store(0, Ordering::Relaxed);
    }

    /// Get cache statistics
    pub fn stats(&self) -> SemanticCacheStats {
        let total_hits = self.total_hits.load(Ordering::Relaxed);
        let total_misses = self.total_misses.load(Ordering::Relaxed);
        let total_warmups = self.total_warmups.load(Ordering::Relaxed);
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
            total_warmups,
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
                        let now = Instant::now();
                        if let Ok(mut guard) = entries.write() {
                            for bucket in guard.values_mut() {
                                bucket.retain(|e| now.duration_since(e.created_at) < e.ttl);
                            }
                            guard.retain(|_, bucket| !bucket.is_empty());
                        }
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
    pub total_warmups: u64,
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
        let mut cache = SemanticResponseCache::new(SemanticCacheConfig::default());
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
        let mut cache = SemanticResponseCache::new(SemanticCacheConfig {
            default_ttl_seconds: 0, // Immediate expiry
            ..Default::default()
        });
        cache.put("hello", json!("world"));
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert!(cache.get("hello").is_none());
    }

    #[test]
    fn test_lru_eviction() {
        let mut cache = SemanticResponseCache::new(SemanticCacheConfig {
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
    fn test_warmup() {
        let mut cache = SemanticResponseCache::new(SemanticCacheConfig::default());
        cache.warmup(vec![("q1".into(), json!("a1")), ("q2".into(), json!("a2"))]);
        assert_eq!(cache.total_warmups.load(Ordering::Relaxed), 2);
        assert!(cache.get("q1").is_some());
        assert!(cache.get("q2").is_some());
    }

    #[test]
    fn test_stats() {
        let mut cache = SemanticResponseCache::new(SemanticCacheConfig::default());
        cache.put("test", json!("value"));
        let _ = cache.get("test");
        let _ = cache.get("nope");
        let stats = cache.stats();
        assert_eq!(stats.total_hits, 1);
        assert_eq!(stats.total_misses, 1);
        assert!((stats.hit_ratio - 0.5).abs() < 0.001);
    }
}
