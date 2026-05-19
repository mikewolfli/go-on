//! Performance optimization utilities
//!
//! This module provides performance optimization tools including caching strategies,
//! memory management, and performance profiling.

use std::collections::HashMap;
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
    /// Memory usage in bytes
    pub memory_usage_bytes: u64,
    /// Cache hit rate
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
    latencies: Vec<f64>,
    /// Maximum latencies to keep for percentile calculation
    max_latencies: usize,
    /// Total operations counter
    total_ops: AtomicU64,
    /// Successful operations counter
    successful_ops: AtomicU64,
    /// Failed operations counter
    failed_ops: AtomicU64,
    /// Cache hits counter
    cache_hits: AtomicU64,
    /// Cache misses counter
    cache_misses: AtomicU64,
}

impl PerformanceMonitor {
    /// Create a new performance monitor
    pub fn new(max_latencies: usize) -> Self {
        Self {
            latencies: Vec::with_capacity(max_latencies),
            max_latencies,
            total_ops: AtomicU64::new(0),
            successful_ops: AtomicU64::new(0),
            failed_ops: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
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
            self.latencies.remove(0);
        }
        self.latencies.push(latency_ms);
    }

    /// Record a cache hit
    pub fn record_cache_hit(&self) {
        self.cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a cache miss
    pub fn record_cache_miss(&self) {
        self.cache_misses.fetch_add(1, Ordering::Relaxed);
    }

    /// Get current metrics
    pub fn get_metrics(&self) -> PerformanceMetrics {
        let total_ops = self.total_ops.load(Ordering::Relaxed);
        let successful_ops = self.successful_ops.load(Ordering::Relaxed);
        let failed_ops = self.failed_ops.load(Ordering::Relaxed);
        let cache_hits = self.cache_hits.load(Ordering::Relaxed);
        let cache_misses = self.cache_misses.load(Ordering::Relaxed);

        // Calculate average latency
        let avg_latency = if !self.latencies.is_empty() {
            self.latencies.iter().sum::<f64>() / self.latencies.len() as f64
        } else {
            0.0
        };

        // Calculate percentiles
        let mut sorted_latencies = self.latencies.clone();
        sorted_latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let p95_latency = if !sorted_latencies.is_empty() {
            let index = (sorted_latencies.len() as f64 * 0.95).floor() as usize;
            sorted_latencies[index.min(sorted_latencies.len() - 1)]
        } else {
            0.0
        };

        let p99_latency = if !sorted_latencies.is_empty() {
            let index = (sorted_latencies.len() as f64 * 0.99).floor() as usize;
            sorted_latencies[index.min(sorted_latencies.len() - 1)]
        } else {
            0.0
        };

        // Calculate cache hit rate
        let total_cache_ops = cache_hits + cache_misses;
        let cache_hit_rate = if total_cache_ops > 0 {
            cache_hits as f64 / total_cache_ops as f64
        } else {
            0.0
        };

        // Get memory usage (simplified - in real implementation, use system APIs)
        let memory_usage = get_memory_usage();

        PerformanceMetrics {
            total_ops,
            successful_ops,
            failed_ops,
            avg_latency_ms: avg_latency,
            p95_latency_ms: p95_latency,
            p99_latency_ms: p99_latency,
            memory_usage_bytes: memory_usage,
            cache_hit_rate,
            cpu_usage_percent: 0.0, // Would require system-specific APIs
        }
    }

    /// Get performance recommendations
    pub fn get_recommendations(&self) -> Vec<PerformanceRecommendation> {
        let metrics = self.get_metrics();
        let mut recommendations = Vec::new();

        // Latency recommendations
        if metrics.p95_latency_ms > 5000.0 {
            recommendations.push(PerformanceRecommendation {
                category: RecommendationCategory::Latency,
                severity: RecommendationSeverity::High,
                message: "P95 latency is very high (>5s). Consider optimizing slow operations."
                    .to_string(),
                suggestion: "Review slowest operations and consider caching or optimization."
                    .to_string(),
            });
        } else if metrics.p95_latency_ms > 2000.0 {
            recommendations.push(PerformanceRecommendation {
                category: RecommendationCategory::Latency,
                severity: RecommendationSeverity::Medium,
                message: "P95 latency is high (>2s).".to_string(),
                suggestion: "Monitor and optimize frequently called slow operations.".to_string(),
            });
        }

        // Cache recommendations
        if metrics.cache_hit_rate < 0.3 && metrics.total_ops > 100 {
            recommendations.push(PerformanceRecommendation {
                category: RecommendationCategory::Cache,
                severity: RecommendationSeverity::Medium,
                message: format!(
                    "Cache hit rate is low ({:.1}%).",
                    metrics.cache_hit_rate * 100.0
                ),
                suggestion: "Consider increasing cache TTL or caching more operations.".to_string(),
            });
        }

        // Memory recommendations
        if metrics.memory_usage_bytes > 500 * 1024 * 1024 {
            // 500MB
            recommendations.push(PerformanceRecommendation {
                category: RecommendationCategory::Memory,
                severity: RecommendationSeverity::Medium,
                message: format!(
                    "Memory usage is high ({:.1} MB).",
                    metrics.memory_usage_bytes as f64 / 1024.0 / 1024.0
                ),
                suggestion: "Consider implementing memory limits or cleanup strategies."
                    .to_string(),
            });
        }

        // Success rate recommendations
        let success_rate = if metrics.total_ops > 0 {
            metrics.successful_ops as f64 / metrics.total_ops as f64
        } else {
            1.0
        };

        if success_rate < 0.9 {
            recommendations.push(PerformanceRecommendation {
                category: RecommendationCategory::Reliability,
                severity: RecommendationSeverity::High,
                message: format!("Success rate is low ({:.1}%).", success_rate * 100.0),
                suggestion: "Investigate and fix common failure modes.".to_string(),
            });
        }

        recommendations
    }
}

/// Performance recommendation
#[derive(Debug, Clone)]
pub struct PerformanceRecommendation {
    /// Recommendation category
    pub category: RecommendationCategory,
    /// Recommendation severity
    pub severity: RecommendationSeverity,
    /// Recommendation message
    pub message: String,
    /// Suggested action
    pub suggestion: String,
}

/// Recommendation category
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecommendationCategory {
    /// Latency optimization
    Latency,
    /// Cache optimization
    Cache,
    /// Memory optimization
    Memory,
    /// CPU optimization
    Cpu,
    /// Reliability improvement
    Reliability,
}

/// Recommendation severity
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecommendationSeverity {
    /// High severity - should be addressed immediately
    High,
    /// Medium severity - should be addressed soon
    Medium,
    /// Low severity - can be addressed when convenient
    Low,
}

/// Smart cache with adaptive TTL
pub struct AdaptiveCache<K, V> {
    /// Cache storage
    storage: HashMap<K, (V, Instant, u32)>,
    /// Default TTL
    default_ttl: Duration,
    /// Maximum cache size
    max_size: usize,
    /// Hit counters for adaptive TTL
    hit_counts: HashMap<K, u32>,
}

impl<K, V> AdaptiveCache<K, V>
where
    K: std::hash::Hash + Eq + Clone,
    V: Clone,
{
    /// Create a new adaptive cache
    pub fn new(default_ttl: Duration, max_size: usize) -> Self {
        Self {
            storage: HashMap::with_capacity(max_size / 2),
            default_ttl,
            max_size,
            hit_counts: HashMap::new(),
        }
    }

    /// Get a value from cache
    pub fn get(&mut self, key: &K) -> Option<V> {
        if let Some((value, inserted_at, hit_count)) = self.storage.get(key) {
            // Check if entry has expired
            if inserted_at.elapsed() < self.default_ttl {
                // Update hit count for adaptive TTL
                let new_hit_count = hit_count + 1;
                self.hit_counts.insert(key.clone(), new_hit_count);

                // Return cloned value
                return Some(value.clone());
            } else {
                // Remove expired entry
                self.storage.remove(key);
                self.hit_counts.remove(key);
            }
        }
        None
    }

    /// Insert a value into cache
    pub fn insert(&mut self, key: K, value: V) {
        // Check if we need to evict entries
        if self.storage.len() >= self.max_size {
            self.evict_oldest();
        }

        // Get adaptive TTL based on historical hit count
        let hit_count = self.hit_counts.get(&key).copied().unwrap_or(0);
        let _ttl = self.calculate_adaptive_ttl(hit_count);

        // Store with creation time and hit count
        self.storage
            .insert(key.clone(), (value, Instant::now(), hit_count));

        // Reset hit count for new entry
        self.hit_counts.insert(key, 0);
    }

    /// Calculate adaptive TTL based on hit count
    fn calculate_adaptive_ttl(&self, hit_count: u32) -> Duration {
        // Popular items get longer TTL
        match hit_count {
            0..=2 => self.default_ttl,       // New or rarely accessed
            3..=10 => self.default_ttl * 2,  // Moderately accessed
            11..=50 => self.default_ttl * 4, // Frequently accessed
            _ => self.default_ttl * 8,       // Very frequently accessed
        }
    }

    /// Evict oldest entries
    fn evict_oldest(&mut self) {
        if self.storage.is_empty() {
            return;
        }

        // Find oldest entry
        let oldest_key = self
            .storage
            .iter()
            .min_by_key(|(_, (_, inserted_at, _))| inserted_at)
            .map(|(key, _)| key.clone());

        if let Some(key) = oldest_key {
            self.storage.remove(&key);
            self.hit_counts.remove(&key);
        }
    }

    /// Clear cache
    pub fn clear(&mut self) {
        self.storage.clear();
        self.hit_counts.clear();
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        let total_size = self.storage.len();
        let total_hits: u32 = self.hit_counts.values().sum();
        let avg_hits = if !self.hit_counts.is_empty() {
            total_hits as f64 / self.hit_counts.len() as f64
        } else {
            0.0
        };

        CacheStats {
            total_size,
            max_size: self.max_size,
            total_hits,
            avg_hits_per_entry: avg_hits,
            utilization: total_size as f64 / self.max_size as f64,
        }
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// Current cache size
    pub total_size: usize,
    /// Maximum cache size
    pub max_size: usize,
    /// Total hits across all entries
    pub total_hits: u32,
    /// Average hits per entry
    pub avg_hits_per_entry: f64,
    /// Cache utilization (0.0 to 1.0)
    pub utilization: f64,
}

/// Get memory usage in bytes for the current process.
///
/// Falls back to `0` when platform-specific APIs are unavailable.
///
/// ## Windows dependency
/// This function uses the `windows-sys` crate (version 0.59) through the
/// `Win32_System_ProcessStatus` and `Win32_System_Threading` features.
/// The dependency is declared in `Cargo.toml` under
/// `[target.'cfg(target_os = "windows")'.dependencies]` so it is only
/// compiled on Windows targets.
#[cfg(target_os = "windows")]
fn get_memory_usage() -> u64 {
    use std::mem::{size_of, zeroed};

    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    // SAFETY: `GetCurrentProcess` returns a pseudo-handle (always valid, never null).
    // `PROCESS_MEMORY_COUNTERS` is a Win32 POD struct initialized with `zeroed()`
    // (which sets cb=0 as per Win32 convention), then cb is set to the struct size.
    // `K32GetProcessMemoryInfo` writes into `counters` via raw pointer — the size
    // parameter ensures no buffer overflow. The function is safe to call from any
    // thread (it reads process-level metrics, not thread-local state).
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
fn get_memory_usage() -> u64 {
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
fn get_memory_usage() -> u64 {
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
fn get_memory_usage() -> u64 {
    0
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

    /// Execute function with timeout
    pub fn with_timeout<F, R>(f: F, timeout: Duration) -> Result<R, TimeoutError>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let (result, receiver) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let res = f();
            let _ = result.send(res);
        });

        match receiver.recv_timeout(timeout) {
            Ok(res) => Ok(res),
            Err(_) => Err(TimeoutError),
        }
    }

    /// Batch operations for better performance
    pub fn batch_operations<F, T, R>(items: Vec<T>, batch_size: usize, operation: F) -> Vec<R>
    where
        F: Fn(Vec<T>) -> Vec<R> + Send + Sync,
        T: Send + Sync + Clone,
        R: Send + Sync,
    {
        let mut results = Vec::new();

        for chunk in items.chunks(batch_size) {
            let chunk_vec = chunk.to_vec();
            let chunk_results = operation(chunk_vec);
            results.extend(chunk_results);
        }

        results
    }
}

/// Timeout error
#[derive(Debug, Clone)]
pub struct TimeoutError;

impl std::fmt::Display for TimeoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "operation timed out")
    }
}

impl std::error::Error for TimeoutError {}

/// Initialize performance monitoring
pub fn init_performance_monitoring() -> Arc<Mutex<PerformanceMonitor>> {
    let monitor = PERFORMANCE_MONITOR
        .get_or_init(|| Arc::new(Mutex::new(PerformanceMonitor::new(1000))))
        .clone();
    info!("Performance monitoring initialized");
    monitor
}

static PERFORMANCE_MONITOR: OnceLock<Arc<Mutex<PerformanceMonitor>>> = OnceLock::new();

pub fn record_global_operation(success: bool, latency_ms: f64) {
    if let Some(monitor) = PERFORMANCE_MONITOR.get() {
        if let Ok(mut guard) = monitor.lock() {
            guard.record_operation(success, latency_ms);
        }
    }
}

pub fn global_metrics_snapshot() -> Option<PerformanceMetrics> {
    let monitor = PERFORMANCE_MONITOR.get()?;
    monitor.lock().ok().map(|guard| guard.get_metrics())
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

    #[test]
    fn performance_monitor_tracks_cache_hit_rate() {
        let monitor = PerformanceMonitor::new(100);
        monitor.record_cache_hit();
        monitor.record_cache_hit();
        monitor.record_cache_hit();
        monitor.record_cache_miss();

        let metrics = monitor.get_metrics();
        assert!((metrics.cache_hit_rate - 0.75).abs() < 0.01);
    }

    #[test]
    fn performance_monitor_empty_state_returns_defaults() {
        let monitor = PerformanceMonitor::new(100);
        let metrics = monitor.get_metrics();
        assert_eq!(metrics.total_ops, 0);
        assert_eq!(metrics.avg_latency_ms, 0.0);
        assert_eq!(metrics.cache_hit_rate, 0.0);
    }
}
