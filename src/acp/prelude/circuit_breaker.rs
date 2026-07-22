//! ACP Circuit Breaker
//!
//! Circuit breaker registry for managing circuit breaker state,
//! admission decisions, and snapshots.

use std::collections::HashMap;
// NOTE: Intentionally using std::sync::Mutex (not tokio::sync::Mutex).
// All methods (record_success, record_failure, is_open, snapshot) are synchronous
// and never hold the lock across .await points. std::sync::Mutex is faster for
// short critical sections — tokio::sync::Mutex would add waker allocation overhead
// with zero benefit. See docs/log/log-20260625-1.md §Remaining Non-Issues.
use std::sync::Mutex as StdMutex;

use serde::Serialize;

use crate::acp::prelude::functions::now_ts;

// ============================================================================
// Snapshot (public)
// ============================================================================

/// Circuit breaker snapshot
#[derive(Debug, Clone, Serialize, Default)]
pub struct CircuitBreakerSnapshot {
    /// Circuit breaker name
    pub name: String,
    /// Current state (closed, open, half-open)
    pub state: String,
    /// Failure count
    pub failure_count: u32,
    /// Success count
    pub success_count: u32,
    /// Last state change timestamp
    pub last_state_change: i64,
    /// Total failures
    pub total_failures: u64,
    /// Total successes
    pub total_successes: u64,
}

// ============================================================================
// Internal state
// ============================================================================

/// Circuit breaker state
#[derive(Debug, Clone)]
struct CircuitBreakerState {
    stage: CircuitBreakerStage,
    failure_count: u32,
    success_count: u32,
    last_state_change: i64,
    open_until: Option<i64>,
    failure_threshold: u32,
}

/// Circuit breaker stage
#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum CircuitBreakerStage {
    #[default]
    Closed,
    Open,
    HalfOpen,
}

impl Default for CircuitBreakerState {
    fn default() -> Self {
        Self {
            stage: CircuitBreakerStage::Closed,
            failure_count: 0,
            success_count: 0,
            last_state_change: 0,
            open_until: None,
            failure_threshold: 5,
        }
    }
}

/// Circuit breaker admission result
///
/// Public API type — re-exported for ACP consumers.
#[non_exhaustive]
pub enum CircuitBreakerAdmission {
    /// The breaker is Closed — request is allowed.
    Closed,
    /// Request allowed as a probe to test if the downstream has recovered.
    HalfOpenProbe,
    /// Request is rejected because the breaker is not ready.
    Rejected {
        state: &'static str,
        retry_after_seconds: Option<i64>,
    },
}

// ============================================================================
// Registry (public API)
// ============================================================================

/// Circuit breaker registry for managing circuit breakers
#[derive(Debug, Default)]
pub struct CircuitBreakerRegistry {
    inner: StdMutex<HashMap<String, CircuitBreakerState>>,
}

impl CircuitBreakerRegistry {
    /// Create a new circuit breaker registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the number of open circuit breakers
    pub fn open_count(&self) -> u32 {
        let guard = crate::lock_or_recover!(self.inner);
        guard
            .values()
            .filter(|state| matches!(state.stage, CircuitBreakerStage::Open))
            .count() as u32
    }

    /// Get circuit breaker snapshots
    pub fn snapshots(&self) -> Vec<CircuitBreakerSnapshot> {
        let guard = crate::lock_or_recover!(self.inner);
        guard
            .iter()
            .map(|(name, state)| CircuitBreakerSnapshot {
                name: name.clone(),
                state: match state.stage {
                    CircuitBreakerStage::Closed => "closed".to_string(),
                    CircuitBreakerStage::Open => "open".to_string(),
                    CircuitBreakerStage::HalfOpen => "half-open".to_string(),
                },
                failure_count: state.failure_count,
                success_count: state.success_count,
                last_state_change: state.last_state_change,
                total_failures: state.failure_count as u64,
                total_successes: state.success_count as u64,
            })
            .collect()
    }

    /// Reset one circuit breaker or all tracked breakers back to closed state.
    pub fn reset(&self, name: Option<&str>) -> usize {
        let mut guard = crate::lock_or_recover!(self.inner);

        let reset_state = |state: &mut CircuitBreakerState| {
            state.stage = CircuitBreakerStage::Closed;
            state.failure_count = 0;
            state.success_count = 0;
            state.last_state_change = now_ts();
            state.open_until = None;
        };

        if let Some(name) = name {
            if let Some(state) = guard.get_mut(name) {
                reset_state(state);
                return 1;
            }
            return 0;
        }

        let count = guard.len();
        for state in guard.values_mut() {
            reset_state(state);
        }
        count
    }

    /// Check if circuit breakers are healthy
    pub fn is_healthy(&self) -> bool {
        self.open_count() == 0
    }

    /// Check whether the named breaker is in the HalfOpen state.
    /// Returns `false` if the breaker is not tracked.
    pub fn is_half_open(&self, name: &str) -> bool {
        let guard = crate::lock_or_recover!(self.inner);
        guard
            .get(name)
            .is_some_and(|state| state.stage == CircuitBreakerStage::HalfOpen)
    }

    /// Admit a request for the given circuit breaker.
    ///
    /// Returns [`CircuitBreakerAdmission::Closed`] when the breaker is green,
    /// [`CircuitBreakerAdmission::HalfOpenProbe`] when a probe is allowed,
    /// and [`CircuitBreakerAdmission::Rejected`] when the breaker is open.
    pub fn admit(&self, name: &str) -> CircuitBreakerAdmission {
        let mut guard = crate::lock_or_recover!(self.inner);
        let state = guard.entry(name.to_string()).or_default();
        match state.stage {
            CircuitBreakerStage::Closed => CircuitBreakerAdmission::Closed,
            CircuitBreakerStage::HalfOpen => {
                // Allow a probe request.
                CircuitBreakerAdmission::HalfOpenProbe
            }
            CircuitBreakerStage::Open => {
                // Check if the open timeout has expired — move to HalfOpen.
                if let Some(until) = state.open_until {
                    if now_ts() >= until {
                        state.stage = CircuitBreakerStage::HalfOpen;
                        state.last_state_change = now_ts();
                        return CircuitBreakerAdmission::HalfOpenProbe;
                    }
                    let retry = until - now_ts();
                    return CircuitBreakerAdmission::Rejected {
                        state: "open",
                        retry_after_seconds: Some(retry),
                    };
                }
                CircuitBreakerAdmission::Rejected {
                    state: "open",
                    retry_after_seconds: None,
                }
            }
        }
    }

    /// Record a successful call, closing or moving out of half-open state.
    pub fn record_success(&self, name: &str) {
        let mut guard = crate::lock_or_recover!(self.inner);
        if let Some(state) = guard.get_mut(name) {
            state.success_count += 1;
            match state.stage {
                CircuitBreakerStage::HalfOpen | CircuitBreakerStage::Open => {
                    // Reset to closed on success.
                    state.stage = CircuitBreakerStage::Closed;
                    state.failure_count = 0;
                    state.last_state_change = now_ts();
                    state.open_until = None;
                }
                CircuitBreakerStage::Closed => {}
            }
        }
    }

    /// Record a failure, potentially transitioning to Open.
    pub fn record_failure(&self, name: &str) {
        self._record_failure(name);
    }

    /// Internal implementation shared by `record_failure`.
    fn _record_failure(&self, name: &str) {
        let mut guard = crate::lock_or_recover!(self.inner);
        let state = guard.entry(name.to_string()).or_default();
        state.failure_count += 1;
        match state.stage {
            CircuitBreakerStage::Closed => {
                if state.failure_count >= state.failure_threshold {
                    state.stage = CircuitBreakerStage::Open;
                    state.last_state_change = now_ts();
                    state.open_until = Some(now_ts() + 30);
                }
            }
            CircuitBreakerStage::HalfOpen => {
                // Failure during half-open probe → back to open.
                state.stage = CircuitBreakerStage::Open;
                state.last_state_change = now_ts();
                state.open_until = Some(now_ts() + 30);
            }
            CircuitBreakerStage::Open => {
                // Already open, just count.
            }
        }
    }
}
