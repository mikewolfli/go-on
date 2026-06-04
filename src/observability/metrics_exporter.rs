//! Prometheus /metrics endpoint exporter.
//!
//! Provides a `/metrics` HTTP endpoint that exports Prometheus-formatted
//! metrics including: request_count, request_duration_seconds, inflight_requests,
//! circuit_breaker_state, agent_success_rate, p95_latency_ms, cache_hit_ratio,
//! and error_rate.
//!
//! ## Relationship with `telemetry_enhanced`
//!
//! GAP-B58-C15: The project has two parallel metrics systems:
//!
//! - **`metrics_exporter`** (this file) — reads `AcpServer::RuntimeMetrics` and
//!   renders a Prometheus `/metrics` endpoint. This is the **primary** path for
//!   Prometheus scraping.
//! - **`telemetry_enhanced::MetricsRecorder`** — a standalone `AppMetrics` collector
//!   used by the structured-logging / OTLP path. It tracks request counts, cache
//!   operations, memory usage, etc.
//!
//! The two are **independent** but complementary:
//! - `MetricsRecorder` feeds into structured event streams (e.g. OTLP traces, JSON logs)
//!   but is **not** exported via `/metrics`.
//! - `RuntimeMetrics` (consumed here) is the canonical source for the Prometheus
//!   endpoint. The bridge function [`bridge_metrics_recorder`] pulls
//!   `MetricsRecorder` values into `RuntimeMetrics` for unified Prometheus
//!   exposure.

use crate::acp::prelude::RuntimeMetrics;
use crate::acp::server::AcpServer;
use crate::observability::telemetry_enhanced::{global_metrics_recorder, MetricsRecorder};
use std::sync::LazyLock;
use std::sync::Mutex;

/// Sliding window over latency bucket histogram snapshots.
///
/// Stores the most recent `N` snapshots of the request_latency_bucket_counts
/// array so that P95 estimates reflect recent behavior rather than cumulative
/// lifetime counts.
///
/// GAP-B58-C15: Activated — the sliding window is wired into `build_prometheus_metrics`
/// via the global `P95_SLIDING_WINDOW`. Each call to `build_prometheus_metrics`
/// records a snapshot and uses the windowed delta for P95 estimation.
pub struct SlidingWindowBuckets {
    /// Circular buffer of bucket snapshots.
    windows: Vec<[u64; 10]>,
    /// Maximum number of snapshots to retain.
    capacity: usize,
    /// Next write index in the circular buffer.
    write_index: usize,
    /// Previous cumulative snapshot for delta computation.
    last_cumulative: Option<[u64; 10]>,
}

/// Methods on the activated sliding-window struct.
impl SlidingWindowBuckets {
    /// Create a new sliding window with the given capacity (number of snapshots).
    /// Each snapshot captures the cumulative bucket counts at a point in time.
    pub fn new(capacity: usize) -> Self {
        Self {
            windows: Vec::with_capacity(capacity),
            capacity,
            write_index: 0,
            last_cumulative: None,
        }
    }

    /// Record a new snapshot of cumulative bucket counts.
    pub fn record_snapshot(&mut self, cumulative: &[u64; 10]) {
        let delta = if let Some(prev) = self.last_cumulative {
            let mut d = [0u64; 10];
            for i in 0..10 {
                d[i] = cumulative[i].saturating_sub(prev[i]);
            }
            d
        } else {
            *cumulative
        };
        self.last_cumulative = Some(*cumulative);

        if self.windows.len() < self.capacity {
            self.windows.push(delta);
        } else {
            self.windows[self.write_index % self.capacity] = delta;
        }
        self.write_index += 1;
    }

    /// Compute the sum of all deltas in the sliding window.
    fn sum(&self) -> [u64; 10] {
        let mut total = [0u64; 10];
        for window in &self.windows {
            for i in 0..10 {
                total[i] = total[i].saturating_add(window[i]);
            }
        }
        total
    }

    /// Reset the sliding window, clearing all stored snapshots.
    pub fn reset(&mut self) {
        self.windows.clear();
        self.write_index = 0;
        self.last_cumulative = None;
    }

    /// Return the number of snapshots currently in the window.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.windows.len()
    }

    /// Return whether the window is empty (no snapshots recorded).
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }
}

/// Thread-safe wrapper around `SlidingWindowBuckets` for use in Prometheus
/// metrics rendering.
///
/// GAP-B58-C15: Activated — provides thread-safe snapshot recording and
/// windowed bucket sum for P95 calculation.
pub struct MetricsSlidingWindow {
    inner: Mutex<SlidingWindowBuckets>,
}

/// Methods on the activated sliding-window wrapper.
impl MetricsSlidingWindow {
    /// Create a new metrics sliding window with the given capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(SlidingWindowBuckets::new(capacity)),
        }
    }

    /// Record a new cumulative bucket snapshot into the window.
    pub fn record_snapshot(&self, cumulative: &[u64; 10]) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.record_snapshot(cumulative);
        }
    }

    /// Reset the sliding window.
    #[allow(dead_code)]
    pub fn reset(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.reset();
        }
    }

    /// Get the sum of all bucket deltas in the window.
    pub fn window_sum(&self) -> [u64; 10] {
        if let Ok(inner) = self.inner.lock() {
            inner.sum()
        } else {
            [0u64; 10]
        }
    }
}

/// Global sliding window for P95 latency tracking (activated).
/// Records histogram bucket snapshots on each `/metrics` scrape.
static P95_SLIDING_WINDOW: LazyLock<MetricsSlidingWindow> =
    LazyLock::new(|| MetricsSlidingWindow::new(12));

/// Reset the per-process cumulative latency bucket counts so that P95
/// reflects recent behavior rather than lifetime totals.
///
/// This is an alternative to the sliding window — call it periodically
/// (e.g. every 5 minutes) to clear the underlying bucket counters.
#[allow(dead_code)]
pub fn reset_buckets(buckets: &mut [u64; 10]) {
    for b in buckets.iter_mut() {
        *b = 0;
    }
}

/// Compute approximate P95 latency from histogram bucket counts.
///
/// Bucket boundaries match `METRIC_LATENCY_BUCKETS_MS` in `acp/prelude.rs`:
/// [1, 5, 10, 50, 100, 500, 1000, 5000, 10000] ms (9 boundaries → 10 buckets,
/// the 10th covering everything > 10000 ms).
fn estimate_p95_latency(buckets: &[u64; 10]) -> f64 {
    let total: u64 = buckets.iter().sum();
    if total == 0 {
        return 0.0;
    }
    let p95_target = (total as f64 * 0.95) as u64;
    let bucket_edges = [
        1.0,
        5.0,
        10.0,
        50.0,
        100.0,
        500.0,
        1000.0,
        5000.0,
        10000.0,
        f64::MAX,
    ];
    let mut cumulative = 0u64;
    for (i, count) in buckets.iter().enumerate() {
        cumulative += count;
        if cumulative >= p95_target {
            let prev_edge = if i == 0 { 0.0 } else { bucket_edges[i - 1] };
            let edge = bucket_edges[i];
            // Linear interpolation within the bucket
            let prev_cumulative = cumulative.saturating_sub(*count);
            let frac = if *count > 0 {
                (p95_target - prev_cumulative) as f64 / *count as f64
            } else {
                0.5
            };
            return prev_edge + frac * (edge - prev_edge);
        }
    }
    // Fallback: P95 is in the overflow bucket — interpolate between the
    // last finite edge (10000 ms) and a reasonable cap.
    bucket_edges[8] + (bucket_edges[9] - bucket_edges[8]) * 0.5
}

/// Build a Prometheus-formatted metrics string from the server status.
///
/// O7: Circuit breaker metrics are sourced from `HyperResilienceEngine`
/// (the canonical resilience authority) rather than the legacy
/// `CircuitBreakerRegistry` on `AcpServer`.
pub fn build_prometheus_metrics(server: &AcpServer) -> String {
    // Bridge OTLP MetricsRecorder values into the Prometheus RuntimeMetrics
    // on every scrape, so that manual metric recordings are visible via /metrics.
    bridge_metrics_recorder(&server.observability.metrics, global_metrics_recorder());

    let status = server.get_status();
    let m = &status.metrics;
    let lifecycle = &status.lifecycle;
    let maintenance = &status.maintenance;
    // ── O7: Read from HyperResilienceEngine instead of legacy registry ──
    let resilience_profile = server.hyper_resilience.profile();
    let circuit_breaker_open_count = resilience_profile.open_circuits;
    let is_draining = server.drain_guard.is_draining();
    let cache_hit_ratio = if m.chat_requests_total > 0 {
        m.cache_hit_rate
    } else {
        0.0
    };
    let error_rate = if m.total_requests > 0 {
        (m.failed_requests as f64 / m.total_requests as f64) * 100.0
    } else {
        0.0
    };
    let agent_success_rate = if m.total_requests > 0 {
        ((m.total_requests.saturating_sub(m.failed_requests)) as f64 / m.total_requests as f64)
            * 100.0
    } else {
        100.0
    };
    // Record snapshot into sliding window for rolling P95 estimate
    P95_SLIDING_WINDOW.record_snapshot(&m.request_latency_bucket_counts);
    let window_sum = P95_SLIDING_WINDOW.window_sum();
    // Use windowed delta if window has data, otherwise fall back to cumulative
    let p95 = if window_sum.iter().any(|&c| c > 0) {
        estimate_p95_latency(&window_sum)
    } else {
        estimate_p95_latency(&m.request_latency_bucket_counts)
    };

    let mut lines = Vec::new();

    lines.push("# HELP go_on_request_count Total number of requests processed".to_string());
    lines.push("# TYPE go_on_request_count counter".to_string());
    lines.push(format!("go_on_request_count {}", m.total_requests));

    lines.push("# HELP go_on_request_duration_seconds Request duration in seconds".to_string());
    lines.push("# TYPE go_on_request_duration_seconds gauge".to_string());
    lines.push(format!(
        "go_on_request_duration_seconds {}",
        m.avg_request_duration_ms / 1000.0
    ));

    lines.push("# HELP go_on_inflight_requests Currently active requests".to_string());
    lines.push("# TYPE go_on_inflight_requests gauge".to_string());
    lines.push(format!("go_on_inflight_requests {}", m.active_requests));

    lines.push("# HELP go_on_circuit_breaker_state Number of open circuit breakers".to_string());
    lines.push("# TYPE go_on_circuit_breaker_state gauge".to_string());
    lines.push(format!(
        "go_on_circuit_breaker_state {}",
        circuit_breaker_open_count
    ));

    lines.push("# HELP go_on_agent_success_rate Agent request success rate (0-100)".to_string());
    lines.push("# TYPE go_on_agent_success_rate gauge".to_string());
    lines.push(format!(
        "go_on_agent_success_rate {:.2}",
        agent_success_rate
    ));

    lines.push("# HELP go_on_p95_latency_ms P95 request latency in milliseconds".to_string());
    lines.push("# TYPE go_on_p95_latency_ms gauge".to_string());
    lines.push(format!("go_on_p95_latency_ms {:.1}", p95));

    lines.push("# HELP go_on_cache_hit_ratio Cache hit ratio (0.0-1.0)".to_string());
    lines.push("# TYPE go_on_cache_hit_ratio gauge".to_string());
    lines.push(format!("go_on_cache_hit_ratio {:.4}", cache_hit_ratio));

    lines.push("# HELP go_on_error_rate Request error rate percentage".to_string());
    lines.push("# TYPE go_on_error_rate gauge".to_string());
    lines.push(format!("go_on_error_rate {:.2}", error_rate));

    lines.push("# HELP go_on_chat_requests_total Total chat requests processed".to_string());
    lines.push("# TYPE go_on_chat_requests_total counter".to_string());
    lines.push(format!(
        "go_on_chat_requests_total {}",
        m.chat_requests_total
    ));

    lines.push("# HELP go_on_review_gate_total Total review gate evaluations".to_string());
    lines.push("# TYPE go_on_review_gate_total counter".to_string());
    lines.push(format!("go_on_review_gate_total {}", m.review_gate_total));

    lines.push("# HELP go_on_vector_search_total Total vector search operations".to_string());
    lines.push("# TYPE go_on_vector_search_total counter".to_string());
    lines.push(format!(
        "go_on_vector_search_total {}",
        m.vector_search_total
    ));

    lines.push("# HELP go_on_successful_requests_total Total successful requests".to_string());
    lines.push("# TYPE go_on_successful_requests_total counter".to_string());
    lines.push(format!(
        "go_on_successful_requests_total {}",
        m.successful_requests
    ));

    lines.push("# HELP go_on_failed_requests_total Total failed requests".to_string());
    lines.push("# TYPE go_on_failed_requests_total counter".to_string());
    lines.push(format!("go_on_failed_requests_total {}", m.failed_requests));

    lines.push("# HELP go_on_maintenance_mode Server maintenance mode (1=on)".to_string());
    lines.push("# TYPE go_on_maintenance_mode gauge".to_string());
    lines.push(format!(
        "go_on_maintenance_mode {}",
        if maintenance.running { 1 } else { 0 }
    ));

    lines.push("# HELP go_on_lifecycle_healthy Server lifecycle healthy (1=healthy)".to_string());
    lines.push("# TYPE go_on_lifecycle_healthy gauge".to_string());
    lines.push(format!(
        "go_on_lifecycle_healthy {}",
        if lifecycle.is_healthy { 1 } else { 0 }
    ));

    lines.push("# HELP go_on_draining Server is draining (1=draining)".to_string());
    lines.push("# TYPE go_on_draining gauge".to_string());
    lines.push(format!(
        "go_on_draining {}",
        if is_draining { 1 } else { 0 }
    ));

    // ── O2: Memory metrics ────────────────────────────────────────────────
    lines.push("# HELP go_on_memory_usage_bytes Current memory usage in bytes".to_string());
    lines.push("# TYPE go_on_memory_usage_bytes gauge".to_string());
    lines.push(format!("go_on_memory_usage_bytes {}", m.memory_usage_bytes));

    // ── O3: Task count / queue depth metrics ──────────────────────────────
    lines.push("# HELP go_on_active_requests Currently active requests".to_string());
    lines.push("# TYPE go_on_active_requests gauge".to_string());
    lines.push(format!("go_on_active_requests {}", m.active_requests));

    lines.push("# HELP go_on_agent_timeout_failures_total Agent timeout failures".to_string());
    lines.push("# TYPE go_on_agent_timeout_failures_total counter".to_string());
    lines.push(format!(
        "go_on_agent_timeout_failures_total {}",
        m.agent_timeout_failures_total
    ));

    lines.join("\n") + "\n"
}

/// Bridge `MetricsRecorder` (structured-logging / OTLP path) values into
/// `RuntimeMetrics` (Prometheus /metrics path) for unified observability.
///
/// Call this periodically (e.g. every metrics scrape) to synchronize the
/// two metric systems. Only writes fields that `MetricsRecorder` tracks
/// and `RuntimeMetrics` also exposes.
pub fn bridge_metrics_recorder(runtime_metrics: &RuntimeMetrics, recorder: &MetricsRecorder) {
    let app = recorder.get_metrics();

    runtime_metrics.update_snapshot(|snap| {
        // Merge cache hit rate using EMA-like blend
        if app.cache_hits + app.cache_misses > 0 {
            let recorder_rate = app.cache_hits as f64 / (app.cache_hits + app.cache_misses) as f64;
            if snap.cache_hit_rate == 0.0 {
                snap.cache_hit_rate = recorder_rate;
            } else {
                snap.cache_hit_rate = 0.3 * recorder_rate + 0.7 * snap.cache_hit_rate;
            }
        }

        // Latency: prefer the more recent recorder value
        if app.avg_latency_ms > 0.0 && snap.avg_request_duration_ms == 0.0 {
            snap.avg_request_duration_ms = app.avg_latency_ms;
        }

        // Active connections / memory from recorder (use max to avoid losing data)
        if app.active_connections > snap.active_requests as u64 {
            snap.active_requests = app.active_connections as u32;
        }
        if app.memory_usage_bytes > snap.memory_usage_bytes {
            snap.memory_usage_bytes = app.memory_usage_bytes;
        }
    });
}

/// A metrics recorder that bridges the OTLP `MetricsRecorder` path with the
/// Prometheus `RuntimeMetrics` path.
///
/// Wraps a `MetricsRecorder` and provides the same recording API. Call
/// [`PrometheusMetricsRecorder::bridge_to`] periodically (or on each metrics
/// scrape) to synchronize recorder values into a `RuntimeMetrics` snapshot
/// for Prometheus exposure.
pub struct PrometheusMetricsRecorder {
    inner: MetricsRecorder,
}

impl PrometheusMetricsRecorder {
    /// Create a new `PrometheusMetricsRecorder`.
    pub fn new() -> Self {
        Self {
            inner: MetricsRecorder::new(),
        }
    }

    /// Record a request outcome.
    pub fn record_request(&self, success: bool, latency_ms: f64) {
        self.inner.record_request(success, latency_ms);
    }

    /// Record a cache hit.
    pub fn record_cache_hit(&self) {
        self.inner.record_cache_hit();
    }

    /// Record a cache miss.
    pub fn record_cache_miss(&self) {
        self.inner.record_cache_miss();
    }

    /// Update the active connection count.
    pub fn update_active_connections(&self, count: u64) {
        self.inner.update_active_connections(count);
    }

    /// Update the memory usage in bytes.
    pub fn update_memory_usage(&self, bytes: u64) {
        self.inner.update_memory_usage(bytes);
    }

    /// Get the inner `AppMetrics` snapshot.
    pub fn get_metrics(&self) -> crate::observability::telemetry_enhanced::AppMetrics {
        self.inner.get_metrics()
    }

    /// Bridge the values recorded in this recorder into the given
    /// `RuntimeMetrics` for Prometheus exposure.
    pub fn bridge_to(&self, runtime_metrics: &RuntimeMetrics) {
        bridge_metrics_recorder(runtime_metrics, &self.inner);
    }
}

impl Default for PrometheusMetricsRecorder {
    fn default() -> Self {
        Self::new()
    }
}
