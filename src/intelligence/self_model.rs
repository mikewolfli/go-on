//! BLUE38 F-GAP-21: Self-Model Core (M5 "自模型核心")
//!
//! Structured self-representation that tracks the system's own capabilities,
//! limitations, identity, and performance. All state is guarded behind
//! `Arc<Mutex<>>` for thread-safe access.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tracing::debug;

// ---------------------------------------------------------------------------
// Data Structures
// ---------------------------------------------------------------------------

/// Identity of the system — who it is, who made it, and descriptive metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfIdentity {
    pub system_name: String,
    pub version: String,
    pub description: String,
    pub creator: String,
    pub created_ms: u64,
    pub tags: Vec<String>,
}

/// Dynamic EMA-based statistics for a capability, updated on each execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityStats {
    /// EMA of success rate in [0.0, 1.0].
    pub effectiveness: f64,
    /// EMA adjusted by sample count; grows toward 1.0 as more samples arrive.
    pub confidence: f64,
    /// EMA of observed latency in ms.
    pub avg_latency_ms: f64,
    /// Total number of execution samples collected.
    pub sample_count: u64,
    /// Timestamp (ms since epoch) of the last recorded execution.
    pub last_updated: u64,
}

/// Configuration for the self-model core's behaviour.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfModelConfig {
    /// How often (in ms) the self-model should be refreshed / updated.
    pub update_interval_ms: u64,
    /// Maximum number of historical snapshots and records to retain.
    pub max_history: usize,
    /// Whether to enable performance tracking (snapshot recording).
    pub enable_performance_tracking: bool,
}

impl Default for SelfModelConfig {
    fn default() -> Self {
        Self {
            update_interval_ms: 60_000, // 1 minute
            max_history: 1000,
            enable_performance_tracking: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Inner {
    identity: Option<SelfIdentity>,
    capability_stats: HashMap<String, CapabilityStats>,
    last_update_ms: u64,
    /// Configuration driving tracking behaviour (retention, throttling,
    /// performance-tracking gate). Stored so `new(config)` stops ignoring it.
    config: SelfModelConfig,
}

// ---------------------------------------------------------------------------
// Public API — SelfModelCore
// ---------------------------------------------------------------------------

/// Thread-safe self-model core that provides a structured representation of
/// the system's own identity, capabilities, limitations, and performance.
#[derive(Debug, Clone)]
pub struct SelfModelCore {
    inner: Arc<Mutex<Inner>>,
}

impl SelfModelCore {
    /// Create a new self-model core with the given configuration.
    pub fn new(config: SelfModelConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                identity: None,
                capability_stats: HashMap::new(),
                last_update_ms: crate::shared::timestamps::now_ts_ms() as u64,
                config,
            })),
        }
    }

    // -- Identity ---------------------------------------------------------

    /// Set (or overwrite) the system identity.
    pub fn set_identity(&self, identity: SelfIdentity) {
        let mut inner = crate::lock_or_recover!(&self.inner, "intelligence");
        inner.identity = Some(identity);
        inner.last_update_ms = crate::shared::timestamps::now_ts_ms() as u64;
    }

    // -- Dynamic EMA Statistics -------------------------------------------

    /// Record an execution result for a capability and update EMA-based dynamic
    /// scoring.
    ///
    /// - `capability_name` — the name of the capability that was executed.
    /// - `success` — whether the execution succeeded.
    /// - `latency` — observed latency in milliseconds.
    ///
    /// EMA formula: `new_score = 0.3 * observed + 0.7 * old_score`.
    /// If no prior stats exist for the capability, the observed value is used
    /// directly as the starting point.
    ///
    /// Config-driven behaviour (all three `SelfModelConfig` fields are wired):
    /// - `enable_performance_tracking = false` → recording is a no-op.
    /// - `update_interval_ms` → per-capability EMA refresh throttle: a
    ///   capability's stats are refreshed at most once per interval, so bursts
    ///   of duplicate feedback within the interval do not skew the EMA.
    /// - `max_history` → `capability_stats` is bounded: when the tracked
    ///   capability count exceeds `max_history`, the least-recently-updated
    ///   entry is evicted.
    pub fn record_execution_result(&self, capability_name: &str, success: bool, latency: u64) {
        const EMA_ALPHA: f64 = 0.3;
        let observed_success = if success { 1.0 } else { 0.0 };
        let now = crate::shared::timestamps::now_ts_ms() as u64;

        let mut inner = crate::lock_or_recover!(&self.inner, "intelligence");
        // Performance tracking can be disabled via config.
        if !inner.config.enable_performance_tracking {
            return;
        }

        // Refresh throttle: the first sample is always recorded; later samples
        // for the same capability are skipped until update_interval_ms elapses.
        if let Some(existing) = inner.capability_stats.get(capability_name) {
            if now.saturating_sub(existing.last_updated) < inner.config.update_interval_ms {
                return;
            }
        }

        // Bound the tracked-capability map (before taking the entry borrow):
        // when at capacity and the capability is new, evict the
        // least-recently-updated entry first.
        if inner.capability_stats.len() >= inner.config.max_history
            && !inner.capability_stats.contains_key(capability_name)
        {
            if let Some(oldest) = inner
                .capability_stats
                .iter()
                .min_by_key(|(_, s)| s.last_updated)
                .map(|(k, _)| k.clone())
            {
                inner.capability_stats.remove(&oldest);
            }
        }

        let stats = inner
            .capability_stats
            .entry(capability_name.to_string())
            .or_insert(CapabilityStats {
                effectiveness: observed_success,
                confidence: 0.0,
                avg_latency_ms: latency as f64,
                sample_count: 0,
                last_updated: now,
            });

        stats.sample_count = stats.sample_count.saturating_add(1);
        stats.last_updated = now;

        // Effectiveness: EMA of observed success (1.0 / 0.0).
        stats.effectiveness =
            EMA_ALPHA * observed_success + (1.0 - EMA_ALPHA) * stats.effectiveness;

        // Confidence: EMA adjusted by sample count.
        // observed_confidence approaches 1.0 as sample_count grows.
        let observed_confidence = (stats.sample_count as f64) / (stats.sample_count as f64 + 10.0);
        stats.confidence = EMA_ALPHA * observed_confidence + (1.0 - EMA_ALPHA) * stats.confidence;

        // Latency: EMA of observed latency.
        stats.avg_latency_ms =
            EMA_ALPHA * (latency as f64) + (1.0 - EMA_ALPHA) * stats.avg_latency_ms;

        debug!(
            capability = %capability_name,
            effectiveness = stats.effectiveness,
            confidence = stats.confidence,
            latency_ms = stats.avg_latency_ms,
            samples = stats.sample_count,
            "SelfModel: updated capability stats"
        );
    }

    /// Return a snapshot of the per-capability EMA statistics.
    ///
    /// Read-side counterpart of `record_execution_result`: exposes the
    /// learned effectiveness/confidence/latency for each capability without
    /// locking the whole core. Consumed by self-model runtime endpoints.
    pub fn capability_stats(&self) -> HashMap<String, CapabilityStats> {
        let inner = crate::lock_or_recover!(&self.inner, "intelligence");
        inner.capability_stats.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(
        update_interval_ms: u64,
        max_history: usize,
        enable_performance_tracking: bool,
    ) -> SelfModelConfig {
        SelfModelConfig {
            update_interval_ms,
            max_history,
            enable_performance_tracking,
        }
    }

    #[test]
    fn test_max_history_evicts_stale_capabilities() {
        let core = SelfModelCore::new(config(0, 2, true));
        core.record_execution_result("cap-a", true, 10);
        // Distinct timestamps make eviction order deterministic.
        std::thread::sleep(std::time::Duration::from_millis(2));
        core.record_execution_result("cap-b", true, 10);
        std::thread::sleep(std::time::Duration::from_millis(2));
        core.record_execution_result("cap-c", true, 10);

        // max_history = 2: the least-recently-updated entry (cap-a, recorded
        // first) must have been evicted.
        let stats = core.capability_stats();
        assert_eq!(stats.len(), 2);
        assert!(!stats.contains_key("cap-a"));
        assert!(stats.contains_key("cap-b"));
        assert!(stats.contains_key("cap-c"));
    }

    #[test]
    fn test_performance_tracking_disabled_skips_recording() {
        let core = SelfModelCore::new(config(0, 100, false));
        core.record_execution_result("cap-a", true, 10);
        assert!(core.capability_stats().is_empty());
    }

    #[test]
    fn test_update_interval_throttles_per_capability() {
        // Huge interval: after the first sample, later samples for the same
        // capability are throttled until the interval elapses.
        let core = SelfModelCore::new(config(1_000_000_000, 100, true));
        core.record_execution_result("cap-a", true, 10);
        core.record_execution_result("cap-a", false, 20);
        core.record_execution_result("cap-a", true, 30);

        let stats = core.capability_stats();
        let cap = &stats["cap-a"];
        // Only the first sample was recorded.
        assert_eq!(cap.sample_count, 1);
        assert_eq!(cap.effectiveness, 1.0);

        // A different capability is unaffected (per-capability throttle).
        core.record_execution_result("cap-b", false, 40);
        assert_eq!(core.capability_stats()["cap-b"].sample_count, 1);
    }
}
