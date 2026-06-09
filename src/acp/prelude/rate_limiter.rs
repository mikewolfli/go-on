//! ACP Phase Rate Limiter
//!
//! Token-bucket rate limiter for per-phase request throttling.

use std::collections::HashMap;
use std::sync::Mutex as StdMutex;

use tracing::warn;

use crate::acp::prelude::functions::now_ts_ms;

// ============================================================================
// Internal token bucket
// ============================================================================

#[derive(Debug, Clone)]
struct TokenBucketState {
    tokens: f64,
    capacity: f64,
    refill_per_second: f64,
    last_refill_ms: i64,
}

impl TokenBucketState {
    fn new(capacity: f64, refill_per_second: f64, now_ms: i64) -> Self {
        Self {
            tokens: capacity,
            capacity,
            refill_per_second,
            last_refill_ms: now_ms,
        }
    }

    fn refill(&mut self, now_ms: i64) {
        let elapsed_ms = (now_ms - self.last_refill_ms).max(0) as f64;
        if elapsed_ms > 0.0 {
            let refill = elapsed_ms / 1000.0 * self.refill_per_second;
            self.tokens = (self.tokens + refill).min(self.capacity);
            self.last_refill_ms = now_ms;
        }
    }
}

// ============================================================================
// Phase rate limiter (public API)
// ============================================================================

/// Phase rate limiter for phase-level throttling
#[derive(Debug, Default)]
pub struct PhaseRateLimiter {
    inner: StdMutex<HashMap<String, TokenBucketState>>,
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

        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        let state = guard
            .entry(phase_name.to_string())
            .or_insert_with(|| TokenBucketState::new(capacity, refill_per_second, now));

        if (state.capacity - capacity).abs() > f64::EPSILON
            || (state.refill_per_second - refill_per_second).abs() > f64::EPSILON
        {
            *state = TokenBucketState::new(capacity, refill_per_second, now);
        }

        state.refill(now);
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
        true
    }
}
