//! ACP Maintenance Tracker
//!
//! Tracks system maintenance cycles, including cleanup of expired entries,
//! vacuum operations, and error reporting.

use std::sync::Mutex as StdMutex;

use serde::Serialize;
use tracing::warn;

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
    /// Last SQLite expired entries removed
    pub last_sqlite_expired_removed: u64,
    /// Whether last cycle vacuumed cache
    pub last_cache_vacuumed: bool,
    /// Whether last cycle vacuumed vector store
    pub last_vector_vacuumed: bool,
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
    inner: StdMutex<MaintenanceSnapshot>,
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
            inner: StdMutex::new(MaintenanceSnapshot {
                running: false,
                cycles_total: 0,
                last_started_at: None,
                last_completed_at: None,
                last_memory_expired_removed: 0,
                last_sqlite_expired_removed: 0,
                last_cache_vacuumed: false,
                last_vector_vacuumed: false,
                last_error: None,
                last_maintenance: now,
                maintenance_interval: 3600, // 1 hour default
                next_maintenance_due: now + 3600,
                tasks_completed: 0,
                tasks_failed: 0,
                maintenance_in_progress: false,
            }),
        }
    }

    /// Get a snapshot of the maintenance state
    pub fn snapshot(&self) -> MaintenanceSnapshot {
        self.inner
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Begin maintenance
    pub fn begin_maintenance(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.maintenance_in_progress = true;
    }

    /// Note that maintenance has started
    pub fn note_started(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.running = true;
        guard.last_started_at = Some(now_ts());
        guard.last_error = None;
    }

    /// End maintenance
    pub fn end_maintenance(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.maintenance_in_progress = false;
        guard.last_maintenance = now_ts();
        guard.next_maintenance_due = guard.last_maintenance + guard.maintenance_interval;
    }

    /// Note that maintenance has failed
    pub fn note_failed(&self, error: &str) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.last_error = Some(error.to_string());
    }

    /// Record maintenance cycle completion
    pub fn note_completed(
        &self,
        memory_removed: usize,
        sqlite_removed: usize,
        cache_vacuumed: bool,
        vector_vacuumed: bool,
    ) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.running = false;
        guard.last_completed_at = Some(now_ts());
        guard.last_memory_expired_removed = memory_removed as u64;
        guard.last_sqlite_expired_removed = sqlite_removed as u64;
        guard.last_cache_vacuumed = cache_vacuumed;
        guard.last_vector_vacuumed = vector_vacuumed;
        guard.last_error = None;
        guard.cycles_total += 1;
    }

    /// Record health check result
    pub fn record_health_check(&self, healthy: bool) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        if healthy {
            guard.tasks_completed += 1;
        } else {
            guard.tasks_failed += 1;
        }
    }
}
