//! ACP Lock Monitor
//!
//! Lock monitoring infrastructure: tracks poisonings and recovery for
//! all ACP `std::sync::Mutex` instances. Wait-time instrumentation has
//! been removed (log-20260622-5: lock monitor stats never queried in
//! production; the atomic ops and Instant::now() syscalls added ~500ns
//! per acquisition with zero downstream consumers).

use std::sync::Mutex as StdMutex;

use tokio::sync::Mutex as TokioMutex;
use tracing::warn;

/// AcpLockMonitor — retained as a zero-overhead placeholder for ABI compatibility.
/// All counter/timing fields removed (never queried in production).
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct AcpLockMonitor;

// ============================================================================
// Lock helper functions
// ============================================================================

/// Acquire a `std::sync::Mutex` with poison recovery.
///
/// Handles poisoned mutexes gracefully, recovering the state and continuing.
/// No wait-time instrumentation (removed for performance — see module docs).
pub fn with_acp_lock<T, R, F>(_name: &'static str, mutex: &StdMutex<T>, operation: F) -> R
where
    F: FnOnce(&mut T) -> R,
{
    match mutex.lock() {
        Ok(mut guard) => operation(&mut guard),
        Err(poisoned) => {
            warn!(
                target: "acp::locks",
                "ACP lock '{}' was poisoned; continuing with recovered state",
                _name
            );
            let mut guard = poisoned.into_inner();
            operation(&mut guard)
        }
    }
}

/// Acquire a `tokio::sync::Mutex` (async version).
///
/// No wait-time instrumentation (removed for performance — see module docs).
pub async fn with_acp_lock_async<T, R, F>(
    _name: &'static str,
    mutex: &TokioMutex<T>,
    operation: F,
) -> R
where
    F: FnOnce(&mut T) -> R,
{
    let mut guard = mutex.lock().await;
    operation(&mut guard)
}
