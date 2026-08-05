//! ACP Maintenance Tracker
//!
//! Tracks system maintenance cycles, including cleanup of expired entries,
//! vacuum operations, and error reporting.

use serde::Serialize;

use crate::acp::prelude::functions::now_ts;

// ============================================================================
// Snapshot (public)
// ============================================================================

/// Maintenance snapshot
#[derive(Debug, Clone, Serialize, Default)]
pub struct MaintenanceSnapshot {
    /// Whether maintenance is running
    pub running: bool,
    /// Total maintenance cycles completed
    pub cycles_total: u64,
    /// Last maintenance started timestamp
    pub last_started_at: Option<i64>,
    /// Last maintenance completed timestamp
    pub last_completed_at: Option<i64>,
    /// Last memory expired entries removed
    pub last_memory_expired_removed: u64,
    /// Last error message if any
    pub last_error: Option<String>,
    /// Last maintenance timestamp (legacy)
    pub last_maintenance: i64,
    /// Maintenance interval in seconds (legacy)
    pub maintenance_interval: i64,
    /// Next maintenance due timestamp (legacy)
    pub next_maintenance_due: i64,
    /// Maintenance tasks completed (legacy)
    pub tasks_completed: u32,
    /// Maintenance tasks failed (legacy)
    pub tasks_failed: u32,
    /// Whether maintenance is in progress (legacy)
    pub maintenance_in_progress: bool,
}

// ============================================================================
// Maintenance tracker (public API)
// ============================================================================

/// Maintenance tracker for system maintenance
#[derive(Debug)]
pub struct MaintenanceTracker {
    snapshot: MaintenanceSnapshot,
}

impl Default for MaintenanceTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl MaintenanceTracker {
    /// Create a new maintenance tracker
    pub fn new() -> Self {
        let now = now_ts();
        Self {
            snapshot: MaintenanceSnapshot {
                running: false,
                cycles_total: 0,
                last_started_at: None,
                last_completed_at: None,
                last_memory_expired_removed: 0,
                last_error: None,
                last_maintenance: now,
                maintenance_interval: 3600, // 1 hour default
                next_maintenance_due: now + 3600,
                tasks_completed: 0,
                tasks_failed: 0,
                maintenance_in_progress: false,
            },
        }
    }

    /// Get a snapshot of the maintenance state
    pub fn snapshot(&self) -> MaintenanceSnapshot {
        self.snapshot.clone()
    }

    /// Note that maintenance has started
    pub fn note_started(&mut self) {
        self.snapshot.running = true;
        self.snapshot.last_started_at = Some(now_ts());
        self.snapshot.last_error = None;
    }

    /// Record maintenance cycle completion
    pub fn note_completed(&mut self, memory_removed: usize) {
        self.snapshot.running = false;
        self.snapshot.last_completed_at = Some(now_ts());
        self.snapshot.last_memory_expired_removed = memory_removed as u64;
        self.snapshot.last_error = None;
        self.snapshot.cycles_total += 1;
    }
}
