//! ACP Inflight Limiter
//!
//! Tracks in-flight request counts globally and per-phase for observability.
//! The former admission API (`try_enter` + RAII `InflightGuard`) had zero
//! production callers — the `phase_max_inflight` / `global_max_inflight`
//! config values were never wired into it, and actual concurrency control is
//! performed by `DrainGuard` (Semaphore) plus the transport-layer semaphores.
//! The limiter now exposes a live snapshot consumed by the `phase` protocol
//! payload.

use std::collections::HashMap;
// NOTE: Intentionally using std::sync::Mutex (not tokio::sync::Mutex).
// All methods are synchronous and never hold the lock across .await points.
use std::sync::Mutex as StdMutex;

// ============================================================================
// Internal state
// ============================================================================

#[derive(Debug, Default)]
struct InflightState {
    global: usize,
    phase: HashMap<String, usize>,
}

// ============================================================================
// Inflight limiter (public API)
// ============================================================================

/// Inflight limiter tracking request concurrency for observability.
#[derive(Debug, Default)]
pub struct InflightLimiter {
    inner: StdMutex<InflightState>,
}

impl InflightLimiter {
    /// Snapshot of (global, phase_map) counts.
    pub fn snapshot(&self) -> (usize, HashMap<String, usize>) {
        self.inner
            .lock()
            .map(|guard| (guard.global, guard.phase.clone()))
            .unwrap_or_default()
    }
}
