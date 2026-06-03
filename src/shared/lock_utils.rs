/// Unified lock guard utilities for consistent lock acquisition across the codebase.
///
/// These helpers provide consistent poisoning recovery and logging.
use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use tracing;

/// Acquire a std Mutex with poison recovery and logging.
///
/// NOTE: Reserved for future use. Once the codebase migrates to a consistent
/// lock-acquisition pattern, this helper will replace inline `match mtx.lock()`
/// blocks across all modules. Currently unused because different modules use
/// their own local patterns (`.unwrap_or_else(|poisoned| ...)`).
#[allow(dead_code)] // F-GAP-49 — reserved lock utilities feature
pub fn lock_guard<'a, T>(mtx: &'a Mutex<T>, name: &'a str) -> MutexGuard<'a, T> {
    match mtx.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!(target: "lock_utils", "Mutex '{name}' was poisoned – recovering");
            poisoned.into_inner()
        }
    }
}

/// Acquire a std RwLock for reading with poison recovery.
///
/// NOTE: Reserved — see `lock_guard` for rationale.
#[allow(dead_code)] // F-GAP-49 — reserved lock utilities feature
pub fn read_guard<'a, T>(lock: &'a RwLock<T>, name: &'a str) -> RwLockReadGuard<'a, T> {
    match lock.read() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!(target: "lock_utils", "RwLock '{name}' read was poisoned – recovering");
            poisoned.into_inner()
        }
    }
}

/// Acquire a std RwLock for writing with poison recovery.
///
/// NOTE: Reserved — see `lock_guard` for rationale.
#[allow(dead_code)] // F-GAP-49 — reserved lock utilities feature
pub fn write_guard<'a, T>(lock: &'a RwLock<T>, name: &'a str) -> RwLockWriteGuard<'a, T> {
    match lock.write() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!(target: "lock_utils", "RwLock '{name}' write was poisoned – recovering");
            poisoned.into_inner()
        }
    }
}

/// Try to acquire a std Mutex with a configurable number of retries.
///
/// NOTE: Reserved — see `lock_guard` for rationale. This variant also
/// provides retry logic for contended locks, which will be adopted once
/// the base `lock_guard` pattern is in use.
#[allow(dead_code)] // F-GAP-49 — reserved lock utilities feature
pub fn try_lock_guard<'a, T>(
    mtx: &'a Mutex<T>,
    name: &'a str,
    retries: u32,
    retry_delay_us: u64,
) -> Option<MutexGuard<'a, T>> {
    for i in 0..retries {
        match mtx.try_lock() {
            Ok(guard) => return Some(guard),
            Err(_) if i < retries - 1 => {
                std::thread::sleep(std::time::Duration::from_micros(retry_delay_us));
            }
            _ => {
                tracing::warn!(target: "lock_utils", "Mutex '{name}' try_lock failed after {retries} retries");
                return None;
            }
        }
    }
    None
}
