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
    /// Total requests processed
    pub total_requests: u64,
    /// Current phase
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
    total_requests: u64,
    current_phase: String,
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
            total_requests: 0,
            current_phase: "running".to_string(),
            last_health_check: now_ts(),
        }
    }

    /// Check if server is healthy
    pub fn is_healthy(&self) -> bool {
        self.healthy
    }

    /// Mark server as healthy
    pub fn mark_healthy(&mut self) {
        self.healthy = true;
    }

    /// Mark server as unhealthy
    pub fn mark_unhealthy(&mut self) {
        self.healthy = false;
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
            total_requests: self.total_requests,
            current_phase: self.current_phase.clone(),
            is_healthy: self.healthy,
            last_health_check: self.last_health_check,
            shutdown_requested: self.shutdown_requested,
        }
    }

    /// Increment total requests counter
    pub fn increment_requests(&mut self) {
        self.total_requests = self.total_requests.saturating_add(1);
    }

    /// Update current phase
    pub fn update_phase(&mut self, phase: &str) {
        self.current_phase = phase.to_string();
    }

    /// Update health check timestamp
    pub fn update_health_check(&mut self) {
        self.last_health_check = now_ts();
    }
}
