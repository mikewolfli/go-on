//! ACP Inflight Limiter
//!
//! Concurrency limiter that tracks in-flight requests globally and per-phase.
//! Returns a RAII guard that decrements the counter on drop.

use std::collections::HashMap;
use std::sync::Arc;
// NOTE: Intentionally using std::sync::Mutex (not tokio::sync::Mutex).
// All methods (try_enter, leave, snapshot) are synchronous and never hold the
// lock across .await points. std::sync::Mutex is faster for short counter
// operations — tokio::sync::Mutex would add overhead with zero benefit.
// See docs/log/log-20260625-1.md §Remaining Non-Issues.
use std::sync::Mutex as StdMutex;

use tracing::warn;

// ============================================================================
// Internal state
// ============================================================================

#[derive(Debug, Default)]
struct InflightState {
    global: usize,
    phase: HashMap<String, usize>,
}

// ============================================================================
// Inflight guard (RAII)
// ============================================================================

/// RAII guard that decrements inflight counters when dropped.
pub struct InflightGuard {
    limiter: Arc<InflightLimiter>,
    phase_name: String,
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.limiter.leave(&self.phase_name);
    }
}

impl Default for InflightGuard {
    fn default() -> Self {
        Self {
            limiter: Arc::new(InflightLimiter::default()),
            phase_name: String::new(),
        }
    }
}

// ============================================================================
// Inflight limiter (public API)
// ============================================================================

/// Inflight limiter for request concurrency control
#[derive(Debug, Default)]
pub struct InflightLimiter {
    inner: StdMutex<InflightState>,
}

impl InflightLimiter {
    /// Create a new inflight limiter
    pub fn new(_max_inflight: u32) -> Self {
        Self::default()
    }

    /// Try to enter the limiter, returning a guard on success.
    pub fn try_enter(
        self: &Arc<Self>,
        phase_name: &str,
        phase_limit: Option<u64>,
        global_limit: Option<u64>,
    ) -> Option<InflightGuard> {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        if let Some(limit) = global_limit {
            if guard.global as u64 >= limit.max(1) {
                return None;
            }
        }

        let phase_count = guard.phase.get(phase_name).copied().unwrap_or(0);
        if let Some(limit) = phase_limit {
            if phase_count as u64 >= limit.max(1) {
                return None;
            }
        }

        guard.global += 1;
        *guard.phase.entry(phase_name.to_string()).or_insert(0) += 1;
        Some(InflightGuard {
            limiter: Arc::clone(self),
            phase_name: phase_name.to_string(),
        })
    }

    fn leave(&self, phase_name: &str) {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.global = guard.global.saturating_sub(1);
        if let Some(value) = guard.phase.get_mut(phase_name) {
            *value = value.saturating_sub(1);
            if *value == 0 {
                guard.phase.remove(phase_name);
            }
        }
    }

    /// Snapshot of (global, phase_map) counts.
    pub fn snapshot(&self) -> (usize, HashMap<String, usize>) {
        self.inner
            .lock()
            .map(|guard| (guard.global, guard.phase.clone()))
            .unwrap_or_default()
    }

    /// Check if inflight limiter is healthy
    pub fn is_healthy(&self) -> bool {
        true
    }
}
