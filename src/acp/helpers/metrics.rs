#[allow(clippy::too_many_arguments)]
fn stream_chunk_notification(
    id: &Option<Value>,
    agent: &str,
    token: &str,
    chunk_index: usize,
    total_chars: usize,
    cache_level: Option<&str>,
    phase: Option<&str>,
    trace_id: Option<&str>,
) -> Value {
    let mut payload = serde_json::Map::new();
    payload.insert("id".to_string(), id.clone().unwrap_or(Value::Null));
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

    Value::Object(payload)
}

#[allow(clippy::too_many_arguments)]
fn stream_done_notification(
    id: &Option<Value>,
    agent: &str,
    chunks: usize,
    total_chars: usize,
    cache_level: Option<&str>,
    phase: Option<&str>,
    trace_id: Option<&str>,
    duration_ms: u64,
) -> Value {
    let mut payload = serde_json::Map::new();
    payload.insert("id".to_string(), id.clone().unwrap_or(Value::Null));
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

fn histogram_prometheus_lines(
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

fn classify_agent_failure(err: &anyhow::Error) -> &'static str {
    let msg = err.to_string().to_ascii_lowercase();
    if msg.contains("timed out") || msg.contains("timeout") {
        return "timeout";
    }
    if msg.contains("panic") {
        return "panic";
    }
    "other"
}

fn record_agent_failure_metrics(metrics: &RuntimeMetrics, err: &anyhow::Error) {
    metrics.inc_agent_failures();
    match classify_agent_failure(err) {
        "timeout" => metrics.inc_agent_timeout_failures(),
        "panic" => metrics.inc_agent_panic_failures(),
        _ => metrics.inc_agent_other_failures(),
    }
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn hash_hex(input: &str, hex_len: usize) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();
    let full = digest
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect::<String>();
    full.chars().take(hex_len).collect()
}

fn escape_prometheus_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn build_prometheus_metrics(
    snapshot: &MetricsSnapshot,
    gauges: &RuntimeGaugeSnapshot,
    breaker_snapshot: &HashMap<String, CircuitBreakerSnapshot>,
    phase_limiter_snapshot: &HashMap<String, (f64, f64)>,
    inflight_snapshot: &(usize, HashMap<String, usize>),
    lifecycle: &LifecycleSnapshot,
    maintenance: &MaintenanceSnapshot,
) -> String {
    let mut lines = Vec::new();
    push_scalar_metric(
        &mut lines,
        "acp_chat_requests_total",
        "counter",
        "Total ACP chat requests handled",
        snapshot.chat_requests_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_cache_lookup_total",
        "counter",
        "Total cache lookups performed",
        snapshot.cache_lookup_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_cache_hit_total",
        "counter",
        "Total cache hits served",
        snapshot.cache_hit_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_cache_store_total",
        "counter",
        "Total cache writes performed",
        snapshot.cache_store_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_vector_search_total",
        "counter",
        "Total vector searches performed",
        snapshot.vector_search_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_vector_hit_total",
        "counter",
        "Total vector retrieval hits",
        snapshot.vector_hit_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_vector_store_total",
        "counter",
        "Total vector memory writes",
        snapshot.vector_store_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_summary_read_total",
        "counter",
        "Total summary memory reads",
        snapshot.summary_read_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_summary_hit_total",
        "counter",
        "Total summary memory hits",
        snapshot.summary_hit_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_summary_store_total",
        "counter",
        "Total summary memory writes",
        snapshot.summary_store_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_agent_failures_total",
        "counter",
        "Total agent execution failures",
        snapshot.agent_failures_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_agent_timeout_failures_total",
        "counter",
        "Total agent timeout failures",
        snapshot.agent_timeout_failures_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_agent_panic_failures_total",
        "counter",
        "Total agent panic failures",
        snapshot.agent_panic_failures_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_agent_other_failures_total",
        "counter",
        "Total uncategorized agent failures",
        snapshot.agent_other_failures_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_review_gate_total",
        "counter",
        "Total review gate evaluations",
        snapshot.review_gate_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_review_gate_approved_total",
        "counter",
        "Total review gate approvals",
        snapshot.review_gate_approved_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_review_gate_rejected_total",
        "counter",
        "Total review gate rejections",
        snapshot.review_gate_rejected_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_review_gate_timeout_total",
        "counter",
        "Total review gate deadline timeouts",
        snapshot.review_gate_timeout_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_review_gate_degraded_total",
        "counter",
        "Total review gate approvals degraded after timeout",
        snapshot.review_gate_degraded_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_review_gate_invalid_response_total",
        "counter",
        "Total invalid review gate responses",
        snapshot.review_gate_invalid_response_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_blue5_doc_lookup_total",
        "counter",
        "Total BLUE5 document lazy-load lookups",
        snapshot.lazy_blue5_doc_lookup_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_blue5_doc_hit_total",
        "counter",
        "Total BLUE5 document lazy-load cache hits",
        snapshot.lazy_blue5_doc_hit_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_blue5_doc_reload_total",
        "counter",
        "Total BLUE5 document lazy-load reloads",
        snapshot.lazy_blue5_doc_reload_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_app_config_lookup_total",
        "counter",
        "Total app config lazy-load lookups",
        snapshot.lazy_app_config_lookup_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_app_config_hit_total",
        "counter",
        "Total app config lazy-load cache hits",
        snapshot.lazy_app_config_hit_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_app_config_reload_total",
        "counter",
        "Total app config lazy-load reloads",
        snapshot.lazy_app_config_reload_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_clarification_lookup_total",
        "counter",
        "Total clarification artifact lazy-load lookups",
        snapshot.lazy_clarification_lookup_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_clarification_hit_total",
        "counter",
        "Total clarification artifact lazy-load cache hits",
        snapshot.lazy_clarification_hit_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_clarification_reload_total",
        "counter",
        "Total clarification artifact lazy-load reloads",
        snapshot.lazy_clarification_reload_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_memory_cache_entries",
        "gauge",
        "Current in-memory cache entries",
        gauges.memory_cache_entries,
    );
    push_scalar_metric(
        &mut lines,
        "acp_sqlite_cache_entries",
        "gauge",
        "Current SQLite cache entries",
        gauges.sqlite_cache_entries,
    );
    push_scalar_metric(
        &mut lines,
        "acp_vector_memory_entries",
        "gauge",
        "Current vector memory entries",
        gauges.vector_memory_entries,
    );
    push_scalar_metric(
        &mut lines,
        "acp_vector_summary_entries",
        "gauge",
        "Current vector summary entries",
        gauges.vector_summary_entries,
    );
    push_scalar_metric(
        &mut lines,
        "acp_circuit_open_agents",
        "gauge",
        "Current open circuit breaker agents",
        gauges.circuit_open_agents,
    );
    push_scalar_metric(
        &mut lines,
        "acp_circuit_half_open_agents",
        "gauge",
        "Current half-open circuit breaker agents",
        gauges.circuit_half_open_agents,
    );
    push_scalar_metric(
        &mut lines,
        "acp_circuit_tracked_agents",
        "gauge",
        "Current tracked circuit breaker agents",
        gauges.circuit_tracked_agents,
    );
    push_scalar_metric(
        &mut lines,
        "acp_rate_limiter_tracked_phases",
        "gauge",
        "Current tracked phases with rate limiter state",
        gauges.rate_limiter_tracked_phases,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lifecycle_shutting_down",
        "gauge",
        "Whether the ACP server is shutting down",
        if lifecycle.shutting_down { 1 } else { 0 },
    );
    push_scalar_metric(
        &mut lines,
        "acp_maintenance_cycles_total",
        "counter",
        "Total maintenance cycles executed",
        maintenance.cycles_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_maintenance_running",
        "gauge",
        "Whether a maintenance cycle is currently running",
        if maintenance.running { 1 } else { 0 },
    );

    push_metric_header(
        &mut lines,
        "acp_inflight_requests",
        "gauge",
        "Current in-flight request count by scope",
    );
    lines.push(format!(
        "acp_inflight_requests{{scope=\"global\"}} {}",
        inflight_snapshot.0
    ));
    for (phase, count) in inflight_snapshot.1.iter() {
        lines.push(format!(
            "acp_inflight_requests{{scope=\"phase\",phase=\"{}\"}} {}",
            escape_prometheus_label(phase),
            count
        ));
    }

    push_metric_header(
        &mut lines,
        "acp_phase_rate_limiter_tokens",
        "gauge",
        "Current token bucket tokens by phase",
    );
    push_metric_header(
        &mut lines,
        "acp_phase_rate_limiter_capacity",
        "gauge",
        "Current token bucket capacity by phase",
    );
    for (phase, (tokens, capacity)) in phase_limiter_snapshot.iter() {
        let phase = escape_prometheus_label(phase);
        lines.push(format!(
            "acp_phase_rate_limiter_tokens{{phase=\"{}\"}} {:.3}",
            phase, tokens
        ));
        lines.push(format!(
            "acp_phase_rate_limiter_capacity{{phase=\"{}\"}} {:.3}",
            phase, capacity
        ));
    }

    push_metric_header(
        &mut lines,
        "acp_circuit_breaker_state",
        "gauge",
        "Current circuit breaker state per agent",
    );
    push_metric_header(
        &mut lines,
        "acp_circuit_breaker_failures",
        "gauge",
        "Current consecutive failures per agent",
    );
    for (agent, state) in breaker_snapshot.iter() {
        let agent = escape_prometheus_label(agent);
        for stage in ["closed", "open", "half_open", "half_open_ready"] {
            let value = if state.state == stage { 1 } else { 0 };
            lines.push(format!(
                "acp_circuit_breaker_state{{agent=\"{}\",state=\"{}\"}} {}",
                agent, stage, value
            ));
        }
        lines.push(format!(
            "acp_circuit_breaker_failures{{agent=\"{}\"}} {}",
            agent, state.consecutive_failures
        ));
    }

    lines.extend(histogram_prometheus_lines(
        "acp_chat_latency_seconds",
        snapshot.chat_latency_count,
        snapshot.chat_latency_sum_seconds,
        &snapshot.chat_latency_bucket_counts,
    ));
    lines.extend(histogram_prometheus_lines(
        "acp_agent_latency_seconds",
        snapshot.agent_latency_count,
        snapshot.agent_latency_sum_seconds,
        &snapshot.agent_latency_bucket_counts,
    ));
    lines.extend(histogram_prometheus_lines(
        "acp_review_latency_seconds",
        snapshot.review_latency_count,
        snapshot.review_latency_sum_seconds,
        &snapshot.review_latency_bucket_counts,
    ));

    lines.join("\n")
}

