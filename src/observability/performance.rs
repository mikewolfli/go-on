//! Performance optimization utilities
//!
//! This module provides performance optimization tools including caching strategies,
//! memory management, and performance profiling.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tracing::info;

#[cfg(target_os = "linux")]
use std::fs;

/// Performance metrics
#[derive(Debug, Clone)]
pub struct PerformanceMetrics {
    /// Total operations
    pub total_ops: u64,
    /// Successful operations
    pub successful_ops: u64,
    /// Failed operations
    pub failed_ops: u64,
    /// Average latency in milliseconds
    pub avg_latency_ms: f64,
    /// P95 latency in milliseconds
    pub p95_latency_ms: f64,
    /// P99 latency in milliseconds
    pub p99_latency_ms: f64,
    // Memory usage in bytes
    pub memory_usage_bytes: u64,
    /// Cache hit rate (filled by AcpServer::get_status from real
    /// vector/summary counters; PerformanceMonitor owns no cache stats).
    pub cache_hit_rate: f64,
    /// CPU usage percentage
    pub cpu_usage_percent: f64,
}

impl Default for PerformanceMetrics {
    fn default() -> Self {
        Self {
            total_ops: 0,
            successful_ops: 0,
            failed_ops: 0,
            avg_latency_ms: 0.0,
            p95_latency_ms: 0.0,
            p99_latency_ms: 0.0,
            memory_usage_bytes: 0,
            cache_hit_rate: 0.0,
            cpu_usage_percent: 0.0,
        }
    }
}

/// Performance monitor
pub struct PerformanceMonitor {
    /// Operation latencies for percentile calculation
    latencies: VecDeque<f64>,
    /// Maximum latencies to keep for percentile calculation
    max_latencies: usize,
    /// Total operations counter
    total_ops: AtomicU64,
    /// Successful operations counter
    successful_ops: AtomicU64,
    /// Failed operations counter
    failed_ops: AtomicU64,
}

impl PerformanceMonitor {
    /// Create a new performance monitor
    pub fn new(max_latencies: usize) -> Self {
        Self {
            latencies: VecDeque::with_capacity(max_latencies),
            max_latencies,
            total_ops: AtomicU64::new(0),
            successful_ops: AtomicU64::new(0),
            failed_ops: AtomicU64::new(0),
        }
    }

    /// Record an operation
    pub fn record_operation(&mut self, success: bool, latency_ms: f64) {
        self.total_ops.fetch_add(1, Ordering::Relaxed);

        if success {
            self.successful_ops.fetch_add(1, Ordering::Relaxed);
        } else {
            self.failed_ops.fetch_add(1, Ordering::Relaxed);
        }

        // Record latency for percentile calculation
        if self.latencies.len() >= self.max_latencies {
            self.latencies.pop_front();
        }
        self.latencies.push_back(latency_ms);
    }

    /// Get current metrics
    pub fn get_metrics(&self) -> PerformanceMetrics {
        let total_ops = self.total_ops.load(Ordering::Relaxed);
        let successful_ops = self.successful_ops.load(Ordering::Relaxed);
        let failed_ops = self.failed_ops.load(Ordering::Relaxed);

        // Calculate average latency
        let avg_latency = if !self.latencies.is_empty() {
            self.latencies.iter().sum::<f64>() / self.latencies.len() as f64
        } else {
            0.0
        };

        // Calculate percentiles (partial selection O(n), no full sort needed).
        let mut sorted_latencies: Vec<f64> = self.latencies.iter().copied().collect();

        let p95_latency = if !sorted_latencies.is_empty() {
            let index = (sorted_latencies.len() as f64 * 0.95).floor() as usize;
            let index = index.min(sorted_latencies.len() - 1);
            sorted_latencies.select_nth_unstable_by(index, |a, b| {
                a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
            });
            sorted_latencies[index]
        } else {
            0.0
        };

        let p99_latency = if !sorted_latencies.is_empty() {
            let index = (sorted_latencies.len() as f64 * 0.99).floor() as usize;
            let index = index.min(sorted_latencies.len() - 1);
            sorted_latencies.select_nth_unstable_by(index, |a, b| {
                a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
            });
            sorted_latencies[index]
        } else {
            0.0
        };

        // Calculate cache hit rate
        let cache_hit_rate = 0.0; // filled by AcpServer::get_status

        // Get memory + CPU usage through a short-TTL cache instead of reading
        // /proc/self/status and /proc/stat on every call.
        let MemCpuSnapshot {
            memory_usage_bytes: memory_usage,
            cpu_usage_percent: cpu_usage,
        } = cached_memory_and_cpu();

        PerformanceMetrics {
            total_ops,
            successful_ops,
            failed_ops,
            avg_latency_ms: avg_latency,
            p95_latency_ms: p95_latency,
            p99_latency_ms: p99_latency,
            memory_usage_bytes: memory_usage,
            cache_hit_rate,
            cpu_usage_percent: cpu_usage,
        }
    }
}

/// Snapshot of the process memory + CPU values, refreshed at most every
/// [`MEM_CPU_CACHE_TTL`].
#[derive(Debug, Clone, Copy)]
struct MemCpuSnapshot {
    memory_usage_bytes: u64,
    cpu_usage_percent: f64,
}

/// Memory + CPU reads hit `/proc` on Linux (or spawn `ps` on macOS) and run on
/// every /health, /metrics, metrics.get, governance.status, … request — cache
/// them briefly so hot paths stay off the filesystem.
const MEM_CPU_CACHE_TTL: Duration = Duration::from_secs(5);

fn cached_memory_and_cpu() -> MemCpuSnapshot {
    static MEM_CPU_CACHE: OnceLock<Mutex<Option<(Instant, MemCpuSnapshot)>>> = OnceLock::new();
    let cache = MEM_CPU_CACHE.get_or_init(|| Mutex::new(None));

    let now = Instant::now();
    if let Ok(guard) = cache.lock() {
        if let Some((fetched_at, snapshot)) = guard.as_ref() {
            if now.duration_since(*fetched_at) < MEM_CPU_CACHE_TTL {
                return *snapshot;
            }
        }
    }

    let snapshot = MemCpuSnapshot {
        memory_usage_bytes: get_memory_usage(),
        cpu_usage_percent: get_cpu_usage(),
    };
    if let Ok(mut guard) = cache.lock() {
        *guard = Some((now, snapshot));
    }
    snapshot
}

/// Get memory usage in bytes for the current process.
///
/// Falls back to `0` when platform-specific APIs are unavailable.
///
/// ## Windows dependency
/// Get current process resident memory in bytes.
///
/// Platform-specific implementations:
/// - Windows: uses `K32GetProcessMemoryInfo` via `windows-sys`.
/// - Linux: reads VmRSS from `/proc/self/status`.
/// - macOS: parses output of `ps -o rss=`.
/// - Other: returns 0 (unsupported platform).
#[cfg(target_os = "windows")]
pub(crate) fn get_memory_usage() -> u64 {
    use std::mem::{size_of, zeroed};

    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    // SAFETY:
    //
    // `GetCurrentProcess()` returns a pseudo-handle that is always valid, never
    // null, and does not need to be closed. It refers to the calling process and
    // is valid from any thread.
    //
    // `PROCESS_MEMORY_COUNTERS` is a Win32 POD struct. Zero-initializing via
    // `zeroed()` and then setting `cb` to `size_of::<PROCESS_MEMORY_COUNTERS>()`
    // is the documented Win32 pattern. The `cb` field must be set before calling
    // into the API; the kernel uses it to know the struct version/size.
    //
    // `K32GetProcessMemoryInfo(handle, &mut counters, cb)` writes the process
    // memory counters into `counters` through a raw pointer. The `cb` parameter
    // ensures the kernel writes at most `size_of::<PROCESS_MEMORY_COUNTERS>`
    // bytes, preventing buffer overflow.
    //
    // This function is safe to call from any thread — it reads process-level
    // metrics and does not touch thread-local state.
    //
    // The return value is BOOL (nonzero = success). We check this before
    // reading `WorkingSetSize`. The `cfg(target_os = "windows")` gate ensures
    // this code only compiles on Windows, where these Win32 APIs are available.
    unsafe {
        let handle = GetCurrentProcess();
        let mut counters: PROCESS_MEMORY_COUNTERS = zeroed();
        counters.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;

        let ok = K32GetProcessMemoryInfo(
            handle,
            &mut counters as *mut PROCESS_MEMORY_COUNTERS,
            size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        );
        if ok != 0 {
            counters.WorkingSetSize as u64
        } else {
            0
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn get_memory_usage() -> u64 {
    // Read VmRSS from /proc for stable process-resident memory on Linux.
    let Ok(status) = fs::read_to_string("/proc/self/status") else {
        return 0;
    };

    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb = rest
                .split_whitespace()
                .find_map(|token| token.parse::<u64>().ok())
                .unwrap_or(0);
            return kb.saturating_mul(1024);
        }
    }

    0
}

#[cfg(target_os = "macos")]
pub(crate) fn get_memory_usage() -> u64 {
    // Read RSS from `ps -o rss=` which works on macOS without extra dependencies.
    // The output is in KB, so multiply by 1024 to get bytes.
    if let Ok(output) = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
    {
        if let Ok(rss_str) = String::from_utf8(output.stdout) {
            if let Ok(kb) = rss_str.trim().parse::<u64>() {
                return kb.saturating_mul(1024);
            }
        }
    }
    0
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
pub(crate) fn get_memory_usage() -> u64 {
    0
}

// ── O8: CPU usage (percent) ────────────────────────────────────────────────
//
// Implementation guidance:
//   Linux:   Read /proc/stat for delta in 'cpu' line (user+nice+system+idle).
//            Compute utilisation as 100 * (total_delta - idle_delta) / total_delta.
//   macOS:   Use `host_cpu_load_info()` from libc via `mach_host_self()`.
//   Windows: Use `GetSystemTimes()` from kernel32.
//
// When none of the above is available, returns 0.0.

/// Returns the current overall CPU usage percentage (0.0 … 100.0).
///
/// On Linux this reads `/proc/stat` and performs a delta-based calculation
/// similar to `top(1)`.  On other platforms a best-effort approach is used;
/// returns 0.0 when no platform-specific implementation exists.
#[cfg(target_os = "linux")]
fn get_cpu_usage() -> f64 {
    linux_cpu_usage()
}

#[cfg(target_os = "macos")]
fn get_cpu_usage() -> f64 {
    macos_cpu_usage()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn get_cpu_usage() -> f64 {
    0.0
}

#[cfg(target_os = "linux")]
fn linux_cpu_usage() -> f64 {
    use std::fs;
    use std::sync::Mutex;
    static PREV: Mutex<Option<(u64, u64)>> = Mutex::new(None);

    let Ok(content) = fs::read_to_string("/proc/stat") else {
        return 0.0;
    };
    let Some(cpu_line) = content.lines().find(|l| l.starts_with("cpu ")) else {
        return 0.0;
    };
    let fields: Vec<u64> = cpu_line
        .split_whitespace()
        .skip(1)
        .filter_map(|s| s.parse::<u64>().ok())
        .collect();
    if fields.len() < 4 {
        return 0.0;
    }
    let total: u64 = fields[0] + fields[1] + fields[2] + fields[3];
    let idle = fields[3];
    let mut prev = PREV.lock().unwrap();
    if let Some((prev_total, prev_idle)) = *prev {
        let total_delta = total.saturating_sub(prev_total);
        let idle_delta = idle.saturating_sub(prev_idle);
        *prev = Some((total, idle));
        if total_delta > 0 {
            return 100.0 * (total_delta.saturating_sub(idle_delta)) as f64 / total_delta as f64;
        }
    } else {
        *prev = Some((total, idle));
    }
    0.0
}

#[cfg(target_os = "macos")]
fn macos_cpu_usage() -> f64 {
    if let Ok(output) = std::process::Command::new("ps")
        .args(["-A", "-o", "%cpu="])
        .output()
    {
        if let Ok(stdout) = String::from_utf8(output.stdout) {
            let total: f64 = stdout
                .lines()
                .filter_map(|l| l.trim().parse::<f64>().ok())
                .sum();
            let ncpus = std::thread::available_parallelism()
                .map(|n| n.get() as f64)
                .unwrap_or(1.0);
            return (total / ncpus).min(100.0);
        }
    }
    0.0
}

/// Performance optimization utilities
pub mod utils {
    use super::*;
    use std::future::Future;

    /// Measure execution time of a function
    pub fn measure_time<F, R>(f: F) -> (R, Duration)
    where
        F: FnOnce() -> R,
    {
        let start = Instant::now();
        let result = f();
        let duration = start.elapsed();
        (result, duration)
    }

    /// Measure execution time of an async function
    pub async fn measure_time_async<F, Fut, R>(f: F) -> (R, Duration)
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = R>,
    {
        let start = Instant::now();
        let result = f().await;
        let duration = start.elapsed();
        (result, duration)
    }
}

static PERFORMANCE_MONITOR: OnceLock<Arc<Mutex<PerformanceMonitor>>> = OnceLock::new();

pub fn record_global_operation(success: bool, latency_ms: f64) {
    // Lazily initialize on first use so the metrics pipeline is active even
    // when startup wiring never called an explicit initializer.
    let monitor = PERFORMANCE_MONITOR.get_or_init(|| {
        let m = Arc::new(Mutex::new(PerformanceMonitor::new(1000)));
        info!("Performance monitoring initialized (lazy on first record)");
        m
    });
    let mut guard = monitor.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("performance monitor lock poisoned, recovering");
        poisoned.into_inner()
    });
    guard.record_operation(success, latency_ms);
}

pub fn global_metrics_snapshot() -> Option<PerformanceMetrics> {
    let monitor = PERFORMANCE_MONITOR.get_or_init(|| {
        let m = Arc::new(Mutex::new(PerformanceMonitor::new(1000)));
        info!("Performance monitoring initialized (lazy snapshot)");
        m
    });
    Some(
        monitor
            .lock()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned, recovering");
                poisoned.into_inner()
            })
            .get_metrics(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn performance_measure_time_returns_duration() {
        let (result, duration) = utils::measure_time(|| {
            std::thread::sleep(Duration::from_millis(1));
            42u32
        });
        assert_eq!(result, 42);
        assert!(duration.as_nanos() > 0);
    }

    #[test]
    fn performance_monitor_records_latency_and_metrics() {
        let mut monitor = PerformanceMonitor::new(100);
        monitor.record_operation(true, 10.0);
        monitor.record_operation(true, 20.0);
        monitor.record_operation(false, 30.0);

        let metrics = monitor.get_metrics();
        assert_eq!(metrics.total_ops, 3);
        assert_eq!(metrics.successful_ops, 2);
        assert_eq!(metrics.failed_ops, 1);
        assert!((metrics.avg_latency_ms - 20.0).abs() < 0.01);
    }
}
