//! Cache Warming & Adaptive TTL — Predictive pre-warming, access-pattern-based
//! TTL adjustment, multi-tier cache management, and hit-rate telemetry.
//!
//! Enhances the existing FastPathCache with intelligent cache management
//! to maximize hit rates and minimize cold-start latency.

#![allow(dead_code)]
#![allow(unused_imports)]

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant};
use tracing::debug;

// ---------------------------------------------------------------------------
// CacheTier
// ---------------------------------------------------------------------------

/// Represents a cache tier in the multi-level hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheTier {
    /// L0: Inline/stack cache — sub-microsecond access.
    L0,
    /// L1: In-memory HashMap — ~1μs access.
    L1,
    /// L2: SQLite-backed — ~1ms access.
    L2,
    /// L3: Vector store — ~10ms access.
    L3,
}

impl CacheTier {
    /// Expected access latency in nanoseconds.
    pub fn latency_ns(&self) -> u64 {
        match self {
            Self::L0 => 0,
            Self::L1 => 1_000,
            Self::L2 => 1_000_000,
            Self::L3 => 10_000_000,
        }
    }

    /// Maximum number of entries this tier can hold.
    pub fn capacity(&self) -> usize {
        match self {
            Self::L0 => 8,
            Self::L1 => 512,
            Self::L2 => 10_000,
            Self::L3 => 100_000,
        }
    }
}

// ---------------------------------------------------------------------------
// AccessPattern
// ---------------------------------------------------------------------------

/// Tracks the access pattern for a cached entry to drive adaptive TTL.
#[derive(Debug, Clone)]
struct AccessPattern {
    /// Total number of times this entry has been accessed.
    access_count: u64,
    /// Timestamps of recent accesses (for frequency calculation).
    recent_accesses: VecDeque<Instant>,
    /// When this entry was first created.
    created_at: Instant,
    /// Current TTL based on access pattern.
    adaptive_ttl: Duration,
}

impl AccessPattern {
    fn new(initial_ttl: Duration) -> Self {
        Self {
            access_count: 0,
            recent_accesses: VecDeque::with_capacity(20),
            created_at: Instant::now(),
            adaptive_ttl: initial_ttl,
        }
    }

    fn record_access(&mut self) {
        self.access_count += 1;
        self.recent_accesses.push_back(Instant::now());
        if self.recent_accesses.len() > 20 {
            self.recent_accesses.pop_front();
        }
        self.recompute_ttl();
    }

    /// Access frequency in accesses per second (over the recent window).
    fn frequency(&self) -> f64 {
        if self.recent_accesses.len() < 2 {
            return 0.0;
        }
        let first = self.recent_accesses.front().unwrap();
        let last = self.recent_accesses.back().unwrap();
        let duration = last.duration_since(*first).as_secs_f64();
        if duration < 0.001 {
            return 0.0;
        }
        (self.recent_accesses.len() - 1) as f64 / duration
    }

    fn recompute_ttl(&mut self) {
        let freq = self.frequency();
        // Adaptive TTL ranges from 30s to 3600s based on frequency
        let ttl_seconds = if freq > 10.0 {
            3600.0 // Very hot: 1 hour
        } else if freq > 1.0 {
            1200.0 // Hot: 20 minutes
        } else if freq > 0.1 {
            300.0 // Warm: 5 minutes
        } else if freq > 0.01 {
            120.0 // Cool: 2 minutes
        } else {
            30.0 // Cold: 30 seconds
        };
        self.adaptive_ttl = Duration::from_secs_f64(ttl_seconds);
    }

    fn is_expired(&self, now: Instant) -> bool {
        // Use the most recent access time
        let last_access = self
            .recent_accesses
            .back()
            .copied()
            .unwrap_or(self.created_at);
        now.duration_since(last_access) > self.adaptive_ttl
    }
}

// ---------------------------------------------------------------------------
// CacheHitMetrics
// ---------------------------------------------------------------------------

/// Telemetry for cache performance monitoring.
#[derive(Debug, Clone, Default)]
pub struct CacheHitMetrics {
    /// Total cache lookups.
    pub total_lookups: AtomicCounter,
    /// Cache hits per tier.
    pub hits_l0: AtomicCounter,
    pub hits_l1: AtomicCounter,
    pub hits_l2: AtomicCounter,
    pub hits_l3: AtomicCounter,
    /// Cache misses.
    pub misses: AtomicCounter,
    /// Evictions per tier.
    pub evictions_l0: AtomicCounter,
    pub evictions_l1: AtomicCounter,
}

#[derive(Debug, Default)]
pub struct AtomicCounter(AtomicU64);

impl AtomicCounter {
    pub fn inc(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed) + 1
    }
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

impl Clone for AtomicCounter {
    fn clone(&self) -> Self {
        Self(AtomicU64::new(self.0.load(Ordering::Relaxed)))
    }
}

impl CacheHitMetrics {
    /// Overall cache hit rate [0.0, 1.0].
    pub fn overall_hit_rate(&self) -> f64 {
        let total = self.total_lookups.get();
        if total == 0 {
            return 0.0;
        }
        let hits =
            self.hits_l0.get() + self.hits_l1.get() + self.hits_l2.get() + self.hits_l3.get();
        hits as f64 / total as f64
    }

    /// Tier-specific hit rate.
    pub fn tier_hit_rate(&self, tier: CacheTier) -> f64 {
        let total = self.total_lookups.get();
        if total == 0 {
            return 0.0;
        }
        let hits = match tier {
            CacheTier::L0 => self.hits_l0.get(),
            CacheTier::L1 => self.hits_l1.get(),
            CacheTier::L2 => self.hits_l2.get(),
            CacheTier::L3 => self.hits_l3.get(),
        };
        hits as f64 / total as f64
    }

    /// Average cache access latency in nanoseconds.
    pub fn avg_latency_ns(&self) -> f64 {
        let total = self.total_lookups.get();
        if total == 0 {
            return 0.0;
        }
        let weighted = self.hits_l0.get() as f64 * CacheTier::L0.latency_ns() as f64
            + self.hits_l1.get() as f64 * CacheTier::L1.latency_ns() as f64
            + self.hits_l2.get() as f64 * CacheTier::L2.latency_ns() as f64
            + self.hits_l3.get() as f64 * CacheTier::L3.latency_ns() as f64
            + self.misses.get() as f64 * 50_000_000.0; // 50ms penalty for miss
        weighted / total as f64
    }
}

// ---------------------------------------------------------------------------
// PreWarmConfig
// ---------------------------------------------------------------------------

/// Configuration for predictive cache pre-warming.
#[derive(Debug, Clone)]
pub struct PreWarmConfig {
    /// Number of top entries to pre-load per category.
    pub top_n_per_category: usize,
    /// Categories to pre-warm: intent_matching, skill_matching, env_bootstrap, tool_routing
    pub categories: Vec<String>,
    /// Whether to run pre-warming at startup.
    pub warm_at_startup: bool,
    /// Whether to warm after idle periods longer than this duration (ms).
    pub warm_after_idle_ms: Option<u64>,
}

impl Default for PreWarmConfig {
    fn default() -> Self {
        Self {
            top_n_per_category: 20,
            categories: vec![
                "intent_matching".to_string(),
                "skill_matching".to_string(),
                "env_bootstrap".to_string(),
                "tool_routing".to_string(),
            ],
            warm_at_startup: true,
            warm_after_idle_ms: Some(30_000),
        }
    }
}

// ---------------------------------------------------------------------------
// CacheWarmingEngine
// ---------------------------------------------------------------------------

/// The central cache warming engine.
pub struct CacheWarmingEngine {
    config: PreWarmConfig,
    metrics: CacheHitMetrics,
    access_patterns: RwLock<HashMap<String, AccessPattern>>,
    /// Keys to pre-warm, populated from DiscoveryCenter historical data.
    pre_warm_keys: RwLock<HashMap<String, Vec<String>>>,
    last_access_time: RwLock<Instant>,
}

impl CacheWarmingEngine {
    pub fn new(config: PreWarmConfig) -> Self {
        Self {
            config,
            metrics: CacheHitMetrics::default(),
            access_patterns: RwLock::new(HashMap::new()),
            pre_warm_keys: RwLock::new(HashMap::new()),
            last_access_time: RwLock::new(Instant::now()),
        }
    }

    /// Record a cache access to update patterns and metrics.
    pub fn record_hit(&self, key: &str, tier: CacheTier) {
        self.metrics.total_lookups.inc();
        match tier {
            CacheTier::L0 => {
                self.metrics.hits_l0.inc();
            }
            CacheTier::L1 => {
                self.metrics.hits_l1.inc();
            }
            CacheTier::L2 => {
                self.metrics.hits_l2.inc();
            }
            CacheTier::L3 => {
                self.metrics.hits_l3.inc();
            }
        }

        let mut patterns = self.access_patterns.write().unwrap();
        patterns
            .entry(key.to_string())
            .or_insert_with(|| AccessPattern::new(Duration::from_secs(300)))
            .record_access();

        *self.last_access_time.write().unwrap() = Instant::now();
    }

    /// Record a cache miss.
    pub fn record_miss(&self) {
        self.metrics.total_lookups.inc();
        self.metrics.misses.inc();
    }

    /// Register keys that should be pre-warmed for a given category.
    pub fn register_pre_warm_keys(&self, category: &str, keys: Vec<String>) {
        self.pre_warm_keys
            .write()
            .unwrap()
            .insert(category.to_string(), keys);
    }

    /// Check if pre-warming should be triggered (after idle period).
    pub fn should_pre_warm(&self) -> bool {
        if let Some(idle_ms) = self.config.warm_after_idle_ms {
            let last = *self.last_access_time.read().unwrap();
            last.elapsed() > Duration::from_millis(idle_ms)
        } else {
            false
        }
    }

    /// Get the keys that should be pre-warmed for the configured categories.
    pub fn get_pre_warm_keys(&self) -> Vec<(String, Vec<String>)> {
        let keys = self.pre_warm_keys.read().unwrap();
        self.config
            .categories
            .iter()
            .filter_map(|cat| keys.get(cat).map(|k| (cat.clone(), k.clone())))
            .collect()
    }

    /// Get adaptive TTL for a cached key.
    pub fn adaptive_ttl(&self, key: &str, default_ttl: Duration) -> Duration {
        let patterns = self.access_patterns.read().unwrap();
        patterns
            .get(key)
            .map(|p| p.adaptive_ttl)
            .unwrap_or(default_ttl)
    }

    /// Check if a cached entry is expired based on adaptive TTL.
    pub fn is_expired(&self, key: &str, created_at: Instant) -> bool {
        let patterns = self.access_patterns.read().unwrap();
        match patterns.get(key) {
            Some(pattern) => pattern.is_expired(Instant::now()),
            None => {
                // No pattern yet: use short default TTL
                Instant::now().duration_since(created_at) > Duration::from_secs(60)
            }
        }
    }

    /// Remove stale entries from the access pattern store.
    pub fn cleanup_stale_patterns(&self, max_age: Duration) -> usize {
        let mut patterns = self.access_patterns.write().unwrap();
        let before = patterns.len();
        patterns.retain(|_, p| {
            p.recent_accesses
                .back()
                .map(|t| t.elapsed() < max_age)
                .unwrap_or(false)
        });
        before - patterns.len()
    }

    /// Get current hit rate metrics.
    pub fn metrics(&self) -> &CacheHitMetrics {
        &self.metrics
    }

    /// Promote an entry to a higher cache tier based on frequency.
    pub fn recommend_tier(&self, key: &str, current_tier: CacheTier) -> CacheTier {
        let patterns = self.access_patterns.read().unwrap();
        if let Some(pattern) = patterns.get(key) {
            let freq = pattern.frequency();
            match current_tier {
                CacheTier::L3 if freq > 1.0 => CacheTier::L2,
                CacheTier::L2 if freq > 10.0 => CacheTier::L1,
                CacheTier::L1 if freq > 50.0 => CacheTier::L0,
                _ => current_tier,
            }
        } else {
            current_tier
        }
    }
}

impl Default for CacheWarmingEngine {
    fn default() -> Self {
        Self::new(PreWarmConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_tier_latency_order() {
        assert!(CacheTier::L0.latency_ns() < CacheTier::L1.latency_ns());
        assert!(CacheTier::L1.latency_ns() < CacheTier::L2.latency_ns());
        assert!(CacheTier::L2.latency_ns() < CacheTier::L3.latency_ns());
    }

    #[test]
    fn test_access_pattern_frequency_zero_with_one_access() {
        let mut pattern = AccessPattern::new(Duration::from_secs(300));
        pattern.record_access();
        assert_eq!(pattern.frequency(), 0.0);
    }

    #[test]
    fn test_access_pattern_adaptive_ttl() {
        let mut pattern = AccessPattern::new(Duration::from_secs(300));
        // Single access: should get cold TTL (30s)
        pattern.record_access();
        assert!(pattern.adaptive_ttl.as_secs() <= 30);
    }

    #[test]
    fn test_cache_hit_metrics() {
        let metrics = CacheHitMetrics::default();
        metrics.total_lookups.inc();
        metrics.total_lookups.inc();
        metrics.hits_l0.inc();
        metrics.misses.inc();
        assert_eq!(metrics.overall_hit_rate(), 0.5);
    }

    #[test]
    fn test_record_hit_updates_metrics() {
        let engine = CacheWarmingEngine::default();
        engine.record_hit("key1", CacheTier::L1);
        engine.record_hit("key1", CacheTier::L1);
        engine.record_miss();
        assert_eq!(engine.metrics().total_lookups.get(), 3);
        assert_eq!(engine.metrics().hits_l1.get(), 2);
        assert_eq!(engine.metrics().misses.get(), 1);
    }

    #[test]
    fn test_adaptive_ttl_fallback() {
        let engine = CacheWarmingEngine::default();
        let default = Duration::from_secs(30);
        assert_eq!(engine.adaptive_ttl("unknown", default), default);
    }

    #[test]
    fn test_is_expired_new_key() {
        let engine = CacheWarmingEngine::default();
        let created = Instant::now();
        assert!(!engine.is_expired("new_key", created));
    }

    #[test]
    fn test_cleanup_stale_patterns() {
        let engine = CacheWarmingEngine::default();
        engine.record_hit("key1", CacheTier::L1);
        engine.record_hit("key2", CacheTier::L1);
        // Patterns just created should not be stale
        let removed = engine.cleanup_stale_patterns(Duration::from_secs(1));
        assert_eq!(removed, 0);
    }

    #[test]
    fn test_pre_warm_config_default() {
        let config = PreWarmConfig::default();
        assert!(config.warm_at_startup);
        assert_eq!(config.categories.len(), 4);
        assert_eq!(config.top_n_per_category, 20);
    }

    #[test]
    fn test_tier_capacity_ordering() {
        assert!(CacheTier::L0.capacity() < CacheTier::L1.capacity());
        assert!(CacheTier::L1.capacity() < CacheTier::L2.capacity());
        assert!(CacheTier::L2.capacity() < CacheTier::L3.capacity());
    }

    #[test]
    fn test_avg_latency_all_hits_l0() {
        let metrics = CacheHitMetrics::default();
        for _ in 0..100 {
            metrics.total_lookups.inc();
            metrics.hits_l0.inc();
        }
        assert!(metrics.avg_latency_ns() < 1000.0);
    }
}
