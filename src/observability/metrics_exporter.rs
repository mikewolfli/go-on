//! Prometheus /metrics endpoint exporter.
//!
//! Provides a `/metrics` HTTP endpoint that exports Prometheus-formatted
//! metrics including: request_count, request_duration_seconds, inflight_requests,
//! circuit_breaker_state, agent_success_rate, p95_latency_ms, cache_hit_ratio,
//! and error_rate.
//!
//! ## Relationship with `telemetry_enhanced`
//!
//! GAP-B58-C15: `metrics_exporter` is the single metrics system:
//!
//! - **`metrics_exporter`** (this file) — reads `AcpServer::RuntimeMetrics` and
//!   renders a Prometheus `/metrics` endpoint. This is the **primary** path for
//!   Prometheus scraping.
//!
//! The legacy `telemetry_enhanced::MetricsRecorder` / `AppMetrics` standalone
//! collector had **zero production writers** and was removed together with the
//! `bridge_metrics_recorder` sync (which merged its all-zero values into
//! `RuntimeMetrics` on every scrape / background tick).

use crate::acp::server::AcpServer;
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

/// Compute approximate P95 latency from histogram bucket counts.
///
/// Canonical implementation shared by the Prometheus exporter, the
/// governance/status payloads and the release-readiness gate. Previously
/// three near-identical copies existed (metrics_exporter, runtime_pack,
/// status_pack) with subtly different overflow handling — the exporter's
/// old copy could return astronomically large values when samples fell in
/// the overflow bucket.
///
/// Bucket boundaries match `METRIC_LATENCY_BUCKETS_MS` in `acp/prelude.rs`:
/// [1, 5, 10, 50, 100, 500, 1000, 5000, 10000] ms (9 boundaries → 10 buckets,
/// the 10th covering everything > 10000 ms).
pub(crate) fn estimate_p95_from_buckets(bucket_counts: &[u64; 10]) -> f64 {
    const P95_BUCKET_BOUNDARIES_MS: [f64; 10] = [
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
    let total: u64 = bucket_counts.iter().sum();
    if total == 0 {
        return 0.0;
    }
    let target = (total as f64 * 0.95).ceil();
    let mut cumulative: u64 = 0;
    for (i, &count) in bucket_counts.iter().enumerate() {
        cumulative += count;
        if cumulative as f64 >= target {
            // Found the bucket containing p95
            let bucket_lower = if i == 0 {
                0.0
            } else {
                P95_BUCKET_BOUNDARIES_MS[i - 1]
            };
            let bucket_upper = P95_BUCKET_BOUNDARIES_MS[i.min(9)];
            if bucket_upper == f64::MAX || bucket_upper - bucket_lower <= 0.0 || count == 0 {
                // Overflow bucket or degenerate case — use twice the lower
                // bound as a conservative estimate instead of interpolating
                // against f64::MAX.
                return if i == 9 {
                    bucket_lower * 2.0
                } else {
                    bucket_lower
                };
            }
            let prev_cumulative = cumulative.saturating_sub(count);
            let fraction = (target - prev_cumulative as f64) / count as f64;
            let estimated = bucket_lower + fraction * (bucket_upper - bucket_lower);
            return (estimated * 100.0).round() / 100.0;
        }
    }
    // All samples fall within buckets — use the upper bound of the last bucket.
    P95_BUCKET_BOUNDARIES_MS[8]
}

/// Build a Prometheus-formatted metrics string from the server status.
///
/// O7: Circuit breaker metrics are sourced from `HyperResilienceEngine`
/// (the canonical resilience authority) rather than the legacy
/// `CircuitBreakerRegistry` on `AcpServer`.
pub async fn build_prometheus_metrics(server: &AcpServer) -> String {
    // NOTE: the `bridge_metrics_recorder` sync (OTLP MetricsRecorder →
    // RuntimeMetrics) was removed — MetricsRecorder has zero production
    // writers, so every bridge call merged all-zero values (dead work on
    // every /metrics scrape and a 15s background loop).
    let status = server.get_status();
    let m = &status.metrics;
    let lifecycle = &status.lifecycle;
    let maintenance = &status.maintenance;
    // ── O7: Read from HyperResilienceEngine instead of legacy registry ──
    let resilience_profile = server.resilience.hyper_resilience.profile().await;
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
        estimate_p95_from_buckets(&window_sum)
    } else {
        estimate_p95_from_buckets(&m.request_latency_bucket_counts)
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
    // (the active-request gauge was removed: `go_on_inflight_requests` above
    // already reports `m.active_requests` with the same HELP text — the
    // duplicate metric name only confused scrapers.)

    lines.push("# HELP go_on_agent_timeout_failures_total Agent timeout failures".to_string());
    lines.push("# TYPE go_on_agent_timeout_failures_total counter".to_string());
    lines.push(format!(
        "go_on_agent_timeout_failures_total {}",
        m.agent_timeout_failures_total
    ));

    lines.join("\n") + "\n"
}
