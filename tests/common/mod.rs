//! Shared test utilities for go-on integration tests.
//!
//! Provides a cross-process file lock that prevents concurrent test
//! processes from interfering with shared resources (ports, databases).

use std::fs;
use std::io::{Read, Write};
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
    ///
    /// If a stale lock file from a crashed process is detected (via PID
    /// aliveness check), it is cleaned up and acquisition retried.
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
                Ok(mut file) => {
                    // Write our PID to the lock file so other processes
                    // can detect stale locks from crashed owners.
                    let _ = write!(file, "{}", std::process::id());
                    let _ = file.flush();
                    return Self {
                        lock_path,
                        file: Some(file),
                    };
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // Try to read the PID from the stale lock file.
                    // If the owner process is gone, remove the stale file
                    // and retry acquisition.
                    if let Ok(mut f) = fs::File::open(&lock_path) {
                        let mut pid_str = String::new();
                        if f.read_to_string(&mut pid_str).is_ok() {
                            if let Ok(pid) = pid_str.trim().parse::<u32>() {
                                if !process_is_alive(pid) {
                                    tracing::warn!(
                                        "removing stale lock file from dead PID {}",
                                        pid
                                    );
                                    drop(f);
                                    let _ = fs::remove_file(&lock_path);
                                    continue; // retry acquisition
                                }
                            }
                        }
                    }

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

/// Check whether a process with the given PID is still alive.
/// Uses `/proc/{pid}` on Linux, `kill -0` on macOS/other Unix.
#[cfg(target_os = "linux")]
fn process_is_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(not(target_os = "linux"))]
fn process_is_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

impl Drop for CrossProcessLock {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.lock_path);
    }
}

/// Find an available TCP port on localhost.
pub fn find_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("find_free_port: bind failed");
    listener
        .local_addr()
        .expect("find_free_port: local_addr failed")
        .port()
}

/// Path to the compiled go-on binary.
pub fn binary_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("target/debug/go-on");
    if !path.exists() {
        path.set_file_name("go-on");
        let alt = std::env::current_dir().unwrap_or_default().join("go-on");
        if alt.exists() {
            return alt;
        }
        if cfg!(target_os = "windows") {
            path.set_extension("exe");
        }
    }
    path
}

/// Suite-level mutex for serializing test execution.
pub fn suite_mutex() -> &'static std::sync::Mutex<()> {
    static SUITE_MUTEX: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    SUITE_MUTEX.get_or_init(|| std::sync::Mutex::new(()))
}

/// Guard that drops a child process on scope exit.
pub struct ChildGuard(pub std::process::Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Ok(None) = self.0.try_wait() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
