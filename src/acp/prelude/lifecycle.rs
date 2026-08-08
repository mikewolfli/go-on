//! ACP Lifecycle State
//!
//! Tracks server lifecycle: health status, shutdown state, uptime, phase.

use serde::Serialize;

use crate::acp::prelude::functions::now_ts;

// ============================================================================
// Snapshot (public)
// ============================================================================

/// Lifecycle snapshot
#[derive(Debug, Clone, Serialize, Default)]
pub struct LifecycleSnapshot {
    /// Server start time
    pub start_time: i64,
    /// Uptime in seconds
    pub uptime_seconds: i64,
    /// Current phase (derived: "running" / "shutting_down")
    pub current_phase: String,
    /// Is healthy
    pub is_healthy: bool,
    /// Health check timestamp
    pub last_health_check: i64,
    /// Shutdown requested
    pub shutdown_requested: bool,
}

// ============================================================================
// Lifecycle state (public API)
// ============================================================================

/// Lifecycle state for server lifecycle management
#[derive(Debug)]
pub struct LifecycleState {
    healthy: bool,
    shutdown_requested: bool,
    start_time: i64,
    last_health_check: i64,
}

impl Default for LifecycleState {
    fn default() -> Self {
        Self::new()
    }
}

impl LifecycleState {
    /// Create a new lifecycle state
    pub fn new() -> Self {
        Self {
            healthy: true,
            shutdown_requested: false,
            start_time: now_ts(),
            last_health_check: now_ts(),
        }
    }

    /// Check if server is healthy
    pub fn is_healthy(&self) -> bool {
        self.healthy
    }

    /// Update the health flag from real runtime signals (e.g. open circuit
    /// breakers). Previously the flag was set once at construction and never
    /// updated, so `/health` always reported `is_healthy: true`. The health
    /// check timestamp is refreshed on every update so `/health` reflects
    /// when the state was last re-evaluated.
    pub fn set_healthy(&mut self, healthy: bool) {
        self.healthy = healthy;
        self.last_health_check = now_ts();
    }

    /// Check if shutdown has been requested
    pub fn shutdown_requested(&self) -> bool {
        self.shutdown_requested
    }

    /// Begin shutdown
    pub fn begin_shutdown(&mut self) {
        self.shutdown_requested = true;
    }

    /// Get a snapshot of the lifecycle state
    pub fn snapshot(&self) -> LifecycleSnapshot {
        let now = now_ts();
        LifecycleSnapshot {
            start_time: self.start_time,
            uptime_seconds: now.saturating_sub(self.start_time),
            // Derived, not a stored constant: previously `current_phase` and
            // `total_requests` were set once at construction and never
            // updated — the serialized health output always showed the same
            // values regardless of actual state.
            current_phase: if self.shutdown_requested {
                "shutting_down".to_string()
            } else {
                "running".to_string()
            },
            is_healthy: self.healthy,
            last_health_check: self.last_health_check,
            shutdown_requested: self.shutdown_requested,
        }
    }
}
