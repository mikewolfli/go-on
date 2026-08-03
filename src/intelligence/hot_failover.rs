//! HotFailover — model failure tracking.
//!
//! The chat fallback path records agent failures here (`record_failure`) and
//! the governance status payload observes the cumulative counter via
//! `metrics()`. The former cooldown blacklist (`failed_models` + lookup API)
//! was removed — production never read it back: fallback only wrote failures
//! and status only read `metrics()`. Failover sequencing itself lives in the
//! chat fallback chain and `HyperResilienceEngine`.

use std::sync::LazyLock;
use std::sync::RwLock;

use tracing::warn;

// ---------------------------------------------------------------------------
// Failover metrics
// ---------------------------------------------------------------------------

/// Metrics tracked during failover operations.
#[derive(Debug, Clone, Default)]
pub struct FailoverMetrics {
    /// Total failover events triggered.
    pub failover_count: u64,
    /// Total models skipped due to cooldown.
    pub cooldown_skips: u64,
    /// Cumulative extra latency added by failovers (ms).
    pub total_failover_latency_ms: u64,
}

// ---------------------------------------------------------------------------
// HotFailover
// ---------------------------------------------------------------------------

/// Model failure tracker. `record_failure` bumps the failover counter; the
/// cumulative metrics are surfaced in the governance status payload.
#[derive(Default)]
pub struct HotFailover {
    metrics: FailoverMetrics,
}

impl HotFailover {
    /// Create a new `HotFailover`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a model failure (increments the failover counter).
    pub fn record_failure(&mut self, model_id: &str) {
        self.metrics.failover_count += 1;
        warn!(
            model = %model_id,
            failover_count = self.metrics.failover_count,
            "HotFailover: model failure recorded"
        );
    }

    /// Return a snapshot of current failover metrics.
    pub fn metrics(&self) -> FailoverMetrics {
        self.metrics.clone()
    }
}

/// Global singleton HotFailover instance, shared across requests.
///
/// Constructed lazily at first access. Wired into governance status profile
/// for observability.
///
/// # Locking notes
///
/// Uses `RwLock` because the access pattern is read-dominated: governance
/// status handlers call `metrics()` (read-only) frequently, while
/// `record_failure()` writes are infrequent (only on model errors).
pub static HOT_FAILOVER_INSTANCE: LazyLock<RwLock<HotFailover>> =
    LazyLock::new(|| RwLock::new(HotFailover::new()));

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_failure_increments_counter() {
        let mut hf = HotFailover::new();

        let metrics = hf.metrics();
        assert_eq!(metrics.failover_count, 0);

        hf.record_failure("model-a");
        hf.record_failure("model-b");

        let metrics = hf.metrics();
        assert_eq!(metrics.failover_count, 2);
    }
}
