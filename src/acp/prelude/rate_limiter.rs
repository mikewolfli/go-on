//! ACP Phase Rate Limiter
//!
//! Token-bucket rate limiter for per-phase request throttling.
//! Uses the shared `BucketMap` from `crate::shared::token_bucket`.

use std::collections::HashMap;

use tracing::trace;

use crate::shared::token_bucket::BucketMap;

// ============================================================================
// Phase rate limiter (public API)
// ============================================================================

/// Phase rate limiter for phase-level throttling
#[derive(Debug, Default)]
pub struct PhaseRateLimiter {
    inner: BucketMap,
}

impl PhaseRateLimiter {
    /// Check if request can pass phase token bucket limiter.
    pub fn allow(&self, phase_name: &str, rpm_limit: u64, burst_capacity: Option<u64>) -> bool {
        if rpm_limit == 0 {
            return false;
        }

        let refill_per_second = rpm_limit as f64 / 60.0;
        let capacity = burst_capacity.unwrap_or(rpm_limit).max(1) as f64;

        self.inner
            .try_consume(phase_name, capacity, refill_per_second)
    }

    /// Number of tracked phases
    pub fn tracked_phases(&self) -> usize {
        self.inner.len()
    }

    /// Snapshot of current tokens per phase
    pub fn snapshot(&self) -> HashMap<String, (f64, f64)> {
        self.inner.snapshot()
    }

    /// Check if rate limiter is healthy
    pub fn is_healthy(&self) -> bool {
        let snapshot = self.inner.snapshot();
        let healthy = if snapshot.is_empty() {
            // No phases configured — nothing to rate-limit, considered healthy
            true
        } else {
            // Healthy if all tracked token buckets have at least one token available
            snapshot.values().all(|(tokens, _)| *tokens >= 1.0)
        };
        trace!(
            "rate_limiter health check: tracked_phases={}, healthy={}",
            snapshot.len(),
            healthy
        );
        healthy
    }
}
