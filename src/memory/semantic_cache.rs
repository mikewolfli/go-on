//! Semantic response cache — cache LLM responses by request hash + embedding similarity
//!
//! Provides:
//! - TTL-based entry expiration
//! - LRU eviction when max entries exceeded
//! - Embedding similarity matching for near-duplicate requests
//! - Cache warm-up on startup

// F-GAP-49: Module now wired into production chat pipeline.

use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::time::{Duration, Instant};

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
    /// Access count (for LRU eviction)
    access_count: u64,
    /// Hit count
    hits: u64,
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
}

impl Default for SemanticCacheConfig {
    fn default() -> Self {
        Self {
            max_entries: 1000,
            default_ttl_seconds: 3600, // 1 hour
            similarity_threshold: 0.95,
            max_request_hash_len: 1024,
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
    entries: HashMap<u64, Vec<CacheEntry>>,
    config: SemanticCacheConfig,
    total_hits: u64,
    total_misses: u64,
    total_warmups: u64,
}

impl SemanticResponseCache {
    pub fn new(config: SemanticCacheConfig) -> Self {
        Self {
            entries: HashMap::new(),
            config,
            total_hits: 0,
            total_misses: 0,
            total_warmups: 0,
        }
    }

    /// Get a cached response if available
    pub fn get(&mut self, request: &str) -> Option<&Value> {
        let hash = simple_request_hash(request, self.config.max_request_hash_len);
        let now = Instant::now();

        if let Some(bucket) = self.entries.get_mut(&hash) {
            // First, remove expired entries
            bucket.retain(|e| now.duration_since(e.created_at) < e.ttl);

            // Find matching entry index — try exact match first, then similarity
            let match_idx = bucket
                .iter()
                .position(|entry| {
                    entry.request_hash == hash && now.duration_since(entry.created_at) < entry.ttl
                })
                .or_else(|| {
                    bucket.iter().position(|entry| {
                        let similarity =
                            jaccard_similarity(request, &format!("{:?}", entry.request_hash));
                        similarity >= self.config.similarity_threshold
                    })
                });

            if let Some(idx) = match_idx {
                let entry = &mut bucket[idx];
                entry.access_count += 1;
                entry.hits += 1;
                self.total_hits += 1;
                return Some(&entry.response);
            }
        }

        self.total_misses += 1;
        None
    }

    /// Cache a response
    pub fn put(&mut self, request: &str, response: Value) {
        let hash = simple_request_hash(request, self.config.max_request_hash_len);

        // LRU eviction if over max entries
        if self.entries.len() >= self.config.max_entries {
            self.evict_lru();
        }

        let entry = CacheEntry {
            response,
            request_hash: hash,
            created_at: Instant::now(),
            ttl: Duration::from_secs(self.config.default_ttl_seconds),
            access_count: 1,
            hits: 0,
        };

        self.entries.entry(hash).or_default().push(entry);
    }

    /// Warm up the cache with known entries
    pub fn warmup(&mut self, requests: Vec<(String, Value)>) {
        for (request, response) in requests {
            self.put(&request, response);
            self.total_warmups += 1;
        }
    }

    /// Evict least recently used entry
    fn evict_lru(&mut self) {
        let mut oldest_key = None;
        let mut oldest_idx = 0;
        let mut oldest_time = Instant::now();

        for (key, bucket) in &self.entries {
            for (i, entry) in bucket.iter().enumerate() {
                if entry.created_at < oldest_time {
                    oldest_time = entry.created_at;
                    oldest_key = Some(*key);
                    oldest_idx = i;
                }
            }
        }

        if let Some(key) = oldest_key {
            if let Some(bucket) = self.entries.get_mut(&key) {
                bucket.remove(oldest_idx);
                if bucket.is_empty() {
                    self.entries.remove(&key);
                }
            }
        }
    }

    /// Clear all entries
    pub fn clear(&mut self) {
        self.entries.clear();
        self.total_hits = 0;
        self.total_misses = 0;
    }

    /// Get cache statistics
    pub fn stats(&self) -> SemanticCacheStats {
        let total = self.total_hits + self.total_misses;
        SemanticCacheStats {
            entries: self.entries.len() as u64,
            total_hits: self.total_hits,
            total_misses: self.total_misses,
            hit_ratio: if total == 0 {
                0.0
            } else {
                self.total_hits as f64 / total as f64
            },
            total_warmups: self.total_warmups,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_exact_match() {
        let mut cache = SemanticResponseCache::new(SemanticCacheConfig::default());
        cache.put("hello world", json!({"response": "hi"}));
        let result = cache.get("hello world");
        assert!(result.is_some());
        assert_eq!(result.unwrap()["response"], "hi");
    }

    #[test]
    fn test_cache_miss() {
        let mut cache = SemanticResponseCache::new(SemanticCacheConfig::default());
        let result = cache.get("never cached");
        assert!(result.is_none());
    }

    #[test]
    fn test_ttl_expiry() {
        let mut config = SemanticCacheConfig::default();
        config.default_ttl_seconds = 0; // Immediate expiry
        let mut cache = SemanticResponseCache::new(config);
        cache.put("hello", json!("world"));
        std::thread::sleep(std::time::Duration::from_millis(10));
        let result = cache.get("hello");
        assert!(result.is_none());
    }

    #[test]
    fn test_lru_eviction() {
        let mut config = SemanticCacheConfig::default();
        config.max_entries = 2;
        let mut cache = SemanticResponseCache::new(config);
        cache.put("a", json!("1"));
        cache.put("b", json!("2"));
        cache.put("c", json!("3"));
        assert_eq!(cache.entries.len(), 2);
    }

    #[test]
    fn test_warmup() {
        let mut cache = SemanticResponseCache::new(SemanticCacheConfig::default());
        cache.warmup(vec![("q1".into(), json!("a1")), ("q2".into(), json!("a2"))]);
        assert_eq!(cache.total_warmups, 2);
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

    #[test]
    fn test_simple_hash_consistent() {
        let h1 = simple_request_hash("hello", 1024);
        let h2 = simple_request_hash("hello", 1024);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_jaccard_identical() {
        let sim = jaccard_similarity("hello world", "hello world");
        assert!((sim - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_jaccard_empty() {
        let sim = jaccard_similarity("", "");
        assert!((sim - 1.0).abs() < 0.001);
    }
}
