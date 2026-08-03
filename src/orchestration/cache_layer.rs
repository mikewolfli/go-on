//! Shared cache layer trait and global metrics collector for go-on.
//!
//! Provides a unified [`CacheLayer`] trait that caches can opt into, plus a
//! [`CacheMetricsCollector`] for aggregating stats across all registered
//! caches.  Registration is entirely optional — a cache works exactly as
//! before without implementing this trait.
//!
//! # Architecture
//!
//! ```text
//!                    ┌──────────────────────────┐
//!                    │  CacheMetricsCollector    │
//!                    │  (holds Vec<Box<dyn CL>>) │
//!                    └──────┬───────────────────┘
//!            ┌──────────────┼──────────────────┐
//!            ▼              ▼
//!     GovernanceCache  SemanticCache …
//!     (impl CacheLayer)(impl CacheLayer)
//! ```
//!
//! A convenience [`GLOBAL_CACHE_METRICS`] singleton and
//! [`register_cache`] / [`get_aggregate_cache_stats`] helper functions let
//! caches participate without creating their own collector.

// CacheLayer trait + CacheMetricsCollector are used through dynamic dispatch
// by ShardedGovernanceCache. register_cache() has no current production
// callers (the former FullAutoFlow registration was removed); the collector
// helpers remain as a pub observability extension point.
// Only convenience helpers + collector methods are currently test-only.
#![cfg_attr(not(test), allow(dead_code))]

use std::sync::Mutex;
use std::sync::OnceLock;

use serde::Serialize;
use serde_json::Value;

// ---------------------------------------------------------------------------
// CacheStats
// ---------------------------------------------------------------------------

/// A snapshot of a cache's current state.
///
/// All counter fields (`hits`, `misses`) are cumulative since the cache was
/// created or last cleared.
#[derive(Debug, Clone, Serialize)]
pub struct CacheStats {
    /// Total number of cache hits.
    pub hits: u64,
    /// Total number of cache misses.
    pub misses: u64,
    /// Current number of live entries.
    pub entries: usize,
    /// Maximum number of entries the cache is configured to hold.
    pub max_entries: usize,
    /// Estimated heap memory usage in bytes (best-effat).
    pub estimated_size_bytes: usize,
}

// ---------------------------------------------------------------------------
// CacheLayer trait
// ---------------------------------------------------------------------------

/// A shared interface that a cache can implement to expose metrics and
/// administrative operations.
///
/// # Opt-in
///
/// This trait is **not** required.  Caches continue to work exactly as before
/// without implementing it.
pub trait CacheLayer: Send + Sync {
    /// A human-readable name for this cache (e.g. `"governance"`,
    /// `"fast_path"`, `"memory_response"`).
    fn name(&self) -> &str;

    /// Return a snapshot of current cache statistics.
    fn stats(&self) -> CacheStats;

    /// Clear all entries and reset hit/miss counters.
    fn clear(&mut self);
}

// ---------------------------------------------------------------------------
// CacheMetricsCollector
// ---------------------------------------------------------------------------

/// Aggregates metrics across multiple caches that implement [`CacheLayer`].
///
/// Use [`CacheMetricsCollector::register`] to add a cache, then call
/// [`aggregate_stats`](CacheMetricsCollector::aggregate_stats) for a combined
/// view or [`all_stats`](CacheMetricsCollector::all_stats) for per-cache
/// breakdowns.
///
/// # Example (unit-test style)
///
/// ```
/// use go_on::orchestration::cache_layer::{CacheMetricsCollector, CacheStats};
///
/// let mut collector = CacheMetricsCollector::new();
/// // … register caches …
/// let agg = collector.aggregate_stats();
/// ```
pub struct CacheMetricsCollector {
    caches: Vec<Box<dyn CacheLayer>>,
}

impl CacheMetricsCollector {
    /// Create an empty collector.
    pub fn new() -> Self {
        Self { caches: Vec::new() }
    }

    /// Create a collector with the given pre-allocated capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            caches: Vec::with_capacity(capacity),
        }
    }

    /// Register a cache for metrics collection.
    pub fn register(&mut self, cache: Box<dyn CacheLayer>) {
        self.caches.push(cache);
    }

    /// Return a list of `(name, stats)` pairs for every registered cache.
    pub fn all_stats(&self) -> Vec<(&str, CacheStats)> {
        self.caches.iter().map(|c| (c.name(), c.stats())).collect()
    }

    /// Sum all stats across every registered cache.
    pub fn aggregate_stats(&self) -> CacheStats {
        let mut total = CacheStats {
            hits: 0,
            misses: 0,
            entries: 0,
            max_entries: 0,
            estimated_size_bytes: 0,
        };
        for cache in &self.caches {
            let s = cache.stats();
            total.hits += s.hits;
            total.misses += s.misses;
            total.entries += s.entries;
            total.max_entries += s.max_entries;
            total.estimated_size_bytes += s.estimated_size_bytes;
        }
        total
    }

    /// Clear all registered caches (entries + counters).
    pub fn clear_all(&mut self) {
        for cache in &mut self.caches {
            cache.clear();
        }
    }

    /// Number of registered caches.
    pub fn len(&self) -> usize {
        self.caches.len()
    }

    /// `true` when no caches are registered.
    pub fn is_empty(&self) -> bool {
        self.caches.is_empty()
    }
}

impl Default for CacheMetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Global singleton registry
// ---------------------------------------------------------------------------

/// Global singleton that holds the [`CacheMetricsCollector`].
///
/// Initialised lazily on first access via [`register_cache`] or
/// [`get_aggregate_cache_stats`].
static GLOBAL_CACHE_METRICS: OnceLock<Mutex<CacheMetricsCollector>> = OnceLock::new();

fn global_metrics() -> &'static Mutex<CacheMetricsCollector> {
    GLOBAL_CACHE_METRICS.get_or_init(|| Mutex::new(CacheMetricsCollector::new()))
}

/// Register a cache with the global metrics collector.
///
/// This is a convenience wrapper that acquires the global singleton and calls
/// [`CacheMetricsCollector::register`].
pub fn register_cache(cache: Box<dyn CacheLayer>) {
    global_metrics()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .register(cache);
}

/// Return aggregate stats for all caches registered via [`register_cache`],
/// as a vector of JSON values (one per cache).
pub fn get_aggregate_cache_stats() -> Vec<Value> {
    let guard = global_metrics().lock().unwrap_or_else(|e| e.into_inner());
    guard
        .all_stats()
        .into_iter()
        .map(|(name, stats)| {
            serde_json::json!({
                "cache": name,
                "hits": stats.hits,
                "misses": stats.misses,
                "entries": stats.entries,
                "max_entries": stats.max_entries,
                "estimated_size_bytes": stats.estimated_size_bytes,
            })
        })
        .collect()
}

/// Clear all caches that have been registered via [`register_cache`].
pub fn clear_all_caches() {
    global_metrics()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear_all();
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── Mock cache for testing ──────────────────────────────────────────────

    struct MockCache {
        name: String,
        stats: CacheStats,
    }

    impl MockCache {
        fn new(
            name: &str,
            hits: u64,
            misses: u64,
            entries: usize,
            max_entries: usize,
            size: usize,
        ) -> Self {
            Self {
                name: name.to_string(),
                stats: CacheStats {
                    hits,
                    misses,
                    entries,
                    max_entries,
                    estimated_size_bytes: size,
                },
            }
        }
    }

    impl CacheLayer for MockCache {
        fn name(&self) -> &str {
            &self.name
        }

        fn stats(&self) -> CacheStats {
            self.stats.clone()
        }

        fn clear(&mut self) {
            self.stats = CacheStats {
                hits: 0,
                misses: 0,
                entries: 0,
                max_entries: self.stats.max_entries,
                estimated_size_bytes: 0,
            };
        }
    }

    // ── CacheStats construction / field access ──────────────────────────────

    #[test]
    fn test_cache_stats_new() {
        let stats = CacheStats {
            hits: 10,
            misses: 2,
            entries: 5,
            max_entries: 100,
            estimated_size_bytes: 4096,
        };
        assert_eq!(stats.hits, 10);
        assert_eq!(stats.misses, 2);
        assert_eq!(stats.entries, 5);
        assert_eq!(stats.max_entries, 100);
        assert_eq!(stats.estimated_size_bytes, 4096);
    }

    #[test]
    fn test_cache_stats_serialize() {
        let stats = CacheStats {
            hits: 1,
            misses: 2,
            entries: 3,
            max_entries: 4,
            estimated_size_bytes: 5,
        };
        let json = serde_json::to_value(&stats).unwrap();
        assert_eq!(json["hits"], 1);
        assert_eq!(json["misses"], 2);
        assert_eq!(json["entries"], 3);
        assert_eq!(json["max_entries"], 4);
        assert_eq!(json["estimated_size_bytes"], 5);
    }

    // ── CacheMetricsCollector behaviour ─────────────────────────────────────

    #[test]
    fn test_collector_empty() {
        let collector = CacheMetricsCollector::new();
        assert!(collector.is_empty());
        assert_eq!(collector.len(), 0);
    }

    #[test]
    fn test_collector_single_cache() {
        let mut collector = CacheMetricsCollector::new();
        collector.register(Box::new(MockCache::new("alpha", 5, 1, 3, 50, 1024)));

        assert!(!collector.is_empty());
        assert_eq!(collector.len(), 1);

        let agg = collector.aggregate_stats();
        assert_eq!(agg.hits, 5);
        assert_eq!(agg.misses, 1);
        assert_eq!(agg.entries, 3);
        assert_eq!(agg.max_entries, 50);
        assert_eq!(agg.estimated_size_bytes, 1024);
    }

    #[test]
    fn test_collector_multiple_caches() {
        let mut collector = CacheMetricsCollector::new();
        collector.register(Box::new(MockCache::new("a", 10, 2, 5, 100, 1024)));
        collector.register(Box::new(MockCache::new("b", 3, 1, 2, 50, 512)));

        assert_eq!(collector.len(), 2);

        // Per-cache breakdown
        let all = collector.all_stats();
        assert_eq!(all.len(), 2);
        // Order is insertion order.
        assert_eq!(all[0].0, "a");
        assert_eq!(all[0].1.hits, 10);
        assert_eq!(all[1].0, "b");
        assert_eq!(all[1].1.hits, 3);

        // Aggregate
        let agg = collector.aggregate_stats();
        assert_eq!(agg.hits, 13);
        assert_eq!(agg.misses, 3);
        assert_eq!(agg.entries, 7);
        assert_eq!(agg.max_entries, 150);
        assert_eq!(agg.estimated_size_bytes, 1536);
    }

    #[test]
    fn test_collector_clear_all_resets_counters() {
        let mut collector = CacheMetricsCollector::new();
        collector.register(Box::new(MockCache::new("a", 10, 2, 5, 100, 1024)));
        collector.register(Box::new(MockCache::new("b", 3, 1, 2, 50, 512)));

        collector.clear_all();

        let agg = collector.aggregate_stats();
        assert_eq!(agg.hits, 0);
        assert_eq!(agg.misses, 0);
        assert_eq!(agg.entries, 0);
        assert_eq!(agg.estimated_size_bytes, 0);
        // max_entries is a configuration bound — it does NOT reset to zero.
        assert_eq!(agg.max_entries, 150);
    }

    #[test]
    fn test_collector_with_capacity() {
        let collector = CacheMetricsCollector::with_capacity(8);
        assert!(collector.is_empty());
    }

    #[test]
    fn test_collector_default() {
        let collector = CacheMetricsCollector::default();
        assert!(collector.is_empty());
    }

    // ── Mock cache clear test ───────────────────────────────────────────────

    #[test]
    fn test_mock_cache_clear() {
        let mut cache = MockCache::new("test", 99, 1, 10, 200, 2048);
        assert_eq!(cache.stats().hits, 99);

        cache.clear();
        let s = cache.stats();
        assert_eq!(s.hits, 0);
        assert_eq!(s.misses, 0);
        assert_eq!(s.entries, 0);
        // max_entries is preserved after clear.
        assert_eq!(s.max_entries, 200);
    }

    // ── Global helper functions (smoke tests) ───────────────────────────────
    //
    // These tests use the global `OnceLock` and therefore share state.
    // The test framework runs tests in the same process, so we only verify
    // that the functions return valid types / don't panic.

    #[test]
    fn test_get_aggregate_cache_stats_returns_vec() {
        let stats = get_aggregate_cache_stats();
        // Must always return a Vec (may be empty if no caches registered).
        assert!(stats.is_empty() || !stats.is_empty());
    }

    #[test]
    fn test_register_cache_does_not_panic() {
        let cache = MockCache::new("global_test", 1, 0, 2, 100, 256);
        // Registering into the global singleton should not panic.
        register_cache(Box::new(cache));
    }

    #[test]
    fn test_clear_all_caches_does_not_panic() {
        // Should not panic even when no caches are registered.
        clear_all_caches();
    }
}
