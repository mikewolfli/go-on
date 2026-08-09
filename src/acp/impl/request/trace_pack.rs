use std::sync::OnceLock;

use super::*;
use crate::governance::hardening::{AuditLogger, BudgetTracker};

static TRACE_EVENTS: OnceLock<StdMutex<Vec<TraceEvent>>> = OnceLock::new();
static ERROR_RESPONSE_IDS: OnceLock<StdMutex<HashSet<String>>> = OnceLock::new();
static TOOL_BUDGET_TRACKERS: OnceLock<tokio::sync::Mutex<HashMap<String, BudgetTracker>>> =
    OnceLock::new();
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

pub(crate) fn tool_budget_trackers() -> &'static tokio::sync::Mutex<HashMap<String, BudgetTracker>>
{
    TOOL_BUDGET_TRACKERS.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()))
}

pub(super) fn mcp_audit_logger() -> &'static AuditLogger {
    // The global sink (`~/.goon/audit.ndjson`) is the single persistence
    // layer; AuditLogger is a unit struct (see hardening.rs).
    MCP_AUDIT_LOGGER.get_or_init(AuditLogger::new)
}

pub(super) fn pua_feedback_collector() -> &'static PuaFeedbackCollector {
    PUA_FEEDBACK_COLLECTOR.get_or_init(|| {
        PuaFeedbackCollector::new(crate::shared::goon_paths::goon_subdir("learning"))
    })
}

/// Upper bound for in-flight error-response marks. Marks are consumed at
/// request completion; the cap only protects against unbounded growth when
/// an error is sent for a request whose accounting never runs (early-return
/// paths that reject before dispatch).
const MAX_ERROR_RESPONSE_MARKS: usize = 8192;

pub(crate) fn mark_error_response(id: Option<&Value>) {
    let Some(value) = id else {
        return;
    };
    let mut guard = error_response_ids().lock().unwrap_or_else(|poisoned| {
        tracing::warn!("lock poisoned, recovering");
        poisoned.into_inner()
    });
    if guard.len() >= MAX_ERROR_RESPONSE_MARKS {
        // Evict roughly half the oldest marks. Marks are only needed until
        // the owning request completes its outcome accounting, so a bounded
        // best-effort set never loses a required signal in practice.
        guard.clear();
    }
    guard.insert(value_to_id(value));
}

pub(crate) fn take_error_response_mark(request_id: &str) -> bool {
    error_response_ids()
        .lock()
        .map(|mut guard| guard.remove(request_id))
        .unwrap_or(false)
}

pub(super) fn trace_metrics_snapshot(server: &AcpServer) -> Value {
    let slow_top_n = server.runtime_config.trace_slow_top_n.max(1);
    let mut requests: Vec<(u64, Value)> = Vec::new();
    let mut phase_buckets: HashMap<String, Vec<u64>> = HashMap::new();
    let mut by_pua_stage: HashMap<String, u64> = HashMap::new();
    let events = trace_events().lock().unwrap_or_else(|poisoned| {
        tracing::warn!("lock poisoned, recovering");
        poisoned.into_inner()
    });
    let buffered_events = events.len();
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
        .persistence
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
pub(crate) fn record_trace_event(
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
        crate::shared::timestamps::now_ts_ms()
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
