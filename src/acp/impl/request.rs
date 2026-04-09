//! Request handling implementation functions for ACP server
//!
//! This module contains standalone functions that implement request handling
//! functionality previously in the `impl AcpServer` block in `impl/request.rs`.
//! These functions take `AcpServer` as their first parameter to maintain
//! compatibility with the original implementation.

use std::sync::{Mutex as StdMutex, OnceLock};

use anyhow::Result;
use serde_json::{json, Value};
use tracing::{debug, info};

use crate::acp::server::AcpServer;
use crate::evaluation::TraceEvent;

use crate::i18n::runtime::{t, tf};
use crate::reinforcement::{
    persist_clarification_session_artifact, persist_consultation_artifact,
    persist_primary_secondary_failover_artifact, persist_primary_secondary_policy_artifact,
    persist_requirement_contract, persist_workflow_learning_event, ArtifactLedger,
    ClarificationSessionArtifact, ConsultationArtifact, PrimaryFailoverReportItem,
    PrimarySecondaryFailoverArtifact, PrimarySecondaryPolicyArtifact, RequirementContractArtifact,
    WorkflowLearningBusArtifact, WorkflowLearningEvent,
};

use crate::rpc_protocol::{value_to_id, JsonRpcRequest, RequestTraceContext};

static TRACE_EVENTS: OnceLock<StdMutex<Vec<TraceEvent>>> = OnceLock::new();

fn trace_events() -> &'static StdMutex<Vec<TraceEvent>> {
    TRACE_EVENTS.get_or_init(|| StdMutex::new(Vec::new()))
}

pub(crate) fn append_trace_event(event: TraceEvent) {
    if let Ok(mut guard) = trace_events().lock() {
        guard.push(event);
        if guard.len() > 2048 {
            let overflow = guard.len() - 2048;
            guard.drain(0..overflow);
        }
    }
}

/// Handle JSON-RPC request
///
/// This function replaces the `AcpServer::handle_request` method.
pub async fn handle_request(server: &AcpServer, request: JsonRpcRequest) -> Result<()> {
    let trace = new_request_trace(server, &request);
    let _request_span = if let Ok(telemetry_guard) = server.telemetry_runtime.lock() {
        telemetry_guard.start_root_span(
            "acp.request",
            &format!("{}:{}", trace.method, trace.request_id),
            vec![],
        )
    } else {
        None
    };

    record_trace_event(
        server,
        &trace,
        "request.start",
        "started",
        "entry",
        json!({"method": trace.method.clone()}),
        None,
        0,
    );

    let request_id = request.id.clone();
    let result = match request.method.as_str() {
        "initialize" => handle_initialize(server, request_id).await,
        "mcp.initialize" => handle_mcp_initialize(server, request_id).await,
        "mcp.tools.list" => handle_mcp_tools_list(server, request_id).await,
        "mcp.tools.call" => {
            handle_mcp_tools_call(server, request.params.unwrap_or_default(), request_id).await
        }
        "chat" => {
            handle_chat(
                server,
                request.params.unwrap_or_default(),
                request_id,
                &trace,
            )
            .await
        }
        "phase" | "phase.status" => {
            handle_phase(
                server,
                request.params.unwrap_or_default(),
                request_id,
                &trace,
            )
            .await
        }
        "metrics" => handle_metrics(server, request_id).await,
        "metrics.prometheus" => handle_metrics_prometheus(server, request_id).await,
        "metrics.reset" => handle_metrics_reset(server, request_id).await,
        "debug_panel.get" | "debug.panel.get" => {
            handle_debug_panel_get(server, request.params.unwrap_or_default(), request_id).await
        }
        "trace.get" => {
            handle_trace_get(server, request.params.unwrap_or_default(), request_id).await
        }
        "shutdown" => handle_shutdown(server, request_id).await,
        "health" | "runtime.health" => handle_health(server, request_id).await,
        "breaker.status" => handle_breaker_status(server, request_id).await,
        "breaker.reset" => {
            handle_breaker_reset(server, request.params.unwrap_or_default(), request_id).await
        }
        "cache.clear" => handle_cache_clear(server, request_id).await,
        "conversation.checkpoint.create" => {
            handle_conversation_checkpoint_create(
                server,
                request.params.unwrap_or_default(),
                request_id,
            )
            .await
        }
        "conversation.checkpoint.list" => {
            handle_conversation_checkpoint_list(
                server,
                request.params.unwrap_or_default(),
                request_id,
            )
            .await
        }
        "conversation.rollback" => {
            handle_conversation_rollback(server, request.params.unwrap_or_default(), request_id)
                .await
        }
        "conversation.checkpoint.prune" => {
            handle_conversation_checkpoint_prune(
                server,
                request.params.unwrap_or_default(),
                request_id,
            )
            .await
        }
        "config.reload" => handle_config_reload(server, request_id).await,
        "autotune.status" => handle_autotune_status(server, request_id).await,
        "autotune.reset" => {
            handle_autotune_reset(server, request.params.unwrap_or_default(), request_id).await
        }
        "workflow.confirm" => {
            handle_workflow_confirm(
                server,
                request.params.unwrap_or_default(),
                request_id,
                &trace,
            )
            .await
        }
        "workflow.clarify" => {
            handle_workflow_clarify(
                server,
                request.params.unwrap_or_default(),
                request_id,
                &trace,
            )
            .await
        }
        "workflow.research" => {
            handle_workflow_research(
                server,
                request.params.unwrap_or_default(),
                request_id,
                &trace,
            )
            .await
        }
        "workflow.consult" => {
            handle_workflow_consult(
                server,
                request.params.unwrap_or_default(),
                request_id,
                &trace,
            )
            .await
        }
        "workflow.execute" => {
            handle_workflow_execute(
                server,
                request.params.unwrap_or_default(),
                request_id,
                &trace,
            )
            .await
        }
        "task.execute" => {
            handle_task_execute(server, request.params.unwrap_or_default(), request_id).await
        }
        "learning.summary" => {
            handle_learning_summary(server, request.params.unwrap_or_default(), request_id).await
        }
        "primary_secondary.summary" => {
            handle_primary_secondary_summary(server, request.params.unwrap_or_default(), request_id)
                .await
        }
        _ => {
            send_error(
                server,
                request_id,
                -32601,
                format!("unknown method: {}", request.method),
                None,
            )
            .await
        }
    };

    let duration_ms = 0; // Simplified for now
    let status = if result.is_ok() { "success" } else { "error" };

    record_trace_event(
        server,
        &trace,
        "request.complete",
        status,
        "exit",
        json!({}),
        None,
        duration_ms,
    );

    result
}

/// Handle initialize request
async fn handle_initialize(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    send_result(
        server,
        request_id,
        json!({
            "name": "go-on",
            "version": "0.3.2",
            "protocol": "acp",
            "capabilities": {
                "chat": true,
                "phase": true,
                "metrics": true,
                "shutdown": true,
                "health": true,
                "debug_panel": true,
                "mcp_adapter": true,
            }
        }),
    )
    .await
}

/// Handle MCP initialize request
async fn handle_mcp_initialize(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    send_result(
        server,
        request_id,
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "serverInfo": {
                "name": "go-on",
                "version": "0.3.2"
            }
        }),
    )
    .await
}

/// Handle MCP tools list request
async fn handle_mcp_tools_list(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    send_result(
        server,
        request_id,
        json!({
            "tools": [
                {
                    "name": "acp_trace_get",
                    "description": "Get ACP trace events",
                    "input_schema": {"type": "object"}
                },
                {
                    "name": "acp_debug_panel_get",
                    "description": "Get ACP debug panel snapshot",
                    "input_schema": {"type": "object"}
                }
            ]
        }),
    )
    .await
}

async fn handle_mcp_tools_call(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let name = params
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or_default();

    let structured = match name {
        "acp_trace_get" => json!({"ok": true, "events": []}),
        "acp_debug_panel_get" => json!({"ok": true, "panel": {}}),
        _ => {
            return send_error(
                server,
                request_id,
                -32602,
                format!("unknown mcp tool: {name}"),
                None,
            )
            .await
        }
    };

    send_result(
        server,
        request_id,
        json!({
            "content": [{"type": "text", "text": structured.to_string()}],
            "structuredContent": structured
        }),
    )
    .await
}

/// Handle chat request
async fn handle_chat(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
    trace: &RequestTraceContext,
) -> Result<()> {
    use crate::acp::r#impl::chat::handle_chat as chat_handler;

    match chat_handler(
        server,
        request_id.clone(),
        Some(params),
        None,
        Some(trace.clone()),
    )
    .await
    {
        Ok(()) => Ok(()),
        Err(err) => {
            let message = err.to_string();
            if message.to_ascii_lowercase().contains("rate limited") {
                send_error(server, request_id, -32029, message, None).await
            } else {
                send_error(server, request_id, -32603, message, None).await
            }
        }
    }
}

/// Handle phase request
async fn handle_phase(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
    _trace: &RequestTraceContext,
) -> Result<()> {
    let rate_limiter = server
        .phase_rate_limiter
        .lock()
        .map(|guard| {
            json!({
                "tracked": guard.tracked_phases(),
                "buckets": guard.snapshot(),
            })
        })
        .unwrap_or_else(|_| json!({"tracked": 0, "buckets": {}}));

    let inflight = server
        .inflight_limiter
        .lock()
        .map(|guard| {
            let (global, phase) = guard.snapshot();
            json!({"global": global, "phase": phase})
        })
        .unwrap_or_else(|_| json!({"global": 0, "phase": {}}));

    send_result(
        server,
        request_id,
        json!({
            "rate_limiter": rate_limiter,
            "inflight": inflight,
        }),
    )
    .await
}

/// Handle metrics request
async fn handle_metrics(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    let status = server.get_status();
    send_result(
        server,
        request_id,
        json!({
            "metrics": status.metrics,
            "timestamp": status.timestamp,
        }),
    )
    .await
}

/// Handle debug panel get request
async fn handle_debug_panel_get(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let state = server.conversation_state.blocking_lock();
    let conversation_count = state
        .checkpoints
        .iter()
        .map(|cp| cp.conversation_id.clone())
        .collect::<std::collections::HashSet<_>>()
        .len();
    let checkpoint_count = state.checkpoints.len();

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "panel": {
                "trace": {"stage_transitions": []},
                "selected_agents": [],
                "review_outcomes": [],
                "runtime_health": {"ok": true},
                "review_gate": {
                    "total": server.metrics.snapshot().review_gate_total,
                },
                "conversations": {
                    "count": conversation_count,
                    "checkpoints": checkpoint_count,
                }
            }
        }),
    )
    .await
}

/// Handle trace get request
async fn handle_trace_get(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;

    let trace_events = trace_events()
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    let trace_events_len = trace_events.len();

    let limited_trace_events = if trace_events.len() > limit {
        trace_events[trace_events.len() - limit..].to_vec()
    } else {
        trace_events
    };

    send_result(
        server,
        request_id,
        json!({
            "events": limited_trace_events,
            "total": trace_events_len,
            "limit": limit,
        }),
    )
    .await
}

/// Handle shutdown request
async fn handle_shutdown(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    info!("{}", t("info.shutdown_requested"));
    server.begin_shutdown();
    server.shutdown_notify.notify_waiters();

    send_result(
        server,
        request_id,
        json!({
            "shutdown": "initiated"
        }),
    )
    .await
}

/// Handle health request
async fn handle_health(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    let status = server.get_status();
    let metrics = server.metrics.snapshot();
    send_result(
        server,
        request_id,
        json!({
            "lifecycle": {
                "shutting_down": status.lifecycle.shutdown_requested,
                "is_healthy": status.lifecycle.is_healthy,
                "uptime_seconds": status.lifecycle.uptime_seconds,
            },
            "maintenance": status.maintenance,
            "review_gate": {
                "total": metrics.review_gate_total,
                "approved": metrics.review_gate_approved_total,
                "rejected": metrics.review_gate_rejected_total,
                "timeout": metrics.review_gate_timeout_total,
                "degraded": metrics.review_gate_degraded_total,
                "invalid_response": metrics.review_gate_invalid_response_total,
            },
            "timestamp": status.timestamp,
        }),
    )
    .await
}

/// Handle autotune status request
async fn handle_autotune_status(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    let autotune_state = if let Some(autotune) = server.autotune.as_ref() {
        let lock = autotune.lock().await;
        Some(lock.clone())
    } else {
        None
    };

    let autotune_config = if let Some(config) = &server.autotune_config {
        Some(config.clone())
    } else {
        None
    };

    send_result(
        server,
        request_id,
        json!({
            "enabled": autotune_config.as_ref().map(|cfg| cfg.enabled).unwrap_or(false),
            "state": autotune_state,
        }),
    )
    .await
}

/// Handle autotune reset request
async fn handle_autotune_reset(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    // Simplified implementation for now
    send_result(server, request_id, json!({"autotune": "reset"})).await
}

/// Handle workflow confirm request
async fn handle_workflow_confirm(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
    _trace: &RequestTraceContext,
) -> Result<()> {
    // Simplified implementation for now
    send_result(server, request_id, json!({"workflow": "confirmed"})).await
}

/// Handle workflow clarify request
async fn handle_workflow_clarify(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
    _trace: &RequestTraceContext,
) -> Result<()> {
    // Simplified implementation for now
    send_result(server, request_id, json!({"workflow": "clarified"})).await
}

/// Handle workflow research request
async fn handle_workflow_research(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
    _trace: &RequestTraceContext,
) -> Result<()> {
    // Simplified implementation for now
    send_result(server, request_id, json!({"workflow": "researched"})).await
}

/// Handle workflow consult request
async fn handle_workflow_consult(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
    _trace: &RequestTraceContext,
) -> Result<()> {
    // Simplified implementation for now
    send_result(server, request_id, json!({"workflow": "consulted"})).await
}

/// Handle workflow execute request
async fn handle_workflow_execute(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
    _trace: &RequestTraceContext,
) -> Result<()> {
    // Simplified implementation for now
    send_result(server, request_id, json!({"workflow": "executed"})).await
}

// Note: The following functions are referenced but not yet implemented.
// They will be implemented when we migrate the corresponding modules.

/// Send error response
async fn send_error(
    server: &AcpServer,
    id: Option<Value>,
    code: i64,
    message: String,
    data: Option<Value>,
) -> Result<()> {
    crate::acp::r#impl::io::send_error(server, id, code, message, data).await
}

/// Send result response
async fn send_result(server: &AcpServer, id: Option<Value>, result: Value) -> Result<()> {
    crate::acp::r#impl::io::send_result(server, id, result).await
}

/// Create new request trace
fn new_request_trace(_server: &AcpServer, request: &JsonRpcRequest) -> RequestTraceContext {
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
fn record_trace_event(
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
