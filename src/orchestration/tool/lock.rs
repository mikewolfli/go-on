//! Tool lock manager – read/write locks per file path.
//!
//! Prevents concurrent `write_file` and `apply_patch` operations from
//! conflicting on the same file path. Read operations can proceed in
//! parallel; write operations are serialised per path.
//!
//! # Thread safety
//!
//! The lock table is protected by a [`std::sync::Mutex`]. Lock acquisition
//! is non-blocking — only [`try_acquire`] is exposed.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tracing::warn;

// ---------------------------------------------------------------------------
// LockMode
// ---------------------------------------------------------------------------

/// Mode for a tool lock on a file path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockMode {
    /// Shared / read lock — multiple readers allowed concurrently.
    Read,
    /// Exclusive / write lock — only one writer, no concurrent readers.
    Write,
}

// ---------------------------------------------------------------------------
// LockHandle
// ---------------------------------------------------------------------------

/// An RAII guard that releases the lock when dropped.
///
/// Created by [`ToolLockManager::try_acquire`] and automatically released on drop.
pub struct LockHandle {
    /// Path that was locked.
    pub path: String,
    /// Mode of the acquired lock.
    pub mode: LockMode,
    /// A sentinel clone of the manager so we can call release on drop.
    manager: ToolLockManager,
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
/// Uses non-blocking `try_acquire` only — no blocking `acquire()`.
/// Tools that fail to acquire a lock should retry via the TAO loop.
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
    fn try_acquire_inner(table: &mut LockTable, path: &str, mode: LockMode) -> bool {
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

    #[test]
    fn multiple_readers_can_acquire_concurrently() {
        let mgr = ToolLockManager::new();
        let h1 = mgr.try_acquire("/tmp/test1", LockMode::Read).unwrap();
        let h2 = mgr.try_acquire("/tmp/test1", LockMode::Read).unwrap();
        let h3 = mgr.try_acquire("/tmp/test1", LockMode::Read).unwrap();
        drop(h1);
        drop(h2);
        drop(h3);
    }

    #[test]
    fn writer_blocks_reader() {
        let mgr = ToolLockManager::new();
        let writer = mgr.try_acquire("/tmp/test2", LockMode::Write).unwrap();

        // Try-acquire a reader should fail while writer is held.
        assert!(mgr.try_acquire("/tmp/test2", LockMode::Read).is_none());

        drop(writer);

        // Now reader should succeed.
        assert!(mgr.try_acquire("/tmp/test2", LockMode::Read).is_some());
    }

    #[test]
    fn reader_blocks_writer() {
        let mgr = ToolLockManager::new();
        let reader = mgr.try_acquire("/tmp/test3", LockMode::Read).unwrap();

        assert!(mgr.try_acquire("/tmp/test3", LockMode::Write).is_none());

        drop(reader);
        assert!(mgr.try_acquire("/tmp/test3", LockMode::Write).is_some());
    }

    #[test]
    fn write_blocks_write() {
        let mgr = ToolLockManager::new();
        let w1 = mgr.try_acquire("/tmp/test4", LockMode::Write).unwrap();
        assert!(mgr.try_acquire("/tmp/test4", LockMode::Write).is_none());
        drop(w1);
        assert!(mgr.try_acquire("/tmp/test4", LockMode::Write).is_some());
    }

    #[test]
    fn different_paths_no_conflict() {
        let mgr = ToolLockManager::new();
        let w1 = mgr.try_acquire("/tmp/a", LockMode::Write).unwrap();
        let w2 = mgr.try_acquire("/tmp/b", LockMode::Write).unwrap();
        drop(w1);
        drop(w2);
    }

    #[test]
    fn writer_waits_until_all_readers_released() {
        let mgr = ToolLockManager::new();

        // Hold a read lock.
        let r1 = mgr.try_acquire("/tmp/test6", LockMode::Read).unwrap();

        // Writer should NOT be able to get the lock while reader holds it.
        assert!(mgr.try_acquire("/tmp/test6", LockMode::Write).is_none());

        // Release reader — writer should now succeed.
        drop(r1);
        assert!(mgr.try_acquire("/tmp/test6", LockMode::Write).is_some());
    }

    #[test]
    fn release_cleans_up_empty_entries() {
        let mgr = ToolLockManager::new();

        {
            let r = mgr.try_acquire("/tmp/cleanup", LockMode::Read).unwrap();
            drop(r);
        }

        // After release, the entry should be gone from the table.
        let table = mgr.inner.lock().expect("mutex poisoned");
        assert!(!table.locks.contains_key("/tmp/cleanup"));
    }

    #[test]
    fn try_acquire_returns_none_when_blocked() {
        let mgr = ToolLockManager::new();
        let _w = mgr.try_acquire("/tmp/try", LockMode::Write).unwrap();

        let result = mgr.try_acquire("/tmp/try", LockMode::Read);
        assert!(result.is_none(), "read should block while write held");

        let result = mgr.try_acquire("/tmp/try", LockMode::Write);
        assert!(result.is_none(), "write should block while write held");
    }

    #[test]
    fn try_acquire_blocked_by_writer() {
        let mgr = ToolLockManager::new();
        let handle = mgr
            .try_acquire("/tmp/real", LockMode::Write)
            .expect("should acquire");

        // While handle is held, try_acquire should fail.
        assert!(mgr.try_acquire("/tmp/real", LockMode::Write).is_none());

        drop(handle);

        // After release, try_acquire should succeed.
        assert!(mgr.try_acquire("/tmp/real", LockMode::Write).is_some());
    }
}
