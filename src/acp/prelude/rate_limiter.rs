//! ACP Phase Rate Limiter
//!
//! Token-bucket rate limiter for per-phase request throttling.
//! Uses the shared `BucketMap` from `crate::shared::token_bucket`.

use std::collections::HashMap;

use crate::shared::token_bucket::{rpm_to_refill_per_second, BucketMap};

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

        let refill_per_second = rpm_to_refill_per_second(rpm_limit);
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
}
