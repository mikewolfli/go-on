//! Shared test utilities for go-on integration tests.
//!
//! Provides a cross-process file lock that prevents concurrent test
//! processes from interfering with shared resources (ports, databases).

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// A cross-process lock implemented via a lock file on disk.
///
/// Tests that bind to fixed ports or access shared databases should
/// acquire this lock before running. The lock file is automatically
/// cleaned up when the `CrossProcessLock` is dropped.
pub struct CrossProcessLock {
    lock_path: PathBuf,
    file: Option<fs::File>,
}

impl CrossProcessLock {
    /// Try to acquire the lock with a retry loop.
    ///
    /// Retries every 100ms up to `timeout_secs` seconds. Panics if
    /// the lock cannot be acquired within the timeout.
    pub fn new(name: &str, timeout_secs: u64) -> Self {
        let mut lock_path = std::env::temp_dir();
        lock_path.push(format!("go-on-test-{name}.lock"));

        let deadline = Instant::now() + Duration::from_secs(timeout_secs);
        loop {
            match fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&lock_path)
            {
                Ok(file) => {
                    return Self {
                        lock_path,
                        file: Some(file),
                    };
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if Instant::now() > deadline {
                        panic!(
                            "could not acquire lock '{}' within {timeout_secs}s \
                             (lock file: {})",
                            name,
                            lock_path.display()
                        );
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    panic!("unexpected error acquiring lock '{}': {e}", name);
                }
            }
        }
    }
}

impl Drop for CrossProcessLock {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.lock_path);
    }
}
