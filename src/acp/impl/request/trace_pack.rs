use super::*;

static TRACE_EVENTS: OnceLock<StdMutex<Vec<TraceEvent>>> = OnceLock::new();
static ERROR_RESPONSE_IDS: OnceLock<StdMutex<HashSet<String>>> = OnceLock::new();
static TOOL_BUDGET_TRACKERS: OnceLock<StdMutex<HashMap<String, BudgetTracker>>> = OnceLock::new();
static TASK_EXECUTE_IDEMPOTENCY_CACHE: OnceLock<StdMutex<IdempotencyCache>> = OnceLock::new();
static MCP_AUDIT_LOGGER: OnceLock<AuditLogger> = OnceLock::new();
static PUA_FEEDBACK_COLLECTOR: OnceLock<PuaFeedbackCollector> = OnceLock::new();
static PUA_RESPONSE_REPORTS: OnceLock<StdMutex<HashMap<String, String>>> = OnceLock::new();

pub(super) fn trace_events() -> &'static StdMutex<Vec<TraceEvent>> {
    TRACE_EVENTS.get_or_init(|| StdMutex::new(Vec::new()))
}

pub(super) fn error_response_ids() -> &'static StdMutex<HashSet<String>> {
    ERROR_RESPONSE_IDS.get_or_init(|| StdMutex::new(HashSet::new()))
}

pub(super) fn pua_response_reports() -> &'static StdMutex<HashMap<String, String>> {
    PUA_RESPONSE_REPORTS.get_or_init(|| StdMutex::new(HashMap::new()))
}

pub(super) fn tool_budget_trackers() -> &'static StdMutex<HashMap<String, BudgetTracker>> {
    TOOL_BUDGET_TRACKERS.get_or_init(|| StdMutex::new(HashMap::new()))
}

pub(super) fn task_execute_idempotency_cache() -> &'static StdMutex<IdempotencyCache> {
    TASK_EXECUTE_IDEMPOTENCY_CACHE
        .get_or_init(|| StdMutex::new(IdempotencyCache::new(Duration::from_secs(300))))
}

pub(super) fn mcp_audit_logger() -> &'static AuditLogger {
    MCP_AUDIT_LOGGER.get_or_init(|| AuditLogger::new(Path::new(".goon").join("audit")))
}

pub(super) fn pua_feedback_collector() -> &'static PuaFeedbackCollector {
    PUA_FEEDBACK_COLLECTOR
        .get_or_init(|| PuaFeedbackCollector::new(Path::new(".goon").join("learning")))
}

pub(super) fn mark_error_response(id: Option<&Value>) {
    let Some(value) = id else {
        return;
    };
    if let Ok(mut guard) = error_response_ids().lock() {
        guard.insert(value_to_id(value));
    }
}

pub(super) fn take_error_response_mark(request_id: &str) -> bool {
    error_response_ids()
        .lock()
        .map(|mut guard| guard.remove(request_id))
        .unwrap_or(false)
}

pub(super) fn build_runtime_gauge_snapshot(server: &AcpServer) -> RuntimeGaugeSnapshot {
    let memory_cache_entries = server
        .cache
        .memory_response_cache
        .lock()
        .map(|cache| cache.active_entries() as u64)
        .unwrap_or(0);
    let sqlite_cache_entries = server
        .cache
        .response_cache
        .as_ref()
        .and_then(|cache| cache.entry_count().ok())
        .unwrap_or(0);
    let (vector_memory_entries, vector_summary_entries) = server
        .cache
        .vector_store
        .as_ref()
        .map(|store| {
            (
                store.memory_entry_count().unwrap_or(0),
                store.summary_entry_count().unwrap_or(0),
            )
        })
        .unwrap_or((0, 0));
    let breaker_snapshots = server
        .circuit_breakers
        .lock()
        .map(|guard| guard.snapshots())
        .unwrap_or_default();
    let circuit_open_agents = breaker_snapshots
        .iter()
        .filter(|item| item.state.eq_ignore_ascii_case("open"))
        .count() as u64;
    let circuit_half_open_agents = breaker_snapshots
        .iter()
        .filter(|item| item.state.eq_ignore_ascii_case("half-open"))
        .count() as u64;
    let circuit_tracked_agents = breaker_snapshots.len() as u64;
    let rate_limiter_tracked_phases = server
        .phase_rate_limiter
        .lock()
        .map(|guard| guard.tracked_phases() as u64)
        .unwrap_or(0);

    RuntimeGaugeSnapshot {
        memory_cache_entries,
        sqlite_cache_entries,
        vector_memory_entries,
        vector_summary_entries,
        circuit_open_agents,
        circuit_half_open_agents,
        circuit_tracked_agents,
        rate_limiter_tracked_phases,
    }
}

pub(super) fn trace_metrics_snapshot(server: &AcpServer) -> Value {
    let slow_top_n = server.runtime_config.trace_slow_top_n.max(1);
    let mut requests: Vec<(u64, Value)> = Vec::new();
    let mut phase_buckets: HashMap<String, Vec<u64>> = HashMap::new();
    let mut by_pua_stage: HashMap<String, u64> = HashMap::new();
    let mut buffered_events = 0usize;

    if let Ok(events) = trace_events().lock() {
        buffered_events = events.len();
        for event in events.iter() {
            if event.event_type == "request.end" {
                let method = event
                    .inputs
                    .get("attributes")
                    .and_then(|value| value.get("method"))
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();
                requests.push((
                    event.duration_ms,
                    json!({
                        "request_id": event.task_id,
                        "method": method,
                        "duration_ms": event.duration_ms,
                        "status": event.status,
                        "timestamp": event.timestamp,
                    }),
                ));
            }

            if event.duration_ms > 0
                && (event.event_type.starts_with("phase.") || event.event_type == "request.end")
            {
                phase_buckets
                    .entry(event.phase.clone())
                    .or_default()
                    .push(event.duration_ms);
            }

            if let Some(stage) = event.pua_stage.as_ref() {
                *by_pua_stage.entry(stage.clone()).or_insert(0) += 1;
            }
        }
    }

    requests.sort_by_key(|right| std::cmp::Reverse(right.0));
    requests.truncate(slow_top_n);
    let requests = requests
        .into_iter()
        .map(|(_, payload)| payload)
        .collect::<Vec<_>>();

    let mut by_phase = serde_json::Map::new();
    for (phase, mut samples) in phase_buckets {
        samples.sort_unstable();
        let p95 = if samples.is_empty() {
            0
        } else {
            let rank = ((samples.len() - 1) as f64 * 0.95).round() as usize;
            samples[rank.min(samples.len() - 1)]
        };
        let p99 = if samples.is_empty() {
            0
        } else {
            let rank = ((samples.len() - 1) as f64 * 0.99).round() as usize;
            samples[rank.min(samples.len() - 1)]
        };
        by_phase.insert(
            phase,
            json!({
                "count": samples.len(),
                "p95_ms": p95,
                "p99_ms": p99,
            }),
        );
    }

    let sampling_rate = server
        .observability
        .telemetry_runtime
        .lock()
        .map(|guard| guard.sampling_rate())
        .unwrap_or(0.0);
    let metrics = server.observability.metrics.snapshot();
    json!({
        "sampling_rate": sampling_rate,
        "buffered_events": buffered_events,
        "slow_requests_top_n": requests,
        "phase_latency": by_phase,
        "pua_stage_counts": by_pua_stage,
        "timeouts": {
            "agent_request_total": metrics.agent_timeout_failures_total,
            "review_gate_total": metrics.review_gate_timeout_total,
            "runtime_probe_total": metrics.runtime_probe_timeout_total,
        },
    })
}

pub(super) fn clone_artifact_ledger(server: &AcpServer) -> ArtifactLedger {
    server
        .artifact_ledger
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_else(|_| ArtifactLedger::new(server.config_path.as_deref().map(Path::new)))
}

pub(super) fn read_latest_artifact<T: DeserializeOwned>(
    ledger: &ArtifactLedger,
    category: &str,
    latest_name: &str,
) -> Option<T> {
    let path = ledger.latest_path(category, latest_name);
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Create new request trace
pub(super) fn new_request_trace(
    _server: &AcpServer,
    request: &JsonRpcRequest,
) -> RequestTraceContext {
    let request_id = request
        .id
        .as_ref()
        .map(value_to_id)
        .unwrap_or_else(|| "notification".to_string());

    RequestTraceContext {
        trace_id: format!("{}:{}", request.method, request_id),
        span_id: "request.root".to_string(),
        method: request.method.clone(),
        request_id,
    }
}

/// Record trace event
#[allow(clippy::too_many_arguments)]
pub(super) fn record_trace_event(
    _server: &AcpServer,
    trace: &RequestTraceContext,
    event_type: &str,
    status: &str,
    stage: &str,
    inputs: Value,
    outputs: Option<Value>,
    duration_ms: u64,
) {
    debug!(
        trace_id = %trace.trace_id,
        span_id = %trace.span_id,
        method = %trace.method,
        event = %event_type,
        status = %status,
        stage = %stage,
        duration_ms = duration_ms,
        "request trace event"
    );

    let attributes = inputs
        .get("attributes")
        .cloned()
        .unwrap_or_else(|| json!({}));
    append_trace_event(TraceEvent {
        timestamp: format!(
            "{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        ),
        event_type: match event_type {
            "request.complete" => "request.end".to_string(),
            other => other.to_string(),
        },
        task_id: trace.request_id.clone(),
        phase: stage.to_string(),
        agent: attributes
            .get("agent")
            .and_then(|v| v.as_str())
            .map(|v| v.to_string()),
        tool: None,
        status: if status == "success" {
            "ok".to_string()
        } else {
            status.to_string()
        },
        inputs: json!({"attributes": attributes}),
        outputs,
        duration_ms,
        error: None,
        pua_stage: None,
    });
}

/// Build execution cycle artifact
pub(super) fn build_execution_cycle(
    method: &str,
    action: &str,
    status: &str,
    _details: Vec<String>,
) -> Value {
    let cycle_id = format!(
        "cycle-{}-{}",
        method.replace('.', "-"),
        crate::acp::prelude::now_ts_ms()
    );
    json!({
        "cycle_id": cycle_id,
        "method": method,
        "action": action,
        "status": status,
        "current_cycle": {
            "plan_version": "1.0",
            "patch_set": [],
            "patch_set_size": 0
        },
        "cycles": [],
        "history_summary": {
            "total_cycles": 1,
            "pending_repair_iterations": 0
        },
        "auto_repair": {
            "target_subtasks": [],
            "next_cycle_preview": null
        }
    })
}
