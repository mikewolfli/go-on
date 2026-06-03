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
//!   endpoint. Future work may bridge `MetricsRecorder` values into `RuntimeMetrics`
//!   for unified Prometheus exposure.

use crate::acp::server::AcpServer;

/// Compute approximate P95 latency from histogram bucket counts.
/// Buckets are: [1, 5, 10, 25, 50, 100, 250, 500, 1000, 5000] ms.
fn estimate_p95_latency(buckets: &[u64; 10]) -> f64 {
    let total: u64 = buckets.iter().sum();
    if total == 0 {
        return 0.0;
    }
    let p95_target = (total as f64 * 0.95) as u64;
    let bucket_edges = [
        1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 5000.0,
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
    bucket_edges[9]
}

/// Build a Prometheus-formatted metrics string from the server status.
pub fn build_prometheus_metrics(server: &AcpServer) -> String {
    let status = server.get_status();
    let m = &status.metrics;
    let lifecycle = &status.lifecycle;
    let maintenance = &status.maintenance;
    let circuit_breakers = &status.circuit_breakers;
    let is_draining = server.drain_guard.is_draining();
    let circuit_breaker_open_count = circuit_breakers
        .iter()
        .filter(|c| c.state == "open")
        .count();
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
    let p95 = estimate_p95_latency(&m.request_latency_bucket_counts);

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

    lines.join("\n") + "\n"
}
