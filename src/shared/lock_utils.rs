//! Lock utility helpers for recovering from poisoned mutexes.
//!
//! Provides a convenience macro [`lock_or_recover`] that reduces the
//! ~5-line `lock().unwrap_or_else(|poisoned| { ... })` pattern to a
//! single line, eliminating ~40+ near-identical occurrences across
//! the codebase.

/// Acquire a lock and recover from a poisoned state.
///
/// # Usage
///
/// ```ignore
/// let guard = lock_or_recover!(some_mutex);
/// // equivalent to:
/// // let guard = some_mutex.lock().unwrap_or_else(|poisoned| {
/// //     tracing::warn!("lock poisoned, recovering");
/// //     poisoned.into_inner()
/// // });
/// ```
///
/// A custom warning message can be provided:
///
/// ```ignore
/// let guard = lock_or_recover!(some_mutex, "custom context message");
/// ```
#[macro_export]
macro_rules! lock_or_recover {
    ($lock:expr) => {
        $lock.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        })
    };
    ($lock:expr, $msg:expr) => {
        $lock.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned — {}, recovering", $msg);
            poisoned.into_inner()
        })
    };
}

/// Acquire a read lock on an `RwLock` and recover from a poisoned state.
#[macro_export]
macro_rules! read_or_recover {
    ($lock:expr) => {
        $lock.read().unwrap_or_else(|poisoned| {
            tracing::warn!("rwlock read poisoned, recovering");
            poisoned.into_inner()
        })
    };
    ($lock:expr, $msg:expr) => {
        $lock.read().unwrap_or_else(|poisoned| {
            tracing::warn!("rwlock read poisoned — {}, recovering", $msg);
            poisoned.into_inner()
        })
    };
}

/// Acquire a write lock on an `RwLock` and recover from a poisoned state.
#[macro_export]
macro_rules! write_or_recover {
    ($lock:expr) => {
        $lock.write().unwrap_or_else(|poisoned| {
            tracing::warn!("rwlock write poisoned, recovering");
            poisoned.into_inner()
        })
    };
    ($lock:expr, $msg:expr) => {
        $lock.write().unwrap_or_else(|poisoned| {
            tracing::warn!("rwlock write poisoned — {}, recovering", $msg);
            poisoned.into_inner()
        })
    };
}
