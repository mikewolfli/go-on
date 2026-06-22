//! ACP Circuit Breaker
//!
//! Circuit breaker registry for managing circuit breaker state,
//! admission decisions, and snapshots.

use std::collections::HashMap;
use std::sync::Mutex as StdMutex;

use serde::Serialize;
use tracing::warn;

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
}

/// Circuit breaker stage
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[expect(
    dead_code,
    reason = "F-GAP-49 planned wiring; matched but not yet constructed"
)]
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
        }
    }
}

/// Circuit breaker admission result
///
/// Public API type — re-exported for ACP consumers.
#[expect(dead_code, reason = "public API for ACP consumers")]
#[non_exhaustive]
pub enum CircuitBreakerAdmission {
    Closed,
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
        let guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard
            .values()
            .filter(|state| matches!(state.stage, CircuitBreakerStage::Open))
            .count() as u32
    }

    /// Get circuit breaker snapshots
    pub fn snapshots(&self) -> Vec<CircuitBreakerSnapshot> {
        let guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
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
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });

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
}
