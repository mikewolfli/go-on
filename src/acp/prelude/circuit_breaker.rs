//! ACP Circuit Breaker Registry — read-only snapshot provider.
//!
//! This registry stores circuit breaker states for Prometheus metrics and
//! health-status reporting. State transitions (Open/Closed/HalfOpen) are
//! managed by `resilience::hyper_resilience::HyperResilienceEngine` — the
//! single source of truth for the circuit breaker state machine.
//!
//! The outer `Arc<StdMutex<CircuitBreakerRegistry>>` in `AcpServer` provides
//! thread safety; this struct holds the data directly (no inner Mutex).
//!
//! NOTE: This registry currently only tracks Closed state. Open/HalfOpen
//! transitions are managed by HyperResilienceEngine internally. If future
//! wiring is needed to sync HRE state back to this registry, restore the
//! Open/HalfOpen variants and transition_to() method.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use crate::acp::prelude::functions::now_ts;
use serde::Serialize;

// ============================================================================
// Snapshot (public)
// ============================================================================

/// Circuit breaker snapshot
#[derive(Debug, Clone, Serialize, Default)]
pub struct CircuitBreakerSnapshot {
    /// Circuit breaker name
    pub name: String,
    /// Current state ("open"/"closed"/"halfopen") from the live source
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
pub(crate) struct CircuitBreakerState {
    pub(crate) failure_count: u32,
    pub(crate) success_count: u32,
    pub(crate) last_state_change: i64,
}

// ============================================================================
// Registry (public API for read-only metrics)
// ============================================================================

/// Circuit breaker registry — stores states for metrics/UI reporting.
///
/// Real breaker state lives in `optimization::failure_prevention::FailurePrevention`
/// (the per-agent circuit breakers driven by request outcomes). When a
/// `source` is attached, `snapshots()` / `open_count()` / `is_healthy()`
/// read the live state from it instead of the (previously always-empty)
/// built-in map.
#[derive(Debug, Default)]
pub struct CircuitBreakerRegistry {
    pub(crate) inner: HashMap<String, CircuitBreakerState>,
    /// Live source of truth for breaker state (the server's FailurePrevention).
    source: Option<Arc<StdMutex<crate::optimization::failure_prevention::FailurePrevention>>>,
}

impl CircuitBreakerRegistry {
    /// Create a new circuit breaker registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach the live FailurePrevention as the snapshot source.
    /// Called once during ServerBuilder::build(); without a source the
    /// registry reports empty/closed (degraded observability, no fake data).
    pub(crate) fn attach_source(
        &mut self,
        fp: Arc<StdMutex<crate::optimization::failure_prevention::FailurePrevention>>,
    ) {
        self.source = Some(fp);
    }

    /// Get the number of open circuit breakers.
    pub fn open_count(&self) -> u32 {
        use crate::optimization::failure_prevention::CircuitBreakerState as FpState;
        if let Some(ref fp) = self.source {
            let fp = fp.lock().unwrap_or_else(|e| e.into_inner());
            return fp
                .breaker_snapshots()
                .iter()
                .filter(|(_, state, ..)| *state == FpState::Open)
                .count() as u32;
        }
        // No source attached — the built-in map has no producer, so nothing is open.
        0
    }

    /// Get circuit breaker snapshots.
    pub fn snapshots(&self) -> Vec<CircuitBreakerSnapshot> {
        use crate::optimization::failure_prevention::CircuitBreakerState as FpState;
        if let Some(ref fp) = self.source {
            let fp = fp.lock().unwrap_or_else(|e| e.into_inner());
            return fp
                .breaker_snapshots()
                .into_iter()
                .map(
                    |(name, state, failures, total, successes)| CircuitBreakerSnapshot {
                        name,
                        state: match state {
                            FpState::Open => "open".to_string(),
                            FpState::HalfOpen => "halfopen".to_string(),
                            FpState::Closed => "closed".to_string(),
                        },
                        failure_count: failures,
                        success_count: successes as u32,
                        last_state_change: now_ts(),
                        total_failures: failures as u64,
                        total_successes: total,
                    },
                )
                .collect();
        }
        self.inner
            .iter()
            .map(|(name, state)| CircuitBreakerSnapshot {
                name: name.clone(),
                state: "closed".to_string(),
                failure_count: state.failure_count,
                success_count: state.success_count,
                last_state_change: state.last_state_change,
                total_failures: state.failure_count as u64,
                total_successes: state.success_count as u64,
            })
            .collect()
    }

    /// Check if all circuit breakers are closed.
    pub fn is_healthy(&self) -> bool {
        self.open_count() == 0
    }

    /// Reset one circuit breaker or all tracked breakers back to initial state.
    /// The caller must hold the outer lock; takes `&mut self` for interior access.
    pub fn reset(&mut self, name: Option<&str>) -> usize {
        let now = now_ts();
        match name {
            Some(name) => {
                if let Some(state) = self.inner.get_mut(name) {
                    state.failure_count = 0;
                    state.success_count = 0;
                    state.last_state_change = now;
                    1
                } else {
                    0
                }
            }
            None => {
                let count = self.inner.len();
                for state in self.inner.values_mut() {
                    state.failure_count = 0;
                    state.success_count = 0;
                    state.last_state_change = now;
                }
                count
            }
        }
    }
}
