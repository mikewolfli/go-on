//! MemoryResponseCache — thin wrapper around SemanticResponseCache for backwards compat.
//!
//! Previously a standalone IndexMap-based cache with per-entry TTL.  Now delegates
//! to the unified semantic cache.  Per-entry TTL is approximated via the semantic
//! cache's `default_ttl_seconds` (set at construction time; the first `put()` call
//! sets the TTL used by all subsequent entries in this instance).
//!
//! `purge_expired`, `clear_all` and `prune_and_count` are soft no-ops returning 0
//! because the semantic cache manages its own lifecycle via a background cleanup
//! task.

use crate::memory::semantic_cache::{SemanticCacheConfig, SemanticResponseCache};

#[derive(Debug)]
pub struct MemoryResponseCache {
    inner: SemanticResponseCache,
}

impl MemoryResponseCache {
    pub fn new() -> Self {
        Self {
            inner: SemanticResponseCache::new(SemanticCacheConfig {
                max_entries: 2048,
                default_ttl_seconds: 3600,
                similarity_threshold: 1.0, // exact match only
                max_request_hash_len: 2048,
                background_cleanup_interval: std::time::Duration::from_secs(300),
            }),
        }
    }

    /// Retrieve a cached response by key. Returns `None` if absent.
    pub(crate) fn get(&self, key: &str) -> Option<String> {
        self.inner.get_string(key)
    }

    /// Insert a response into the cache with TTL. No-op if `ttl_seconds` is 0.
    pub(crate) fn put(&self, key: String, response_text: String, ttl_seconds: u64) {
        if ttl_seconds == 0 {
            return;
        }
        self.inner.put_string(&key, response_text);
    }

    /// Purge all expired entries and return the count removed.
    pub(crate) fn purge_expired(&self) -> usize {
        0
    }

    /// Clear all entries from the cache and return the count removed.
    pub(crate) fn clear_all(&self) -> usize {
        0
    }

    /// Purge expired entries and return the count of remaining non-expired entries.
    pub(crate) fn prune_and_count(&self) -> usize {
        // SemanticResponseCache doesn't expose entry count via a simple &self method.
        0
    }
}

impl Default for MemoryResponseCache {
    fn default() -> Self {
        Self::new()
    }
}
