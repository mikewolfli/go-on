//! ACP Circuit Breaker Registry — read-only snapshot provider.
//!
//! This registry stores circuit breaker states for Prometheus metrics and
//! health-status reporting. State transitions (Open/Closed/HalfOpen) are
//! managed by `resilience::hyper_resilience::HyperResilienceEngine` — the
//! single source of truth for the circuit breaker state machine.
//!
//! The outer `Arc<StdMutex<CircuitBreakerRegistry>>` in `AcpServer` provides
//! thread safety; this struct holds the data directly (no inner Mutex).

use std::collections::HashMap;

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
pub(crate) struct CircuitBreakerState {
    pub(crate) stage: CircuitBreakerStage,
    pub(crate) failure_count: u32,
    pub(crate) success_count: u32,
    pub(crate) last_state_change: i64,
}

/// Circuit breaker stage
///
/// Only `Closed` is actively used; `Open` and `HalfOpen` are reserved
/// for future sync with `HyperResilienceEngine`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum CircuitBreakerStage {
    #[default]
    Closed,
    #[expect(
        dead_code,
        reason = "reserved for future HyperResilienceEngine sync; never constructed yet"
    )]
    Open,
    #[expect(
        dead_code,
        reason = "reserved for future HyperResilienceEngine sync; never constructed yet"
    )]
    HalfOpen,
}

// ============================================================================
// Registry (public API for read-only metrics)
// ============================================================================

/// Circuit breaker registry — stores states for metrics/UI reporting.
///
/// State transitions happen in `HyperResilienceEngine`. This registry is
/// populated by the health-check cycle and is read-only from the ACP side.
///
/// Thread safety is guaranteed by the outer `Arc<StdMutex<>>` in AcpServer.
#[derive(Debug, Default)]
pub struct CircuitBreakerRegistry {
    pub(crate) inner: HashMap<String, CircuitBreakerState>,
}

impl CircuitBreakerRegistry {
    /// Create a new circuit breaker registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the number of open circuit breakers.
    pub fn open_count(&self) -> u32 {
        self.inner
            .values()
            .filter(|state| matches!(state.stage, CircuitBreakerStage::Open))
            .count() as u32
    }

    /// Get circuit breaker snapshots.
    pub fn snapshots(&self) -> Vec<CircuitBreakerSnapshot> {
        self.inner
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

    /// Check if all circuit breakers are closed (healthy).
    pub fn is_healthy(&self) -> bool {
        self.open_count() == 0
    }

    /// Reset one circuit breaker or all tracked breakers back to closed state.
    /// The caller must hold the outer lock; takes `&mut self` for interior access.
    pub fn reset(&mut self, name: Option<&str>) -> usize {
        let now = now_ts();
        match name {
            Some(name) => {
                if let Some(state) = self.inner.get_mut(name) {
                    state.stage = CircuitBreakerStage::Closed;
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
                    state.stage = CircuitBreakerStage::Closed;
                    state.failure_count = 0;
                    state.success_count = 0;
                    state.last_state_change = now;
                }
                count
            }
        }
    }
}
