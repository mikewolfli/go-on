//! ACP Circuit Breaker Registry — read-only snapshot provider.
//!
//! This registry stores circuit breaker states for health-status reporting.
//! State transitions (Open/Closed/HalfOpen) and per-service health/degradation
//! are managed by `resilience::hyper_resilience::HyperResilienceEngine` — the
//! single resilience authority. The outer `Arc<StdMutex<CircuitBreakerRegistry>>`
//! in `AcpServer` provides thread safety; the registry reads live state from
//! the engine via `source`.

use std::collections::HashMap;
use std::sync::Arc;

use crate::acp::prelude::functions::now_ts;
use crate::resilience::hyper_resilience::HyperResilienceEngine;
use serde::Serialize;

// ============================================================================
// Snapshot (public)
// ============================================================================

/// Circuit breaker snapshot
#[derive(Debug, Clone, Serialize, Default)]
pub struct CircuitBreakerSnapshot {
    /// Circuit breaker name
    pub name: String,
    /// Current state ("open"/"closed"/"halfopen" from the live source;
    /// "unknown" when no live source is attached — never a fake "closed").
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

/// Circuit breaker state (bookkeeping map; the live source is the engine).
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
/// Real breaker + health state lives in the unified `HyperResilienceEngine`.
/// When a `source` is attached, `snapshots()` / `open_count()` / `is_healthy()`
/// / `reset()` read/write the live state through it instead of the (never
/// produced) built-in map.
///
/// **Without a source**, `snapshots()` reports each tracked breaker's state as
/// `"unknown"` (not `"closed"`) — the built-in map has no producer, so any
/// concrete state would be fabricated; `open_count()` stays 0. In practice the
/// server always attaches the engine via [`CircuitBreakerRegistry::attach_source`].
#[derive(Debug, Default)]
pub struct CircuitBreakerRegistry {
    pub(crate) inner: HashMap<String, CircuitBreakerState>,
    /// Live source of truth (the server's unified hyper-resilience engine).
    source: Option<Arc<HyperResilienceEngine>>,
}

impl CircuitBreakerRegistry {
    /// Create a new circuit breaker registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach the unified hyper-resilience engine as the live source.
    /// Called once during `ServerBuilder::build()`. Without a source the
    /// registry reports empty/"unknown" states (degraded observability, no
    /// fabricated "closed" data).
    pub(crate) fn attach_source(&mut self, hre: Arc<HyperResilienceEngine>) {
        self.source = Some(hre);
    }

    /// Get the number of open circuit breakers.
    pub fn open_count(&self) -> u32 {
        if let Some(ref hre) = self.source {
            return hre.open_breaker_count();
        }
        // No source attached — the built-in map has no producer, so nothing is open.
        0
    }

    /// Get circuit breaker snapshots.
    pub fn snapshots(&self) -> Vec<CircuitBreakerSnapshot> {
        if let Some(ref hre) = self.source {
            let now = now_ts();
            return hre
                .breaker_snapshots()
                .into_iter()
                .map(
                    |(name, state, failures, total, successes)| CircuitBreakerSnapshot {
                        name,
                        state: match state {
                            crate::resilience::hyper_resilience::CircuitState::Open => {
                                "open".to_string()
                            }
                            crate::resilience::hyper_resilience::CircuitState::HalfOpen => {
                                "halfopen".to_string()
                            }
                            crate::resilience::hyper_resilience::CircuitState::Closed => {
                                "closed".to_string()
                            }
                        },
                        failure_count: failures as u32,
                        success_count: successes as u32,
                        last_state_change: now,
                        total_failures: failures,
                        total_successes: total,
                    },
                )
                .collect();
        }
        self.inner
            .iter()
            .map(|(name, state)| CircuitBreakerSnapshot {
                name: name.clone(),
                // No live source: report "unknown" instead of a fabricated
                // "closed" so health surfaces never show false positives.
                state: "unknown".to_string(),
                failure_count: state.failure_count,
                success_count: state.success_count,
                last_state_change: state.last_state_change,
                total_failures: state.failure_count as u64,
                total_successes: state.success_count as u64,
            })
            .collect()
    }

    /// Recover one breaker or all tracked breakers back to the healthy
    /// baseline via the unified engine (breaker closed, counters zeroed,
    /// health reset). Returns the number of services actually recovered.
    pub fn reset(&mut self, name: Option<&str>) -> usize {
        if let Some(ref hre) = self.source {
            return hre.recover_services(name).len();
        }
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
