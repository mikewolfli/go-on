//! Metrics helper functions for ACP server
//!
//! This module provides utility functions for metrics collection,
//! streaming notifications, and Prometheus metric formatting.

use serde_json::{json, Map, Value};

use crate::observability::observability::push_metric_header;

/// Histogram bucket boundaries for latency monitoring (seconds)
#[allow(dead_code)] // F-GAP-49 — Infrastructure — reserved for future histogram metric exposure
const HISTOGRAM_BUCKETS_SECONDS: [f64; 9] = [
    0.001, // 1ms
    0.005, // 5ms
    0.01,  // 10ms
    0.05,  // 50ms
    0.1,   // 100ms
    0.5,   // 500ms
    1.0,   // 1s
    5.0,   // 5s
    10.0,  // 10s
];

/// Stream chunk notification
#[allow(clippy::too_many_arguments)]
pub fn stream_chunk_notification(
    id: Option<&Value>,
    agent: &str,
    token: &str,
    chunk_index: usize,
    total_chars: usize,
    cache_level: Option<&str>,
    phase: Option<&str>,
    trace_id: Option<&str>,
    reasoning: Option<&str>,
) -> Value {
    let mut payload = Map::new();
    payload.insert("id".to_string(), id.cloned().unwrap_or(Value::Null));
    payload.insert("agent".to_string(), Value::String(agent.to_string()));
    payload.insert("token".to_string(), Value::String(token.to_string()));
    payload.insert("chunk_index".to_string(), json!(chunk_index));
    payload.insert("total_chars".to_string(), json!(total_chars));

    if let Some(level) = cache_level {
        payload.insert("cached".to_string(), Value::Bool(true));
        payload.insert("cache_level".to_string(), Value::String(level.to_string()));
    }
    if let Some(phase_name) = phase {
        payload.insert("phase".to_string(), Value::String(phase_name.to_string()));
    }
    if let Some(trace) = trace_id {
        payload.insert("trace_id".to_string(), Value::String(trace.to_string()));
    }
    if let Some(reasoning_text) = reasoning {
        if !reasoning_text.is_empty() {
            payload.insert(
                "reasoning".to_string(),
                Value::String(reasoning_text.to_string()),
            );
        }
    }

    Value::Object(payload)
}

/// Stream done notification
#[allow(clippy::too_many_arguments)]
pub fn stream_done_notification(
    id: Option<&Value>,
    agent: &str,
    chunks: usize,
    total_chars: usize,
    cache_level: Option<&str>,
    phase: Option<&str>,
    trace_id: Option<&str>,
    duration_ms: u64,
) -> Value {
    let mut payload = Map::new();
    payload.insert("id".to_string(), id.cloned().unwrap_or(Value::Null));
    payload.insert("agent".to_string(), Value::String(agent.to_string()));
    payload.insert("done".to_string(), Value::Bool(true));
    payload.insert("chunks".to_string(), json!(chunks));
    payload.insert("total_chars".to_string(), json!(total_chars));
    payload.insert("duration_ms".to_string(), json!(duration_ms));

    if let Some(level) = cache_level {
        payload.insert("cached".to_string(), Value::Bool(true));
        payload.insert("cache_level".to_string(), Value::String(level.to_string()));
    }
    if let Some(phase_name) = phase {
        payload.insert("phase".to_string(), Value::String(phase_name.to_string()));
    }
    if let Some(trace) = trace_id {
        payload.insert("trace_id".to_string(), Value::String(trace.to_string()));
    }

    Value::Object(payload)
}

/// Generate Prometheus histogram lines
#[allow(dead_code)] // F-GAP-49 — Infrastructure — reserved for future Prometheus exposition
pub fn histogram_prometheus_lines(
    name: &str,
    count: u64,
    sum_seconds: f64,
    buckets: &[u64; HISTOGRAM_BUCKETS_SECONDS.len() + 1],
) -> Vec<String> {
    let mut lines = Vec::new();
    push_metric_header(
        &mut lines,
        name,
        "histogram",
        "ACP latency distribution in seconds",
    );
    let mut cumulative = 0_u64;
    for (idx, le) in HISTOGRAM_BUCKETS_SECONDS.iter().enumerate() {
        cumulative = cumulative.saturating_add(buckets[idx]);
        lines.push(format!("{}_bucket{{le=\"{}\"}} {}", name, le, cumulative));
    }
    cumulative = cumulative.saturating_add(buckets[HISTOGRAM_BUCKETS_SECONDS.len()]);
    lines.push(format!("{}_bucket{{le=\"+Inf\"}} {}", name, cumulative));
    lines.push(format!("{}_sum {}", name, sum_seconds));
    lines.push(format!("{}_count {}", name, count));
    lines
}

/// Classify agent failure type
#[cfg(test)]
#[allow(dead_code)]
// F-GAP-49 — reserved for future use
pub fn classify_agent_failure(err: &anyhow::Error) -> &'static str {
    let msg = err.to_string().to_ascii_lowercase();
    if msg.contains("timed out") || msg.contains("timeout") {
        return "timeout";
    }
    if msg.contains("panic") {
        return "panic";
    }
    "other"
}

/// Escape Prometheus label value
#[allow(dead_code)] // F-GAP-49 — Infrastructure — reserved for future Prometheus exposition
pub fn escape_prometheus_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Metrics snapshot structure
#[allow(dead_code)] // F-GAP-49 — Infrastructure type — reserved for future metrics aggregation
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    /// Total chat requests
    pub chat_requests_total: u64,
    /// Cache lookup total
    pub cache_lookup_total: u64,
    /// Cache hit total
    pub cache_hit_total: u64,
    /// Cache store total
    #[allow(dead_code)] // Reserved for future metrics exposure
    // F-GAP-49 — reserved for future use
    pub cache_store_total: u64,
    /// Vector search total
    #[allow(dead_code)] // Reserved for future metrics exposure
    // F-GAP-49 — reserved for future use
    pub vector_search_total: u64,
    /// Vector hit total
    #[allow(dead_code)] // Reserved for future metrics exposure
    // F-GAP-49 — reserved for future use
    pub vector_hit_total: u64,
    /// Vector store total
    #[allow(dead_code)] // Reserved for future metrics exposure
    // F-GAP-49 — reserved for future use
    pub vector_store_total: u64,
    /// Summary read total
    #[allow(dead_code)] // Reserved for future metrics exposure
    // F-GAP-49 — reserved for future use
    pub summary_read_total: u64,
    /// Summary hit total
    #[allow(dead_code)] // Reserved for future metrics exposure
    // F-GAP-49 — reserved for future use
    pub summary_hit_total: u64,
    /// Summary store total
    #[allow(dead_code)] // Reserved for future metrics exposure
    // F-GAP-49 — reserved for future use
    pub summary_store_total: u64,
    /// Agent failures total
    #[allow(dead_code)] // Reserved for future metrics exposure
    // F-GAP-49 — reserved for future use
    pub agent_failures_total: u64,
    /// Agent timeout failures total
    #[allow(dead_code)] // Reserved for future metrics exposure
    // F-GAP-49 — reserved for future use
    pub agent_timeout_failures_total: u64,
    /// Local runtime probe timeout total
    pub runtime_probe_timeout_total: u64,
    /// Agent panic failures total
    #[allow(dead_code)] // Reserved for future metrics exposure
    // F-GAP-49 — reserved for future use
    pub agent_panic_failures_total: u64,
    /// Agent other failures total
    #[allow(dead_code)] // Reserved for future metrics exposure
    // F-GAP-49 — reserved for future use
    pub agent_other_failures_total: u64,
    /// Review gate total
    pub review_gate_total: u64,
    /// Review gate approved total
    pub review_gate_approved_total: u64,
    /// Review gate rejected total
    pub review_gate_rejected_total: u64,
    /// Review gate timeout total
    pub review_gate_timeout_total: u64,
    /// Review gate degraded total
    pub review_gate_degraded_total: u64,
    /// Review gate invalid response total
    pub review_gate_invalid_response_total: u64,
    /// Lazy BLUE5 doc lookup total
    #[allow(dead_code)] // Reserved for future metrics exposure
    // F-GAP-49 — reserved for future use
    pub lazy_blue5_doc_lookup_total: u64,
    /// Lazy BLUE5 doc hit total
    #[allow(dead_code)] // Reserved for future metrics exposure
    // F-GAP-49 — reserved for future use
    pub lazy_blue5_doc_hit_total: u64,
    /// Lazy BLUE5 doc reload total
    #[allow(dead_code)] // Reserved for future metrics exposure
    // F-GAP-49 — reserved for future use
    pub lazy_blue5_doc_reload_total: u64,
    /// Lazy app config lookup total
    #[allow(dead_code)] // Reserved for future metrics exposure
    // F-GAP-49 — reserved for future use
    pub lazy_app_config_lookup_total: u64,
    /// Lazy app config hit total
    #[allow(dead_code)] // Reserved for future metrics exposure
    // F-GAP-49 — reserved for future use
    pub lazy_app_config_hit_total: u64,
    /// Lazy app config reload total
    #[allow(dead_code)] // Reserved for future metrics exposure
    // F-GAP-49 — reserved for future use
    pub lazy_app_config_reload_total: u64,
    /// Lazy clarification lookup total
    #[allow(dead_code)] // Reserved for future metrics exposure
    // F-GAP-49 — reserved for future use
    pub lazy_clarification_lookup_total: u64,
    /// Lazy clarification hit total
    #[allow(dead_code)] // Reserved for future metrics exposure
    // F-GAP-49 — reserved for future use
    pub lazy_clarification_hit_total: u64,
    /// Lazy clarification reload total
    #[allow(dead_code)] // Reserved for future metrics exposure
    // F-GAP-49 — reserved for future use
    pub lazy_clarification_reload_total: u64,
    /// Chat latency count
    pub chat_latency_count: u64,
    /// Chat latency sum seconds
    pub chat_latency_sum_seconds: f64,
    /// Chat latency bucket counts
    pub chat_latency_bucket_counts: [u64; HISTOGRAM_BUCKETS_SECONDS.len() + 1],
    /// Agent latency count
    pub agent_latency_count: u64,
    /// Agent latency sum seconds
    pub agent_latency_sum_seconds: f64,
    /// Agent latency bucket counts
    pub agent_latency_bucket_counts: [u64; HISTOGRAM_BUCKETS_SECONDS.len() + 1],
    /// Review latency count
    pub review_latency_count: u64,
    /// Review latency sum seconds
    pub review_latency_sum_seconds: f64,
    /// Review latency bucket counts
    pub review_latency_bucket_counts: [u64; HISTOGRAM_BUCKETS_SECONDS.len() + 1],
}

/// Runtime gauge snapshot
#[allow(dead_code)] // F-GAP-49 — Infrastructure type — reserved for future gauge snapshot collection
#[derive(Debug, Clone)]
pub struct RuntimeGaugeSnapshot {
    /// Memory cache entries
    pub memory_cache_entries: u64,
    /// SQLite cache entries
    pub sqlite_cache_entries: u64,
    /// Vector memory entries
    #[allow(dead_code)] // F-GAP-49 — Reserved for future metrics exposure
    pub vector_memory_entries: u64,
    /// Vector summary entries
    #[allow(dead_code)] // F-GAP-49 — Reserved for future metrics exposure
    pub vector_summary_entries: u64,
    /// Circuit open agents
    #[allow(dead_code)] // F-GAP-49 — Reserved for future metrics exposure
    pub circuit_open_agents: u64,
    /// Circuit half-open agents
    #[allow(dead_code)] // F-GAP-49 — Reserved for future metrics exposure
    pub circuit_half_open_agents: u64,
    /// Circuit tracked agents
    #[allow(dead_code)] // F-GAP-49 — Reserved for future metrics exposure
    pub circuit_tracked_agents: u64,
    /// Rate limiter tracked phases
    #[allow(dead_code)] // F-GAP-49 — Reserved for future metrics exposure
    pub rate_limiter_tracked_phases: u64,
}

/// Circuit breaker snapshot
#[allow(dead_code)] // F-GAP-49 — Infrastructure type — reserved for future circuit breaker metrics
#[derive(Debug, Clone)]
pub struct CircuitBreakerSnapshot {
    /// Circuit breaker state
    pub state: String,
    /// Consecutive failures
    pub consecutive_failures: u64,
}

/// Lifecycle snapshot
#[allow(dead_code)] // F-GAP-49 — Infrastructure type — reserved for future lifecycle metrics
#[derive(Debug, Clone)]
pub struct LifecycleSnapshot {
    /// Whether shutting down
    pub shutting_down: bool,
}

/// Maintenance snapshot
#[allow(dead_code)] // F-GAP-49 — Infrastructure type — reserved for future maintenance metrics
#[derive(Debug, Clone)]
pub struct MaintenanceSnapshot {
    /// Maintenance cycles total
    pub cycles_total: u64,
    /// Whether maintenance is running
    #[allow(dead_code)] // F-GAP-49 — Reserved for future metrics exposure
    pub running: bool,
}
