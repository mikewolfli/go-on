//! ACP Lock Helpers
//!
//! Poison-recovery lock wrappers for `std::sync::Mutex` and `tokio::sync::Mutex`.
//! Wait-time instrumentation removed (log-20260622-5: lock monitor stats were
//! never queried in production; the atomic ops and Instant::now() syscalls added
//! ~500ns per acquisition with zero downstream consumers).
//!
//! The dead `AcpLockMonitor` placeholder has been removed (log-20260623-7).

use std::sync::Mutex as StdMutex;

use tokio::sync::Mutex as TokioMutex;
use tracing::warn;

// ============================================================================
// Lock helper functions
// ============================================================================

/// Acquire a `std::sync::Mutex` with poison recovery.
///
/// Handles poisoned mutexes gracefully, recovering the state and continuing.
/// No wait-time instrumentation (removed for performance — see module docs).
pub fn with_acp_lock<T, R, F>(mutex: &StdMutex<T>, operation: F) -> R
where
    F: FnOnce(&mut T) -> R,
{
    match mutex.lock() {
        Ok(mut guard) => operation(&mut guard),
        Err(poisoned) => {
            warn!(
                target: "acp::locks",
                "ACP lock was poisoned; continuing with recovered state",
            );
            let mut guard = poisoned.into_inner();
            operation(&mut guard)
        }
    }
}

/// Acquire a `tokio::sync::Mutex` (async version).
///
/// No wait-time instrumentation (removed for performance — see module docs).
pub async fn with_acp_lock_async<T, R, F>(mutex: &TokioMutex<T>, operation: F) -> R
where
    F: FnOnce(&mut T) -> R,
{
    let mut guard = mutex.lock().await;
    operation(&mut guard)
}
