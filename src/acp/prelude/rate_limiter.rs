//! ACP Phase Rate Limiter
//!
//! Token-bucket rate limiter for per-phase request throttling.

use std::collections::HashMap;
use std::sync::Mutex as StdMutex;

use tracing::trace;

use crate::acp::prelude::functions::now_ts_ms;
use crate::shared::token_bucket::TokenBucket;

// ============================================================================
// Phase rate limiter (public API)
// ============================================================================

/// Phase rate limiter for phase-level throttling
#[derive(Debug, Default)]
pub struct PhaseRateLimiter {
    inner: StdMutex<HashMap<String, TokenBucket>>,
}

impl PhaseRateLimiter {
    /// Check if request can pass phase token bucket limiter.
    pub fn allow(&self, phase_name: &str, rpm_limit: u64, burst_capacity: Option<u64>) -> bool {
        if rpm_limit == 0 {
            return false;
        }

        let now = now_ts_ms();
        let refill_per_second = rpm_limit as f64 / 60.0;
        let capacity = burst_capacity.unwrap_or(rpm_limit).max(1) as f64;

        let mut guard = crate::lock_or_recover!(self.inner);
        let state = guard
            .entry(phase_name.to_string())
            .or_insert_with(|| TokenBucket::new_ms(capacity, refill_per_second, now));

        if (state.capacity - capacity).abs() > f64::EPSILON
            || (state.refill_rate - refill_per_second).abs() > f64::EPSILON
        {
            *state = TokenBucket::new_ms(capacity, refill_per_second, now);
        }

        state.refill_ms(now);
        if state.tokens < 1.0 {
            return false;
        }
        state.tokens -= 1.0;
        true
    }

    /// Number of tracked phases
    pub fn tracked_phases(&self) -> usize {
        self.inner.lock().map(|guard| guard.len()).unwrap_or(0)
    }

    /// Snapshot of current tokens per phase
    pub fn snapshot(&self) -> HashMap<String, (f64, f64)> {
        self.inner
            .lock()
            .map(|guard| {
                guard
                    .iter()
                    .map(|(phase, state)| (phase.clone(), (state.tokens, state.capacity)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Check if rate limiter is healthy
    pub fn is_healthy(&self) -> bool {
        let guard = crate::lock_or_recover!(self.inner);
        let healthy = if guard.is_empty() {
            // No phases configured — nothing to rate-limit, considered healthy
            true
        } else {
            // Healthy if all tracked token buckets have at least one token available
            guard.values().all(|bucket| bucket.tokens >= 1.0)
        };
        trace!(
            "rate_limiter health check: tracked_phases={}, healthy={}",
            guard.len(),
            healthy
        );
        healthy
    }
}
