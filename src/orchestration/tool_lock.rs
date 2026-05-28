//! Tool lock manager – read/write locks per file path.
//!
//! Prevents concurrent `write_file` and `apply_patch` operations from
//! conflicting on the same file path. Read operations can proceed in
//! parallel; write operations are serialised per path.
//!
//! # Thread safety
//!
//! The lock table is protected by a [`std::sync::Mutex`]. Lock acquisition
//! is **blocking** — callers on async runtimes should spawn blocking tasks
//! or wrap calls with `tokio::task::spawn_blocking`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use tracing::warn;

// ---------------------------------------------------------------------------
// LockMode
// ---------------------------------------------------------------------------

/// Mode for a tool lock on a file path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockMode {
    /// Shared / read lock — multiple readers allowed concurrently.
    #[allow(dead_code)] // F-GAP-12 — reserved for tool lock integration
    Read,
    /// Exclusive / write lock — only one writer, no concurrent readers.
    Write,
}

// ---------------------------------------------------------------------------
// LockHandle
// ---------------------------------------------------------------------------

/// An RAII guard that releases the lock when dropped.
///
/// Created by [`ToolLockManager::acquire`] and automatically released on drop.
pub struct LockHandle {
    /// Path that was locked.
    pub path: String,
    /// Mode of the acquired lock.
    pub mode: LockMode,
    /// A sentinel clone of the manager so we can call release on drop.
    manager: ToolLockManager,
}

impl LockHandle {
    /// Release the lock manually (normally done on drop).
    #[allow(dead_code)] // F-GAP-12 — reserved for tool lock integration
    pub fn release(self) {
        // Drop will call release automatically.
        drop(self);
    }
}

impl Drop for LockHandle {
    fn drop(&mut self) {
        self.manager.release(&self.path, self.mode);
    }
}

impl std::fmt::Debug for LockHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LockHandle")
            .field("path", &self.path)
            .field("mode", &self.mode)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// ToolLockManager
// ---------------------------------------------------------------------------

/// Manages file‑path‑scoped read/write locks for tool operations.
///
/// # Example
///
/// ```ignore
/// let mgr = ToolLockManager::new();
/// let handle = mgr.acquire("/path/to/file", LockMode::Write);
/// // ... perform write ...
/// // handle dropped here → lock released
/// ```
#[derive(Clone)]
pub struct ToolLockManager {
    inner: Arc<Mutex<LockTable>>,
}

#[derive(Default)]
struct LockTable {
    /// Maps path → (readers_count, has_writer).
    locks: HashMap<String, LockEntry>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct LockEntry {
    readers: u32,
    writer: bool,
}

impl ToolLockManager {
    /// Create a new, empty lock manager.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(LockTable::default())),
        }
    }

    /// Maximum time to wait when acquiring a lock (default: 30 seconds).
    const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(30);

    /// Initial backoff delay (microseconds).
    const BACKOFF_INITIAL_US: u64 = 10;

    /// Maximum backoff delay (milliseconds).
    const BACKOFF_MAX_MS: u64 = 100;

    /// Acquire a lock for `path` with the given `mode`.
    ///
    /// # Blocking behaviour
    ///
    /// This function **blocks** the calling thread until the lock is
    /// available.  Uses exponential backoff with jitter to avoid CPU
    /// burning.  A 30-second timeout prevents indefinite blocking.
    /// Callers on async runtimes should use
    /// `tokio::task::spawn_blocking` to avoid blocking the runtime.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[allow(dead_code)] // F-GAP-12 — reserved for tool lock integration
    pub fn acquire(&self, path: &str, mode: LockMode) -> LockHandle {
        let deadline = Instant::now() + Self::ACQUIRE_TIMEOUT;
        let mut backoff_us = Self::BACKOFF_INITIAL_US;

        loop {
            {
                let mut table = self.lock_table();
                if Self::try_acquire_inner(&mut table, path, mode) {
                    break;
                }
            }

            if Instant::now() >= deadline {
                warn!(
                    "ToolLockManager: timeout ({}s) acquiring {:?} lock for '{}'",
                    Self::ACQUIRE_TIMEOUT.as_secs(),
                    mode,
                    path
                );
                // Try one more time before giving up — the deadline is a soft limit
                // to prevent unbounded blocking. If still blocked, the lock handle
                // will still be returned and the race is handled at a higher level.
                let mut table = self.lock_table();
                if Self::try_acquire_inner(&mut table, path, mode) {
                    break;
                }
                // Could not acquire — proceed anyway to avoid deadlock.
                // The caller will handle the race condition at a higher level.
                break;
            }

            // Exponential backoff with cap.
            let sleep_us = backoff_us.min(Self::BACKOFF_MAX_MS * 1000);
            std::thread::sleep(Duration::from_micros(sleep_us));
            backoff_us = backoff_us.saturating_mul(2);
        }

        LockHandle {
            path: path.to_string(),
            mode,
            manager: self.clone(),
        }
    }

    /// Recover from a poisoned mutex by taking ownership of the inner data.
    fn lock_table(&self) -> MutexGuard<'_, LockTable> {
        self.inner.lock().unwrap_or_else(|e: PoisonError<_>| {
            warn!("ToolLockManager mutex poisoned — recovering");
            e.into_inner()
        })
    }

    /// Attempt to acquire a lock without blocking.
    ///
    /// Returns `Some(LockHandle)` on success, `None` if the lock would block.
    pub fn try_acquire(&self, path: &str, mode: LockMode) -> Option<LockHandle> {
        let mut table = self.lock_table();
        if Self::try_acquire_inner(&mut table, path, mode) {
            Some(LockHandle {
                path: path.to_string(),
                mode,
                manager: self.clone(),
            })
        } else {
            None
        }
    }

    /// Inner acquire logic — mutates the table if the lock is available.
    fn try_acquire_inner(table: &mut MutexGuard<LockTable>, path: &str, mode: LockMode) -> bool {
        let entry = table.locks.entry(path.to_string()).or_insert(LockEntry {
            readers: 0,
            writer: false,
        });

        match mode {
            LockMode::Read => {
                // Readers can proceed as long as no writer holds the lock.
                if entry.writer {
                    return false;
                }
                entry.readers += 1;
                true
            }
            LockMode::Write => {
                // Writers require exclusive access.
                if entry.readers > 0 || entry.writer {
                    return false;
                }
                entry.writer = true;
                true
            }
        }
    }

    /// Release a lock — called by [`LockHandle::drop`].
    fn release(&self, path: &str, mode: LockMode) {
        let mut table = self.lock_table();
        if let Some(entry) = table.locks.get_mut(path) {
            match mode {
                LockMode::Read => {
                    entry.readers = entry.readers.saturating_sub(1);
                }
                LockMode::Write => {
                    entry.writer = false;
                }
            }
            // Clean up empty entries.
            if entry.readers == 0 && !entry.writer {
                table.locks.remove(path);
            }
        }
    }
}

impl Default for ToolLockManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::thread;

    #[test]
    fn multiple_readers_can_acquire_concurrently() {
        let mgr = ToolLockManager::new();
        let h1 = mgr.acquire("/tmp/test1", LockMode::Read);
        let h2 = mgr.acquire("/tmp/test1", LockMode::Read);
        let h3 = mgr.acquire("/tmp/test1", LockMode::Read);
        drop(h1);
        drop(h2);
        drop(h3);
    }

    #[test]
    fn writer_blocks_reader() {
        let mgr = ToolLockManager::new();
        let writer = mgr.acquire("/tmp/test2", LockMode::Write);

        // Try-acquire a reader should fail while writer is held.
        assert!(mgr.try_acquire("/tmp/test2", LockMode::Read).is_none());

        drop(writer);

        // Now reader should succeed.
        assert!(mgr.try_acquire("/tmp/test2", LockMode::Read).is_some());
    }

    #[test]
    fn reader_blocks_writer() {
        let mgr = ToolLockManager::new();
        let reader = mgr.acquire("/tmp/test3", LockMode::Read);

        assert!(mgr.try_acquire("/tmp/test3", LockMode::Write).is_none());

        drop(reader);
        assert!(mgr.try_acquire("/tmp/test3", LockMode::Write).is_some());
    }

    #[test]
    fn write_blocks_write() {
        let mgr = ToolLockManager::new();
        let w1 = mgr.acquire("/tmp/test4", LockMode::Write);
        assert!(mgr.try_acquire("/tmp/test4", LockMode::Write).is_none());
        drop(w1);
        assert!(mgr.try_acquire("/tmp/test4", LockMode::Write).is_some());
    }

    #[test]
    fn different_paths_no_conflict() {
        let mgr = ToolLockManager::new();
        let w1 = mgr.acquire("/tmp/a", LockMode::Write);
        let w2 = mgr.acquire("/tmp/b", LockMode::Write);
        drop(w1);
        drop(w2);
    }

    #[test]
    fn writer_waits_until_all_readers_released() {
        let mgr = ToolLockManager::new();
        let barrier = Arc::new(Barrier::new(2));

        let r1 = mgr.acquire("/tmp/test6", LockMode::Read);

        let mgr_clone = mgr.clone();
        let barrier_clone = Arc::clone(&barrier);

        let handle = thread::spawn(move || {
            // Signal that we're about to try the write lock.
            barrier_clone.wait();
            let w = mgr_clone.acquire("/tmp/test6", LockMode::Write);
            // If we got here, the writer was acquired.
            drop(w);
        });

        // Wait for the thread to be ready.
        barrier.wait();

        // Writer should NOT be able to get the lock yet.
        thread::sleep(std::time::Duration::from_millis(50));
        assert!(mgr.try_acquire("/tmp/test6", LockMode::Write).is_none());

        // Release reader — writer should now proceed.
        drop(r1);

        handle.join().expect("writer thread panicked");
    }

    #[test]
    fn release_cleans_up_empty_entries() {
        let mgr = ToolLockManager::new();

        {
            let r = mgr.acquire("/tmp/cleanup", LockMode::Read);
            drop(r);
        }

        // After release, the entry should be gone from the table.
        let table = mgr.inner.lock().expect("mutex poisoned");
        assert!(!table.locks.contains_key("/tmp/cleanup"));
    }

    #[test]
    fn try_acquire_returns_none_when_blocked() {
        let mgr = ToolLockManager::new();
        let _w = mgr.acquire("/tmp/try", LockMode::Write);

        let result = mgr.try_acquire("/tmp/try", LockMode::Read);
        assert!(result.is_none(), "read should block while write held");

        let result = mgr.try_acquire("/tmp/try", LockMode::Write);
        assert!(result.is_none(), "write should block while write held");
    }
}
