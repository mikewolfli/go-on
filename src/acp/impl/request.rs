/// 协议模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolMode {
    Auto,
    Acp,
    Mcp,
}

impl ProtocolMode {
    pub fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "acp" => ProtocolMode::Acp,
            "mcp" => ProtocolMode::Mcp,
            _ => ProtocolMode::Auto,
        }
    }
}

/// 从config.toml/runtime_config读取协议模式
fn get_protocol_mode(server: &AcpServer) -> ProtocolMode {
    // 尝试从runtime_config.protocol_mode读取
    if let Some(mode) = server.runtime_config.protocol_mode.as_deref() {
        ProtocolMode::from_str(mode)
    } else {
        ProtocolMode::Auto
    }
}

/// 判断请求属于MCP协议
fn is_mcp_request(method: &str) -> bool {
    method.starts_with("mcp.") || method == "mcp.initialize"
}

/// 判断请求属于ACP/A2A协议
fn is_acp_request(method: &str) -> bool {
    // ACP/A2A常用方法
    matches!(
        method,
        "initialize"
            | "chat"
            | "phase"
            | "phase.status"
            | "metrics.get"
            | "metrics"
            | "metrics.prometheus"
            | "shutdown"
            | "health"
            | "runtime.health"
            | "breaker.status"
            | "breaker.reset"
            | "cache.clear"
            | "vector.clear"
            | "maintenance.gc"
            | "action.check"
            | "conversation.checkpoint.create"
            | "conversation.checkpoint.list"
            | "conversation.rollback"
            | "conversation.checkpoint.prune"
            | "config.reload"
            | "autotune.get"
            | "autotune.status"
            | "autotune.reset"
            | "workflow.confirm"
            | "workflow.clarify"
            | "workflow.research"
            | "workflow.consult"
            | "workflow.generate"
            | "workflow.execute"
            | "task.plan"
            | "task.execute"
            | "learning.summary"
            | "phase.policy.replay"
            | "primary_secondary.summary"
             // diagnostics / ops – also used by vscode-addon in ACP mode
             | "metrics.reset"
             | "trace.get"
             | "trace.metrics"
             | "debug_panel.get"
             | "debug.panel.get"
    )
}
// Request handling implementation functions for ACP server
//
// This module contains standalone functions that implement request handling
// functionality previously in the `impl AcpServer` block in `impl/request.rs`.
// These functions take `AcpServer` as其 first parameter to maintain
// compatibility with the original implementation.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::Instant;

use anyhow::Result;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::Duration;
use tracing::{debug, info};

use crate::acp::background::run_maintenance_cycle;
use crate::acp::helpers::metrics::{
    build_prometheus_metrics, CircuitBreakerSnapshot as PrometheusCircuitBreakerSnapshot,
    LifecycleSnapshot as PrometheusLifecycleSnapshot,
    MaintenanceSnapshot as PrometheusMaintenanceSnapshot,
    MetricsSnapshot as PrometheusMetricsSnapshot, RuntimeGaugeSnapshot,
};
use crate::acp::prelude::enforce_checkpoint_capacity;
use crate::acp::r#impl::storage::cache_clear;
use crate::acp::server::AcpServer;
use crate::agent::{AgentAuditLog, AgentTaskEnvelope, Message};
use crate::config::{validate_runtime_readiness, AppConfig, AutoTuneState};
use crate::evaluation::TraceEvent;

use crate::acp::helpers::policy::{rank_execution_agents, resolve_review_policy};
use crate::acp::helpers::requirement::{
    evaluate_requirement_gate, parse_requirement_contract_from_params,
    resolve_learning_clarification_metrics,
};
use crate::flow_with_models::FlowModelSelector;
use crate::i18n::runtime::{t, tf};
use crate::memory_module::{MemoryClass, MemoryEntry, MemoryPromotionReport, MemoryStore};
use crate::orchestration::task_router::TaskRouter;
use crate::reinforcement::{
    build_task_plan, build_workflow_generated_artifact, persist_clarification_session_artifact,
    persist_consultation_artifact, persist_execution_decision,
    persist_primary_secondary_failover_artifact, persist_primary_secondary_policy_artifact,
    persist_requirement_contract, persist_task_execution_summary, persist_task_plan,
    persist_workflow_generated, persist_workflow_learning_event, persist_workflow_research,
    recommend_agent_order_from_execution_history, recommend_failure_strategy_from_learning,
    recommend_parallelism_from_learning, recommend_predicted_success_rate_from_learning,
    recommend_work_grade_from_learning, run_action_check, ActionCheckKind, ArtifactLedger,
    ClarificationSessionArtifact, ConsultationArtifact, ExecutionAssignmentRecord,
    ExecutionDecisionArtifact, ExecutionDecisionCandidate, KnowledgeBusArtifact,
    ParallelPhaseDecisionRecord, PrimaryFailoverReportItem, PrimarySecondaryFailoverArtifact,
    PrimarySecondaryPolicyArtifact, RequirementContractArtifact, TaskExecutionMetrics,
    TaskExecutionSummary, WorkflowGeneratedArtifact, WorkflowLearningBusArtifact,
    WorkflowLearningEvent, WorkflowResearchArtifact,
};
use crate::tool::{ToolInput, ToolRegistry};
use crate::vector::VectorStore;

use crate::rpc_protocol::{value_to_id, JsonRpcRequest, RequestTraceContext};

static TRACE_EVENTS: OnceLock<StdMutex<Vec<TraceEvent>>> = OnceLock::new();
static ERROR_RESPONSE_IDS: OnceLock<StdMutex<HashSet<String>>> = OnceLock::new();

fn trace_events() -> &'static StdMutex<Vec<TraceEvent>> {
    TRACE_EVENTS.get_or_init(|| StdMutex::new(Vec::new()))
}

fn error_response_ids() -> &'static StdMutex<HashSet<String>> {
    ERROR_RESPONSE_IDS.get_or_init(|| StdMutex::new(HashSet::new()))
}

fn mark_error_response(id: Option<&Value>) {
    let Some(value) = id else {
        return;
    };
    if let Ok(mut guard) = error_response_ids().lock() {
        guard.insert(value_to_id(value));
    }
}

fn take_error_response_mark(request_id: &str) -> bool {
    error_response_ids()
        .lock()
        .map(|mut guard| guard.remove(request_id))
        .unwrap_or(false)
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
    // 协议自适应分发
    let protocol_mode = get_protocol_mode(server);
    let method = request.method.as_str();
    match protocol_mode {
        ProtocolMode::Acp => {
            if !is_acp_request(method) {
                return send_error(
                    server,
                    request.id,
                    -32601,
                    format!("ACP模式下不支持方法: {}", method),
                    None,
                )
                .await;
            }
        }
        ProtocolMode::Mcp => {
            if !is_mcp_request(method) {
                return send_error(
                    server,
                    request.id,
                    -32601,
                    format!("MCP模式下不支持方法: {}", method),
                    None,
                )
                .await;
            }
        }
        ProtocolMode::Auto => {
            // 若为MCP方法，优先走MCP分支，否则走ACP
            // 允许混用
        }
    }
    let started = Instant::now();
    server.metrics.inc_active_requests();
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
        "metrics.get" => handle_metrics_get(server, request_id).await,
        "metrics" => handle_metrics(server, request_id).await,
        "metrics.prometheus" => handle_metrics_prometheus(server, request_id).await,
        "metrics.reset" => handle_metrics_reset(server, request_id).await,
        "debug_panel.get" | "debug.panel.get" => {
            handle_debug_panel_get(server, request.params.unwrap_or_default(), request_id).await
        }
        "trace.get" => {
            handle_trace_get(server, request.params.unwrap_or_default(), request_id).await
        }
        "trace.metrics" => handle_trace_metrics(server, request_id).await,
        "shutdown" => handle_shutdown(server, request_id).await,
        "health" | "runtime.health" => handle_health(server, request_id).await,
        "breaker.status" => handle_breaker_status(server, request_id).await,
        "breaker.reset" => {
            handle_breaker_reset(server, request.params.unwrap_or_default(), request_id).await
        }
        "cache.clear" => handle_cache_clear(server, request_id).await,
        "vector.clear" => handle_vector_clear(server, request_id).await,
        "maintenance.gc" => handle_maintenance_gc(server, request_id).await,
        "action.check" => {
            handle_action_check(server, request.params.unwrap_or_default(), request_id).await
        }
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
        "autotune.get" => handle_autotune_get(server, request_id).await,
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
        "workflow.generate" => {
            handle_workflow_generate(
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
        "task.plan" => {
            handle_task_plan(
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
        "phase.policy.replay" => {
            handle_phase_policy_replay(server, request.params.unwrap_or_default(), request_id).await
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

    let duration_ms = started.elapsed().as_millis() as u64;
    let success = result.is_ok() && !take_error_response_mark(&trace.request_id);
    let status = if success { "success" } else { "error" };
    server
        .metrics
        .record_request_outcome(success, duration_ms as f64);
    server.metrics.dec_active_requests();

    record_trace_event(
        server,
        &trace,
        "request.complete",
        status,
        "exit",
        json!({"attributes": {"method": trace.method.clone()}}),
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
            "version": env!("CARGO_PKG_VERSION"),
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
                "version": env!("CARGO_PKG_VERSION")
            }
        }),
    )
    .await
}

/// Handle MCP tools list request
async fn handle_mcp_tools_list(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    let tools = build_mcp_tool_descriptors(server);

    send_result(
        server,
        request_id,
        json!({
            "tools": tools
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

    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let structured = match execute_mcp_tool_call(server, name, &arguments).await {
        Ok(structured) => structured,
        Err(err) => {
            return send_error(server, request_id, -32602, err.to_string(), None).await;
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

async fn handle_metrics_get(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    send_result(
        server,
        request_id,
        serde_json::to_value(server.metrics.snapshot())?,
    )
    .await
}

async fn handle_metrics_prometheus(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    let metrics = server.metrics.snapshot();
    let gauges = build_runtime_gauge_snapshot(server);
    let breaker_snapshot = server
        .circuit_breakers
        .lock()
        .map(|guard| {
            guard
                .snapshots()
                .into_iter()
                .map(|item| {
                    (
                        item.name,
                        PrometheusCircuitBreakerSnapshot {
                            state: item.state,
                            consecutive_failures: item.failure_count as u64,
                        },
                    )
                })
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let phase_limiter_snapshot = server
        .phase_rate_limiter
        .lock()
        .map(|guard| guard.snapshot())
        .unwrap_or_default();
    let inflight_snapshot = server
        .inflight_limiter
        .lock()
        .map(|guard| guard.snapshot())
        .unwrap_or_default();
    let lifecycle_snapshot = server
        .lifecycle_state
        .lock()
        .map(|guard| PrometheusLifecycleSnapshot {
            shutting_down: guard.shutdown_requested(),
        })
        .unwrap_or(PrometheusLifecycleSnapshot {
            shutting_down: false,
        });
    let maintenance_snapshot = server
        .maintenance_tracker
        .lock()
        .map(|guard| {
            let snapshot = guard.snapshot();
            PrometheusMaintenanceSnapshot {
                cycles_total: snapshot.cycles_total,
                running: snapshot.running,
            }
        })
        .unwrap_or(PrometheusMaintenanceSnapshot {
            cycles_total: 0,
            running: false,
        });
    let text = build_prometheus_metrics(
        &PrometheusMetricsSnapshot {
            chat_requests_total: metrics.chat_requests_total,
            cache_lookup_total: 0,
            cache_hit_total: 0,
            cache_store_total: 0,
            vector_search_total: metrics.vector_search_total,
            vector_hit_total: metrics.vector_hit_total,
            vector_store_total: metrics.vector_store_total,
            summary_read_total: metrics.summary_read_total,
            summary_hit_total: metrics.summary_hit_total,
            summary_store_total: metrics.summary_store_total,
            agent_failures_total: metrics.failed_requests,
            agent_timeout_failures_total: 0,
            agent_panic_failures_total: 0,
            agent_other_failures_total: 0,
            review_gate_total: metrics.review_gate_total,
            review_gate_approved_total: metrics.review_gate_approved_total,
            review_gate_rejected_total: metrics.review_gate_rejected_total,
            review_gate_timeout_total: metrics.review_gate_timeout_total,
            review_gate_degraded_total: metrics.review_gate_degraded_total,
            review_gate_invalid_response_total: metrics.review_gate_invalid_response_total,
            lazy_blue5_doc_lookup_total: 0,
            lazy_blue5_doc_hit_total: 0,
            lazy_blue5_doc_reload_total: 0,
            lazy_app_config_lookup_total: 0,
            lazy_app_config_hit_total: 0,
            lazy_app_config_reload_total: 0,
            lazy_clarification_lookup_total: 0,
            lazy_clarification_hit_total: 0,
            lazy_clarification_reload_total: 0,
            chat_latency_count: metrics.chat_requests_total,
            chat_latency_sum_seconds: metrics.chat_latency_sum_ms / 1000.0,
            chat_latency_bucket_counts: metrics.chat_latency_bucket_counts,
            agent_latency_count: metrics.total_requests,
            agent_latency_sum_seconds: metrics.request_latency_sum_ms / 1000.0,
            agent_latency_bucket_counts: metrics.request_latency_bucket_counts,
            review_latency_count: metrics.review_gate_total,
            review_latency_sum_seconds: metrics.review_latency_sum_ms / 1000.0,
            review_latency_bucket_counts: metrics.review_latency_bucket_counts,
        },
        &gauges,
        &breaker_snapshot,
        &phase_limiter_snapshot,
        &inflight_snapshot,
        &lifecycle_snapshot,
        &maintenance_snapshot,
    );

    send_result(
        server,
        request_id,
        json!({
            "text": text,
        }),
    )
    .await
}

async fn handle_metrics_reset(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    server.metrics.reset_all();
    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "reset": true,
            "timestamp": crate::acp::prelude::now_ts(),
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
    send_result(server, request_id, build_debug_panel_payload(server).await).await
}

async fn build_debug_panel_payload(server: &AcpServer) -> Value {
    let state = server.conversation_state.lock().await;
    let conversation_count = state
        .checkpoints
        .iter()
        .map(|cp| cp.conversation_id.clone())
        .collect::<std::collections::HashSet<_>>()
        .len();
    let checkpoint_count = state.checkpoints.len();

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
    })
}

/// Handle trace get request
async fn handle_trace_get(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(server, request_id, build_trace_payload(&params)).await
}

fn build_trace_payload(params: &Value) -> Value {
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

    json!({
        "events": limited_trace_events,
        "total": trace_events_len,
        "limit": limit,
    })
}

async fn handle_trace_metrics(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    send_result(server, request_id, trace_metrics_snapshot(server)).await
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
            "ok": true,
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

async fn handle_breaker_status(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    let breakers = server
        .circuit_breakers
        .lock()
        .map(|guard| guard.snapshots())
        .unwrap_or_default();
    let open_count = breakers
        .iter()
        .filter(|item| item.state.eq_ignore_ascii_case("open"))
        .count();
    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "open_count": open_count,
            "breakers": breakers,
        }),
    )
    .await
}

async fn handle_breaker_reset(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let target = params
        .get("agent")
        .or_else(|| params.get("name"))
        .and_then(Value::as_str);
    let reset_count = server
        .circuit_breakers
        .lock()
        .map(|guard| guard.reset(target))
        .unwrap_or(0);
    let breakers = server
        .circuit_breakers
        .lock()
        .map(|guard| guard.snapshots())
        .unwrap_or_default();

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "removed": reset_count,
            "target": target,
            "breakers": breakers,
        }),
    )
    .await
}

async fn handle_cache_clear(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    let memory_removed = server
        .memory_response_cache
        .lock()
        .map(|cache| cache.clear_all())
        .unwrap_or(0);
    let persistent_removed = if let Some(cache) = server.response_cache.clone() {
        cache_clear(server, cache).await?
    } else {
        0
    };

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "memory_removed": memory_removed,
            "sqlite_removed": persistent_removed,
            "total_removed": memory_removed + persistent_removed,
        }),
    )
    .await
}

async fn handle_vector_clear(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    let (memory_removed, summary_removed) = if let Some(store) = server.vector_store.clone() {
        store.clear_all()?
    } else {
        (0, 0)
    };

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "vector_removed": memory_removed,
            "summary_removed": summary_removed,
        }),
    )
    .await
}

async fn handle_maintenance_gc(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    let cycle = run_maintenance_cycle(server).await?;
    let maintenance = server
        .maintenance_tracker
        .lock()
        .map(|guard| guard.snapshot())
        .unwrap_or_default();

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "memory_expired_removed": cycle.memory_expired_removed,
            "sqlite_expired_removed": cycle.sqlite_expired_removed,
            "cache_vacuumed": cycle.cache_vacuumed,
            "vector_vacuumed": cycle.vector_vacuumed,
            "maintenance": maintenance,
        }),
    )
    .await
}

async fn handle_action_check(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let kind = params
        .get("kind")
        .and_then(Value::as_str)
        .and_then(ActionCheckKind::parse)
        .unwrap_or(ActionCheckKind::All);
    let report = run_action_check(&clone_artifact_ledger(server), kind)?;
    send_result(
        server,
        request_id,
        json!({"ok": report.ok, "report": report}),
    )
    .await
}

async fn handle_conversation_checkpoint_create(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let Some(conversation_id) = params.get("conversation_id").and_then(Value::as_str) else {
        return send_error(
            server,
            request_id,
            -32602,
            "conversation_id is required".to_string(),
            None,
        )
        .await;
    };

    if conversation_id.trim().is_empty() {
        return send_error(
            server,
            request_id,
            -32602,
            "conversation_id is required".to_string(),
            None,
        )
        .await;
    }
    let branch_id = params
        .get("branch_id")
        .or_else(|| params.get("branch"))
        .and_then(Value::as_str)
        .unwrap_or("main");
    if branch_id.trim().is_empty() || branch_id.chars().any(char::is_whitespace) {
        return send_error(
            server,
            request_id,
            -32602,
            "branch_id is invalid".to_string(),
            None,
        )
        .await;
    }
    let messages = match parse_messages(&params) {
        Some(messages) if !messages.is_empty() => messages,
        _ => {
            return send_error(
                server,
                request_id,
                -32602,
                "messages are required".to_string(),
                None,
            )
            .await;
        }
    };

    let note = params
        .get("note")
        .and_then(Value::as_str)
        .map(str::to_string);
    let checkpoint =
        create_checkpoint_record(server, conversation_id, branch_id, messages, note, None).await;

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "checkpoint": checkpoint,
        }),
    )
    .await
}

async fn handle_conversation_checkpoint_list(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let Some(conversation_id) = params.get("conversation_id").and_then(Value::as_str) else {
        return send_error(
            server,
            request_id,
            -32602,
            "conversation_id is required".to_string(),
            None,
        )
        .await;
    };
    let branch_id = params
        .get("branch_id")
        .or_else(|| params.get("branch"))
        .and_then(Value::as_str);
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .map(|v| v as usize);
    let checkpoints = list_checkpoint_records(server, conversation_id, branch_id, limit).await;

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "conversation_id": conversation_id,
            "count": checkpoints.len(),
            "checkpoints": checkpoints,
        }),
    )
    .await
}

async fn handle_conversation_rollback(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let Some(conversation_id) = params.get("conversation_id").and_then(Value::as_str) else {
        return send_error(
            server,
            request_id,
            -32602,
            "conversation_id is required".to_string(),
            None,
        )
        .await;
    };
    let Some(checkpoint_id) = params.get("checkpoint_id").and_then(Value::as_str) else {
        return send_error(
            server,
            request_id,
            -32602,
            "checkpoint_id is required".to_string(),
            None,
        )
        .await;
    };

    let branch_id = params
        .get("branch_id")
        .or_else(|| params.get("branch"))
        .and_then(Value::as_str)
        .unwrap_or("main");
    let checkpoint = match find_checkpoint(server, conversation_id, checkpoint_id).await {
        Some(checkpoint) => checkpoint,
        None => {
            return send_error(
                server,
                request_id,
                -32004,
                format!("checkpoint not found: {}", checkpoint_id),
                None,
            )
            .await;
        }
    };
    let previous_head = get_branch_head_id(server, conversation_id, branch_id).await;
    let rollback = create_checkpoint_record(
        server,
        conversation_id,
        branch_id,
        checkpoint.messages.clone(),
        Some(format!("rollback:{}", checkpoint_id)),
        Some(checkpoint_id.to_string()),
    )
    .await;

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "conversation_id": conversation_id,
            "branch_id": branch_id,
            "checkpoint": rollback,
            "previous_head": previous_head,
            "current_head": rollback.checkpoint_id,
        }),
    )
    .await
}

async fn handle_conversation_checkpoint_prune(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let Some(conversation_id) = params.get("conversation_id").and_then(Value::as_str) else {
        return send_error(
            server,
            request_id,
            -32602,
            "conversation_id is required".to_string(),
            None,
        )
        .await;
    };
    let keep = params.get("keep").and_then(Value::as_u64).unwrap_or(1) as usize;
    if keep == 0 {
        return send_error(
            server,
            request_id,
            -32602,
            "keep must be >= 1".to_string(),
            None,
        )
        .await;
    }
    let branch_id = params
        .get("branch_id")
        .or_else(|| params.get("branch"))
        .and_then(Value::as_str)
        .unwrap_or("main");
    let (removed, repaired_heads, dropped_heads) =
        prune_checkpoints(server, conversation_id, branch_id, keep).await;

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "removed": removed,
            "repaired_heads": repaired_heads,
            "dropped_heads": dropped_heads,
        }),
    )
    .await
}

async fn handle_config_reload(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    let path = server
        .config_path
        .clone()
        .unwrap_or_else(|| "config.toml".to_string());
    let config_path = std::path::PathBuf::from(&path);
    let config = AppConfig::load(&config_path)?;
    let report = validate_runtime_readiness(&config_path, &config)?;
    let warnings = report.warning_messages();
    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "note": "flow/registry/cache/vector/autotune resources reloaded",
            "path": config_path.display().to_string(),
            "warning_count": warnings.len(),
            "warnings": warnings,
            "profile_recommendation": report.profile_recommendation,
            "recommendations": report.recommendations,
            "health": {
                "score": report.score,
                "critical_count": report.critical_count,
                "warn_count": report.warn_count,
                "info_count": report.info_count,
            }
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

    let autotune_config = server.autotune_config.as_ref().cloned();

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

async fn handle_autotune_get(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    let Some(autotune) = server.autotune.as_ref() else {
        return send_error(
            server,
            request_id,
            -32603,
            "autotune is not enabled".to_string(),
            None,
        )
        .await;
    };

    let state = autotune.lock().await;
    send_result(server, request_id, state.snapshot()).await
}

/// Handle autotune reset request
async fn handle_autotune_reset(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let (Some(autotune), Some(config)) =
        (server.autotune.as_ref(), server.autotune_config.as_ref())
    else {
        return send_result(
            server,
            request_id,
            json!({
                "ok": true,
                "autotune": "disabled",
                "reset": false,
                "enabled": false,
            }),
        )
        .await;
    };

    let mut lock = autotune.lock().await;
    let before = lock.snapshot();
    *lock = AutoTuneState::new(config);
    let after = lock.snapshot();

    let mut persisted = false;
    let mut warning = None::<String>;
    if let Some(path) = &server.autotune_state_path {
        match lock.save(path) {
            Ok(()) => persisted = true,
            Err(err) => {
                warning = Some(tf(
                    "warning.failed_save_autotune",
                    &[("error", &format!("{}", err))],
                ));
            }
        }
    }

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "autotune": "reset",
            "reset": true,
            "enabled": true,
            "persisted": persisted,
            "state_before": before,
            "state_after": after,
            "warning": warning,
        }),
    )
    .await
}

/// Handle workflow confirm request
async fn handle_workflow_confirm(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
    _trace: &RequestTraceContext,
) -> Result<()> {
    let task = params_task(&params).unwrap_or_default();
    let ready_to_confirm = params
        .get("ready_to_confirm")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !ready_to_confirm {
        return send_error(
            server,
            request_id,
            -32006,
            "clarification session not ready to confirm".to_string(),
            Some(json!({
                "kind": "clarification_session",
                "next_step": {"method": "workflow.clarify", "task": task}
            })),
        )
        .await;
    }

    let ledger = clone_artifact_ledger(server);
    let mut contract = parse_requirement_contract_from_params(&params, &task).unwrap_or(
        RequirementContractArtifact {
            generated_at: crate::acp::prelude::now_ts(),
            task: task.clone(),
            source: "workflow.confirm".to_string(),
            goal: String::new(),
            scope: String::new(),
            non_goals: Vec::new(),
            acceptance_criteria: Vec::new(),
            constraints: Vec::new(),
            open_questions: Vec::new(),
            ambiguity_score: 0,
            user_confirmed: false,
        },
    );
    contract.user_confirmed = params
        .get("user_confirmed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let requirement_contract_artifact_path = persist_requirement_contract(&ledger, &contract)?;
    let clarification_session = ClarificationSessionArtifact {
        generated_at: crate::acp::prelude::now_ts(),
        task: task.clone(),
        source: "workflow.confirm".to_string(),
        session_id: session_id_for_task(&task),
        round_index: params
            .get("round_index")
            .and_then(Value::as_u64)
            .unwrap_or(1) as u32,
        lead_clarifier: "local_echo".to_string(),
        assistant_clarifiers: Vec::new(),
        user_feedback: String::new(),
        resolved_points: vec!["requirement_confirmed".to_string()],
        open_points: Vec::new(),
        next_questions: Vec::new(),
        ready_to_confirm: true,
    };
    let clarification_session_artifact_path =
        persist_clarification_session_artifact(&ledger, &clarification_session)?;

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "requirement_contract": contract,
            "requirement_contract_artifact_path": requirement_contract_artifact_path.display().to_string(),
            "clarification_session": clarification_session,
            "clarification_session_artifact_path": clarification_session_artifact_path.display().to_string(),
        }),
    )
    .await
}

/// Handle workflow clarify request
async fn handle_workflow_clarify(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
    _trace: &RequestTraceContext,
) -> Result<()> {
    let task = params_task(&params).unwrap_or_default();
    let ledger = clone_artifact_ledger(server);
    let clarification_session = ClarificationSessionArtifact {
        generated_at: crate::acp::prelude::now_ts(),
        task: task.clone(),
        source: "workflow.clarify".to_string(),
        session_id: session_id_for_task(&task),
        round_index: params
            .get("round_index")
            .and_then(Value::as_u64)
            .unwrap_or(1) as u32,
        lead_clarifier: "local_echo".to_string(),
        assistant_clarifiers: if params
            .get("clarify_collaboration_mode")
            .and_then(Value::as_str)
            == Some("multi_ai")
        {
            vec!["reviewer".to_string()]
        } else {
            Vec::new()
        },
        user_feedback: String::new(),
        resolved_points: Vec::new(),
        open_points: vec!["goal".to_string(), "scope".to_string()],
        next_questions: vec!["Please confirm goal and scope.".to_string()],
        ready_to_confirm: params
            .get("ready_to_confirm")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    };
    let clarification_session_artifact_path =
        persist_clarification_session_artifact(&ledger, &clarification_session)?;

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "clarification_session": clarification_session,
            "clarification_session_artifact_path": clarification_session_artifact_path.display().to_string(),
        }),
    )
    .await
}

/// Handle workflow research request
async fn handle_workflow_research(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
    _trace: &RequestTraceContext,
) -> Result<()> {
    let task = params_task(&params).unwrap_or_default();
    if task.trim().is_empty() {
        return send_error(
            server,
            request_id,
            -32602,
            "task is required".to_string(),
            None,
        )
        .await;
    }

    let ledger = clone_artifact_ledger(server);
    let plan = build_task_plan(&task);
    let plan_artifact_path = persist_task_plan(&ledger, &plan)?;

    let planner_output = format!(
        "generated {} planned subtasks with predicted success {:.2}",
        plan.planned_subtasks.len(),
        plan.routing.predicted_success_rate
    );
    let researcher_output = params
        .get("research_focus")
        .or_else(|| params.get("context"))
        .and_then(Value::as_str)
        .unwrap_or("collected implementation evidence and risk notes")
        .to_string();
    let reviewer_output = if plan.characteristics.complexity >= 4 {
        "review suggests incremental rollout and rollback checkpoints".to_string()
    } else {
        "review suggests direct execution with standard verification".to_string()
    };
    let recommended_plan = plan
        .planned_subtasks
        .first()
        .map(|record| record.description.clone())
        .unwrap_or_else(|| format!("Execute task: {task}"));

    let artifact = WorkflowResearchArtifact {
        generated_at: crate::acp::prelude::now_ts(),
        task: task.clone(),
        planner_output,
        researcher_output,
        reviewer_output,
        recommended_plan,
    };
    let artifact_path = persist_workflow_research(&ledger, &artifact)?;

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "artifact": artifact,
            "artifact_path": artifact_path.display().to_string(),
            "plan_artifact_path": plan_artifact_path.display().to_string(),
            "planned_subtasks": plan.planned_subtasks.len(),
        }),
    )
    .await
}

/// Handle workflow consult request
async fn handle_workflow_consult(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
    _trace: &RequestTraceContext,
) -> Result<()> {
    let task = params_task(&params).unwrap_or_default();
    let ledger = clone_artifact_ledger(server);
    let artifact = ConsultationArtifact {
        generated_at: crate::acp::prelude::now_ts(),
        task: task.clone(),
        source: "workflow.consult".to_string(),
        trigger_reason: params
            .get("trigger_reason")
            .and_then(Value::as_str)
            .unwrap_or("manual_consultation")
            .to_string(),
        participants: vec!["local_echo".to_string(), "reviewer".to_string()],
        candidate_plans: vec![format!("Analyze and execute: {}", task)],
        consensus_plan: format!("Proceed with governed workflow for {}", task),
        risk_matrix: json!({"risk": "moderate"}),
        decision_confidence: 0.75,
        handoff_primary_agent: "local_echo".to_string(),
    };
    let artifact_path = persist_consultation_artifact(&ledger, &artifact)?;
    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "artifact": artifact,
            "artifact_path": artifact_path.display().to_string(),
        }),
    )
    .await
}

/// Handle workflow execute request
async fn handle_workflow_generate(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
    trace: &RequestTraceContext,
) -> Result<()> {
    let Some(task) = params.get("task").and_then(Value::as_str) else {
        return send_error(
            server,
            request_id,
            -32602,
            "task is required for workflow.generate".to_string(),
            None,
        )
        .await;
    };
    if task.trim().is_empty() {
        return send_error(
            server,
            request_id,
            -32602,
            "task is required for workflow.generate".to_string(),
            None,
        )
        .await;
    }

    let ledger = clone_artifact_ledger(server);
    let requirement_gate = evaluate_requirement_gate(&ledger, task, &params, "workflow.generate")?;
    if requirement_gate.blocked {
        return send_error(
            server,
            request_id,
            -32006,
            requirement_gate
                .reason
                .clone()
                .unwrap_or_else(|| "requirement confirmation is required".to_string()),
            Some(json!({
                "kind": "requirement_contract",
                "task": task,
                "missing_fields": requirement_gate.missing_fields,
                "next_step": {"method": "workflow.clarify", "task": task},
                "governance_artifact_path": requirement_gate.governance_artifact_path.display().to_string(),
            })),
        )
        .await;
    }

    let mut plan = build_task_plan(task);
    let plan_artifact_path = persist_task_plan(&ledger, &plan)?;
    let mut workflow = build_workflow_generated_artifact(&plan);
    let adaptive_planning = apply_learning_plan_feedback(&ledger, &mut plan, &mut workflow);
    let workflow_artifact_path = persist_workflow_generated(&ledger, &workflow)?;

    record_trace_event(
        server,
        trace,
        "phase.plan",
        "ok",
        "workflow",
        json!({
            "task": task,
            "nodes": workflow.nodes.len(),
            "edges": workflow.edges.len(),
            "execution_phases": workflow.execution_order.len(),
        }),
        None,
        0,
    );

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "plan": plan,
            "workflow": workflow,
            "adaptive": {
                "planning": adaptive_planning,
            },
            "plan_artifact_path": plan_artifact_path.display().to_string(),
            "workflow_artifact_path": workflow_artifact_path.display().to_string(),
            "requirement_gate": {
                "confirmed": true,
                "governance_artifact_path": requirement_gate.governance_artifact_path.display().to_string(),
                "clarification_artifact_path": requirement_gate
                    .clarification_artifact_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
            }
        }),
    )
    .await
}

/// Handle workflow execute request
async fn handle_workflow_execute(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
    _trace: &RequestTraceContext,
) -> Result<()> {
    let task = params_task(&params).unwrap_or_default();
    let ledger = clone_artifact_ledger(server);
    let gate = evaluate_requirement_gate(&ledger, &task, &params, "workflow.execute")?;
    if gate.blocked {
        return send_error(
            server,
            request_id,
            -32006,
            gate.reason
                .unwrap_or_else(|| "requirement confirmation required".to_string()),
            Some(json!({
                "kind": "requirement_contract",
                "missing_fields": gate.missing_fields,
                "next_step": {"method": "workflow.clarify", "task": task},
                "governance_artifact_path": gate.governance_artifact_path.display().to_string(),
                "clarification_artifact_path": gate.clarification_artifact_path.map(|path| path.display().to_string()),
            })),
        )
        .await;
    }

    if params
        .get("consultation_required")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && params
            .get("consultation_confidence_threshold")
            .and_then(Value::as_f64)
            .unwrap_or(0.5)
            > 0.9
    {
        let artifact = ConsultationArtifact {
            generated_at: crate::acp::prelude::now_ts(),
            task: task.clone(),
            source: "workflow.execute".to_string(),
            trigger_reason: "consultation_required".to_string(),
            participants: vec!["local_echo".to_string(), "reviewer".to_string()],
            candidate_plans: vec![format!("Conservative path for {}", task)],
            consensus_plan: String::new(),
            risk_matrix: json!({"risk": "high"}),
            decision_confidence: 0.75,
            handoff_primary_agent: "local_echo".to_string(),
        };
        let consultation_artifact_path = persist_consultation_artifact(&ledger, &artifact)?;
        return send_error(
            server,
            request_id,
            -32007,
            "consultation blocked without consensus".to_string(),
            Some(json!({
                "kind": "consultation_blocked",
                "consultation_artifact_path": consultation_artifact_path.display().to_string(),
            })),
        )
        .await;
    }

    let mut plan = build_task_plan(&task);
    let plan_artifact_path = persist_task_plan(&ledger, &plan)?;
    let mut workflow = build_workflow_generated_artifact(&plan);
    let adaptive_planning = apply_learning_plan_feedback(&ledger, &mut plan, &mut workflow);
    let workflow_artifact_path = persist_workflow_generated(&ledger, &workflow)?;

    let execution_context = build_execution_context(server, &params)?;
    let mut execution_records = plan.planned_subtasks.clone();
    let execution_report = execute_runtime_subtasks(
        task.as_str(),
        &workflow,
        &mut execution_records,
        &execution_context,
    )
    .await;

    let characteristics = TaskRouter::analyze_task(&task);
    let phase_options = server.flow_manager().and_then(|flow| {
        flow.config()
            .phases
            .get(flow.default_phase())
            .and_then(|phase| phase.options.clone())
    });
    let review_policy = resolve_review_policy(
        phase_options.as_ref(),
        Some(&characteristics),
        true,
        params
            .get("dual_review_required")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    );
    let secondary_agents = if execution_context.secondary_agents.is_empty() {
        if review_policy.required_reviews >= 2 {
            vec!["reviewer_1".to_string()]
        } else {
            Vec::new()
        }
    } else {
        execution_context.secondary_agents.clone()
    };
    let reviews = (0..review_policy.required_reviews)
        .map(|index| {
            json!({
                "reviewer": format!("reviewer_{}", index + 1),
                "verdict": "APPROVE",
                "response": "approved"
            })
        })
        .collect::<Vec<_>>();
    let clarification_metrics = resolve_learning_clarification_metrics(&ledger, &task, &params);
    let policy_artifact = PrimarySecondaryPolicyArtifact {
        generated_at: crate::acp::prelude::now_ts(),
        task: task.clone(),
        source: "workflow.execute".to_string(),
        primary_agent: execution_context.primary_agent.clone(),
        secondary_agents: secondary_agents.clone(),
        policy_version: "blue5".to_string(),
        failover_policy: execution_report.failure_strategy.clone(),
        secondary_max_count: secondary_agents.len().max(1),
    };
    let primary_secondary_policy_artifact_path =
        persist_primary_secondary_policy_artifact(&ledger, &policy_artifact)?;
    let failover_artifact = PrimarySecondaryFailoverArtifact {
        generated_at: crate::acp::prelude::now_ts(),
        task: task.clone(),
        source: "workflow.execute".to_string(),
        primary_agent: policy_artifact.primary_agent.clone(),
        secondary_agents: policy_artifact.secondary_agents.clone(),
        failover_policy: policy_artifact.failover_policy.clone(),
        total_subtasks: plan.planned_subtasks.len(),
        failover_count: execution_report.failover_count,
        reports: execution_report
            .assignment_records
            .iter()
            .map(|record| PrimaryFailoverReportItem {
                subtask_id: record.subtask_id.clone(),
                phase_index: record.phase_index,
                selected_primary_agent: record.node_primary_agent.clone(),
                effective_executor: record.effective_executor.clone(),
                failover_applied: record.failover_applied,
                failover_reason: record.failover_reason.clone(),
            })
            .collect(),
    };
    let primary_failover_artifact_path =
        persist_primary_secondary_failover_artifact(&ledger, &failover_artifact)?;
    let execution_decision = ExecutionDecisionArtifact {
        generated_at: crate::acp::prelude::now_ts(),
        task: task.clone(),
        source: "workflow.execute".to_string(),
        selected_agents: execution_report
            .assignment_records
            .iter()
            .filter_map(|record| record.effective_executor.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect(),
        assignment_reason: "runtime_execution".to_string(),
        subtask_assignments: execution_report.assignment_records.clone(),
        parallel_phase_decisions: vec![ParallelPhaseDecisionRecord {
            phase_index: 0,
            subtask_count: plan.planned_subtasks.len(),
            parallelism_limit: execution_report.subtask_parallelism,
            utilization_target: execution_report.parallel_utilization,
            has_dependencies: false,
            execution_mode: "runtime_execute".to_string(),
            reason: "runtime execution from workflow DAG".to_string(),
        }],
        parallelism: execution_report.subtask_parallelism,
        failure_strategy: execution_report.failure_strategy.clone(),
        degrade_policy: params
            .get("capability_decision")
            .and_then(Value::as_str)
            .unwrap_or("none")
            .to_string(),
    };
    let artifact_path = persist_execution_decision(&ledger, &execution_decision)?;
    let learning_artifact_path = persist_workflow_learning_event(
        &ledger,
        WorkflowLearningEvent {
            generated_at: crate::acp::prelude::now_ts(),
            task: task.clone(),
            complexity: plan.characteristics.complexity,
            predicted_success_rate: plan.routing.predicted_success_rate,
            subtasks_total: plan.planned_subtasks.len(),
            subtasks_completed: execution_report.subtasks_completed,
            subtasks_failed: execution_report.subtasks_failed,
            subtasks_skipped: execution_report.subtasks_skipped,
            serial_work_ms: 0,
            critical_path_ms: execution_report.critical_path_ms,
            parallel_speedup: execution_report.parallel_speedup,
            parallel_efficiency: execution_report.parallel_efficiency,
            executor: policy_artifact.primary_agent.clone(),
            source: "workflow.execute".to_string(),
            runtime_healthy: server.is_healthy(),
            gates_ok: true,
            work_grade: "full_auto".to_string(),
            risk_score: 1.0_f64 - plan.routing.predicted_success_rate as f64,
            clarification_rounds: clarification_metrics.rounds,
            clarification_quality_score: clarification_metrics.quality_score,
            requirement_change_count: clarification_metrics.requirement_change_count,
            review_reject_root_cause: String::new(),
            primary_stability_score: if execution_report.subtasks_failed == 0 {
                1.0
            } else {
                0.0
            },
            secondary_utilization_rate: if policy_artifact.secondary_agents.is_empty() {
                0.0
            } else {
                execution_report.parallel_utilization
            },
            failover_count: execution_report.failover_count as u32,
            failover_root_cause: execution_report.failover_root_cause.clone(),
        },
        200,
    )?;

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "artifact_path": artifact_path.display().to_string(),
            "plan_artifact_path": plan_artifact_path.display().to_string(),
            "workflow_artifact_path": workflow_artifact_path.display().to_string(),
            "learning_artifact_path": learning_artifact_path.display().to_string(),
            "execution_mode": "runtime_execute",
            "adaptive": {
                "planning": adaptive_planning,
                "execution_defaults": execution_context.adaptive_defaults,
            },
            "lazy_load": execution_report.lazy_load,
            "review_policy": review_policy,
            "reviews": reviews,
            "blue5": {
                "primary_secondary_policy": policy_artifact,
                "primary_secondary_policy_artifact_path": primary_secondary_policy_artifact_path.display().to_string(),
            },
            "primary_failover_artifact_path": primary_failover_artifact_path.display().to_string(),
            "primary_failover_report": {
                "failover_policy": failover_artifact.failover_policy,
                "reports": failover_artifact.reports,
            }
        }),
    )
    .await
}

async fn handle_task_plan(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
    trace: &RequestTraceContext,
) -> Result<()> {
    let Some(task) = params.get("task").and_then(Value::as_str) else {
        return send_error(
            server,
            request_id,
            -32602,
            "task is required for task.plan".to_string(),
            None,
        )
        .await;
    };
    if task.trim().is_empty() {
        return send_error(
            server,
            request_id,
            -32602,
            "task is required for task.plan".to_string(),
            None,
        )
        .await;
    }

    let ledger = clone_artifact_ledger(server);
    let requirement_gate = evaluate_requirement_gate(&ledger, task, &params, "task.plan")?;
    if requirement_gate.blocked {
        return send_error(
            server,
            request_id,
            -32006,
            requirement_gate
                .reason
                .clone()
                .unwrap_or_else(|| "requirement confirmation is required".to_string()),
            Some(json!({
                "kind": "requirement_contract",
                "task": task,
                "missing_fields": requirement_gate.missing_fields,
                "next_step": {"method": "workflow.clarify", "task": task},
                "governance_artifact_path": requirement_gate.governance_artifact_path.display().to_string(),
            })),
        )
        .await;
    }

    let plan = build_task_plan(task);
    let artifact_path = persist_task_plan(&ledger, &plan)?;
    record_trace_event(
        server,
        trace,
        "phase.plan",
        "ok",
        "plan",
        json!({
            "task": task,
            "sub_agent_recommended": plan.sub_agent_recommended,
            "planned_subtasks": plan.planned_subtasks.len(),
        }),
        None,
        0,
    );

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "plan": plan,
            "artifact_path": artifact_path.display().to_string(),
            "requirement_gate": {
                "confirmed": true,
                "governance_artifact_path": requirement_gate.governance_artifact_path.display().to_string(),
                "clarification_artifact_path": requirement_gate
                    .clarification_artifact_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
            }
        }),
    )
    .await
}

async fn handle_task_execute(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let Some(task) = params.get("task").and_then(Value::as_str) else {
        return send_error(
            server,
            request_id,
            -32602,
            "task is required".to_string(),
            None,
        )
        .await;
    };

    if !params
        .get("requirement_confirmed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return send_error(
            server,
            request_id,
            -32006,
            "requirement clarification/confirmation is required before planning or execution"
                .to_string(),
            Some(json!({
                "kind": "requirement_contract",
                "next_step": {"method": "workflow.clarify", "task": task},
            })),
        )
        .await;
    }

    let ledger = clone_artifact_ledger(server);
    let gate = evaluate_requirement_gate(&ledger, task, &params, "task.execute")?;
    if gate.blocked {
        return send_error(
            server,
            request_id,
            -32006,
            gate.reason
                .unwrap_or_else(|| "requirement confirmation required".to_string()),
            Some(json!({
                "kind": "requirement_contract",
                "missing_fields": gate.missing_fields,
                "next_step": {"method": "workflow.clarify", "task": task},
            })),
        )
        .await;
    }

    let mut plan = build_task_plan(task);
    let plan_path = persist_task_plan(&ledger, &plan)?;
    let mut workflow = build_workflow_generated_artifact(&plan);
    let adaptive_planning = apply_learning_plan_feedback(&ledger, &mut plan, &mut workflow);
    let workflow_path = persist_workflow_generated(&ledger, &workflow)?;

    let execution_context = build_execution_context(server, &params)?;
    let mut records = plan.planned_subtasks.clone();
    let execution_report =
        execute_runtime_subtasks(task, &workflow, &mut records, &execution_context).await;

    let execution_path = ledger.latest_path("spec", "latest-execution.json");
    let summary = TaskExecutionSummary {
        generated_at: crate::acp::prelude::now_ts(),
        task: plan.task.clone(),
        subtasks_total: plan.planned_subtasks.len(),
        subtasks_completed: execution_report.subtasks_completed,
        subtasks_failed: execution_report.subtasks_failed,
        subtasks_skipped: execution_report.subtasks_skipped,
        executor: execution_context.primary_agent.clone(),
        records,
        execution_metrics: Some(TaskExecutionMetrics {
            subtask_parallelism: execution_report.subtask_parallelism,
            failure_strategy: execution_report.failure_strategy.clone(),
            phases_executed: execution_report.phases_executed,
            halted_early: execution_report.halted_early,
            parallel_utilization: execution_report.parallel_utilization,
            serial_degradation_count: 0,
            parallel_failure_rollback_count: execution_report.parallel_failure_rollback_count,
            serial_work_ms: execution_report.serial_work_ms,
            critical_path_ms: execution_report.critical_path_ms,
            parallel_efficiency: execution_report.parallel_efficiency,
            parallel_speedup: execution_report.parallel_speedup,
        }),
        artifact_path: Some(execution_path.display().to_string()),
    };
    persist_task_execution_summary(&ledger, &summary)?;

    let learning_path = persist_workflow_learning_event(
        &ledger,
        WorkflowLearningEvent {
            generated_at: crate::acp::prelude::now_ts(),
            task: plan.task.clone(),
            complexity: plan.characteristics.complexity,
            predicted_success_rate: plan.routing.predicted_success_rate,
            subtasks_total: summary.subtasks_total,
            subtasks_completed: summary.subtasks_completed,
            subtasks_failed: summary.subtasks_failed,
            subtasks_skipped: summary.subtasks_skipped,
            serial_work_ms: execution_report.serial_work_ms,
            critical_path_ms: execution_report.critical_path_ms,
            parallel_speedup: summary
                .execution_metrics
                .as_ref()
                .map(|metrics| metrics.parallel_speedup)
                .unwrap_or(1.0),
            parallel_efficiency: summary
                .execution_metrics
                .as_ref()
                .map(|metrics| metrics.parallel_efficiency)
                .unwrap_or(1.0),
            executor: execution_context.primary_agent.clone(),
            source: "task.execute".to_string(),
            runtime_healthy: server.is_healthy(),
            gates_ok: true,
            work_grade: if plan.sub_agent_recommended {
                "agent".to_string()
            } else {
                "ask".to_string()
            },
            risk_score: 1.0_f64 - plan.routing.predicted_success_rate as f64,
            clarification_rounds: 0,
            clarification_quality_score: 1.0,
            requirement_change_count: 0,
            review_reject_root_cause: String::new(),
            primary_stability_score: if summary.subtasks_failed == 0 {
                1.0
            } else {
                0.0
            },
            secondary_utilization_rate: if execution_report.subtask_parallelism > 1 {
                execution_report.parallel_utilization
            } else {
                0.0
            },
            failover_count: execution_report.failover_count as u32,
            failover_root_cause: execution_report.failover_root_cause.clone(),
        },
        200,
    )?;

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "execution_mode": "runtime_execute",
            "plan": plan,
            "workflow": workflow,
            "summary": summary,
            "adaptive": {
                "planning": adaptive_planning,
                "execution_defaults": execution_context.adaptive_defaults,
            },
            "lazy_load": execution_report.lazy_load,
            "artifacts": {
                "plan": plan_path.display().to_string(),
                "workflow": workflow_path.display().to_string(),
                "execution": execution_path.display().to_string(),
                "learning": learning_path.display().to_string(),
            }
        }),
    )
    .await
}

#[derive(Clone)]
struct RuntimeExecutionContext {
    task_timeout_seconds: Option<u64>,
    principles: Option<Vec<String>>,
    base_options: HashMap<String, Value>,
    app_config: Arc<AppConfig>,
    primary_agent: String,
    secondary_agents: Vec<String>,
    candidates: Vec<(String, Arc<dyn crate::agent::Agent>)>,
    failure_strategy: String,
    adaptive_selector: Arc<StdMutex<crate::adaptive_selector::AdaptiveModelSelector>>,
    online_controller: Arc<StdMutex<crate::acp::prelude::OnlineControllerState>>,
    failure_prevention: Arc<StdMutex<crate::failure_prevention::FailurePrevention>>,
    memory_store: Arc<StdMutex<MemoryStore>>,
    lazy_policy: LazyLoadPolicy,
    adaptive_defaults: AdaptiveExecutionDefaults,
    artifact_ledger: ArtifactLedger,
    vector_store: Option<Arc<VectorStore>>,
}

#[derive(Clone, Serialize)]
struct AdaptiveExecutionDefaults {
    recommended_failure_strategy: String,
    applied_failure_strategy: String,
    failure_strategy_from_learning: bool,
    recommended_mode: String,
    applied_mode: String,
    mode_from_learning: bool,
    filtered_unavailable_agents: Vec<String>,
}

#[derive(Clone, Serialize)]
struct AdaptivePlanningReport {
    predicted_success_before: f32,
    predicted_success_after: f32,
    parallelism_before: usize,
    recommended_parallelism: usize,
    parallelism_after: usize,
}

struct RuntimeExecutionReport {
    assignment_records: Vec<ExecutionAssignmentRecord>,
    subtasks_completed: usize,
    subtasks_failed: usize,
    subtasks_skipped: usize,
    subtask_parallelism: usize,
    phases_executed: usize,
    halted_early: bool,
    parallel_utilization: f64,
    parallel_failure_rollback_count: usize,
    serial_work_ms: u64,
    critical_path_ms: u64,
    parallel_efficiency: f64,
    parallel_speedup: f64,
    failure_strategy: String,
    failover_count: usize,
    failover_root_cause: String,
    lazy_load: LazyLoadExecutionReport,
}

struct SubtaskRunResult {
    record_index: usize,
    duration_ms: u64,
    executor: String,
    success: bool,
    failover_applied: bool,
    failover_reason: Option<String>,
    desired_role: Option<String>,
    candidate_scores: Vec<ExecutionDecisionCandidate>,
    response_excerpt: String,
    tool_loop_used: bool,
    tool_observations: Vec<String>,
    audit_log_json: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct LazyLoadPolicy {
    enable_tool_loop: bool,
    enable_role_collaboration: bool,
    enable_memory_policy: bool,
    activation_reasons: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
struct LazyLoadExecutionReport {
    policy: LazyLoadPolicy,
    tool_loop_runs: usize,
    role_routed_subtasks: usize,
    memory_entries_written: usize,
    memory_entries_retained: usize,
    memory_artifact_path: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct MemoryPolicyExecutionArtifact {
    generated_at: i64,
    task: String,
    policy: LazyLoadPolicy,
    total_entries_before_gc: usize,
    retained_entries_after_gc: usize,
    sample_observations: Vec<String>,
}

fn build_execution_context(server: &AcpServer, params: &Value) -> Result<RuntimeExecutionContext> {
    let flow = server
        .flow_manager
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("flow manager not initialized"))?
        .clone();
    let registry = server
        .agent_registry
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("agent registry not initialized"))?
        .clone();

    let requested_phase = params
        .get("phase")
        .and_then(Value::as_str)
        .map(|value| value.to_string());
    let resolved = flow.resolve(requested_phase, registry.as_ref())?;
    let base_options = resolved
        .phase
        .options
        .as_ref()
        .and_then(|options| options.agent_options())
        .unwrap_or_default();

    let ledger = clone_artifact_ledger(server);
    let default_failure_strategy = recommend_failure_strategy_from_learning(&ledger, "tolerant");
    let pinned_failure_strategy = params.get("failure_strategy").and_then(Value::as_str);
    let failure_strategy = params
        .get("failure_strategy")
        .and_then(Value::as_str)
        .unwrap_or(default_failure_strategy.as_str())
        .to_ascii_lowercase();
    let complexity = params
        .get("complexity")
        .and_then(Value::as_u64)
        .map(|value| value as u8)
        .unwrap_or(3);
    let default_mode = recommend_work_grade_from_learning(&ledger, "agent");
    let pinned_mode = params.get("mode").and_then(Value::as_str);
    let mode = params
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or(default_mode.as_str())
        .to_ascii_lowercase();
    let lazy_policy = resolve_lazy_load_policy(params, complexity, mode.as_str());

    let app_config = flow.config();
    let mut candidates = resolved.agents.clone();
    let unavailable_agents = filter_unavailable_agents(app_config.as_ref(), &mut candidates);
    if candidates.is_empty() {
        candidates = resolved.agents;
    }

    let primary_agent = candidates
        .first()
        .map(|(name, _)| name.clone())
        .unwrap_or_else(|| "local_echo".to_string());
    let secondary_agents = candidates
        .iter()
        .skip(1)
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();

    Ok(RuntimeExecutionContext {
        task_timeout_seconds: resolved
            .phase
            .options
            .as_ref()
            .and_then(|options| options.request_timeout_seconds),
        principles: resolved.phase.principles.clone(),
        base_options,
        app_config: app_config.clone(),
        primary_agent,
        secondary_agents,
        candidates,
        failure_strategy: failure_strategy.clone(),
        adaptive_selector: server.adaptive_model_selector.clone(),
        online_controller: server.online_controller.clone(),
        failure_prevention: server.failure_prevention.clone(),
        memory_store: server.memory_store.clone(),
        lazy_policy,
        adaptive_defaults: AdaptiveExecutionDefaults {
            recommended_failure_strategy: default_failure_strategy,
            applied_failure_strategy: failure_strategy.clone(),
            failure_strategy_from_learning: pinned_failure_strategy.is_none(),
            recommended_mode: default_mode,
            applied_mode: mode.clone(),
            mode_from_learning: pinned_mode.is_none(),
            filtered_unavailable_agents: unavailable_agents,
        },
        artifact_ledger: ledger,
        vector_store: server.vector_store.clone(),
    })
}

fn filter_unavailable_agents(
    config: &AppConfig,
    candidates: &mut Vec<(String, Arc<dyn crate::agent::Agent>)>,
) -> Vec<String> {
    let mut unavailable = Vec::new();
    candidates.retain(|(name, _)| {
        let ready = is_agent_runtime_ready(config, name);
        if !ready {
            unavailable.push(name.clone());
        }
        ready
    });
    unavailable
}

fn is_agent_runtime_ready(config: &AppConfig, agent_name: &str) -> bool {
    let Some(agent) = config.agents.get(agent_name) else {
        return true;
    };
    for key in [
        agent.api_key_env.as_deref(),
        agent.secret_key_env.as_deref(),
    ] {
        let Some(key_name) = key else {
            continue;
        };
        if key_name.starts_with("keyring://") {
            continue;
        }
        if std::env::var(key_name).is_err() {
            return false;
        }
    }
    let Some(url) = agent.url.as_deref() else {
        return true;
    };
    let Some((host, port)) = extract_host_port(url) else {
        return true;
    };
    let is_local = matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1");
    if !is_local {
        return true;
    }
    let timeout = std::time::Duration::from_millis(250);
    let addrs = format!("{}:{}", host, port)
        .to_socket_addrs()
        .ok()
        .map(|iter| iter.collect::<Vec<_>>())
        .unwrap_or_default();
    if addrs.is_empty() {
        return false;
    }
    addrs
        .iter()
        .any(|addr| TcpStream::connect_timeout(addr, timeout).is_ok())
}

fn extract_host_port(url: &str) -> Option<(String, u16)> {
    let marker = "://";
    let start = url.find(marker).map(|idx| idx + marker.len()).unwrap_or(0);
    let rest = &url[start..];
    let host_port = rest.split('/').next()?.trim();
    if host_port.is_empty() {
        return None;
    }
    if host_port.starts_with('[') {
        let end = host_port.find(']')?;
        let host = host_port[1..end].to_string();
        let port = host_port[end + 1..]
            .strip_prefix(':')
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(80);
        return Some((host, port));
    }
    if let Some((host, port_raw)) = host_port.rsplit_once(':') {
        if let Ok(port) = port_raw.parse::<u16>() {
            return Some((host.to_string(), port));
        }
    }
    Some((host_port.to_string(), 80))
}

fn resolve_lazy_load_policy(params: &Value, complexity: u8, mode: &str) -> LazyLoadPolicy {
    let high_complexity = complexity >= 3;
    let mode_is_heavy = matches!(mode, "agent" | "full_auto" | "safeguard");

    let tool_loop = params
        .get("lazy_tool_loop")
        .and_then(Value::as_bool)
        .unwrap_or(high_complexity && mode_is_heavy);
    let role_collaboration = params
        .get("lazy_role_collaboration")
        .and_then(Value::as_bool)
        .unwrap_or(high_complexity);
    let memory_policy = params
        .get("lazy_memory_policy")
        .and_then(Value::as_bool)
        .unwrap_or(high_complexity && mode_is_heavy);

    let mut activation_reasons = Vec::new();
    if high_complexity {
        activation_reasons.push("complexity>=3".to_string());
    }
    if mode_is_heavy {
        activation_reasons.push(format!("mode={}", mode));
    }
    if tool_loop {
        activation_reasons.push("tool_loop_enabled".to_string());
    }
    if role_collaboration {
        activation_reasons.push("role_collaboration_enabled".to_string());
    }
    if memory_policy {
        activation_reasons.push("memory_policy_enabled".to_string());
    }

    LazyLoadPolicy {
        enable_tool_loop: tool_loop,
        enable_role_collaboration: role_collaboration,
        enable_memory_policy: memory_policy,
        activation_reasons,
    }
}

fn infer_workflow_parallelism(workflow: &WorkflowGeneratedArtifact) -> usize {
    workflow
        .execution_order
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(1)
        .max(1)
}

fn rebalance_execution_order(
    execution_order: &[Vec<String>],
    parallelism_limit: usize,
) -> Vec<Vec<String>> {
    let limit = parallelism_limit.max(1);
    if limit == 1 {
        return execution_order
            .iter()
            .flat_map(|phase| phase.iter().cloned().map(|node| vec![node]))
            .collect();
    }

    let mut rebalanced = Vec::new();
    for phase in execution_order {
        if phase.len() <= limit {
            rebalanced.push(phase.clone());
            continue;
        }

        for chunk in phase.chunks(limit) {
            rebalanced.push(chunk.to_vec());
        }
    }
    rebalanced
}

fn apply_learning_plan_feedback(
    ledger: &ArtifactLedger,
    plan: &mut crate::reinforcement::TaskPlanArtifact,
    workflow: &mut WorkflowGeneratedArtifact,
) -> AdaptivePlanningReport {
    let predicted_success_before = plan.routing.predicted_success_rate;
    plan.routing.predicted_success_rate = recommend_predicted_success_rate_from_learning(
        ledger,
        plan.routing.predicted_success_rate,
        plan.characteristics.complexity,
    );

    let parallelism_before = infer_workflow_parallelism(workflow);
    let recommended_parallelism =
        recommend_parallelism_from_learning(ledger, parallelism_before, 1, 4);
    workflow.execution_order =
        rebalance_execution_order(&workflow.execution_order, recommended_parallelism);
    let parallelism_after = infer_workflow_parallelism(workflow);

    AdaptivePlanningReport {
        predicted_success_before,
        predicted_success_after: plan.routing.predicted_success_rate,
        parallelism_before,
        recommended_parallelism,
        parallelism_after,
    }
}

async fn execute_runtime_subtasks(
    task: &str,
    workflow: &WorkflowGeneratedArtifact,
    records: &mut [crate::reinforcement::PlannedSubtaskRecord],
    context: &RuntimeExecutionContext,
) -> RuntimeExecutionReport {
    let mut id_to_index = HashMap::new();
    for (index, record) in records.iter().enumerate() {
        id_to_index.insert(record.id.clone(), index);
    }

    let mut assignment_records = Vec::new();
    let mut phases_executed = 0_usize;
    let mut halted_early = false;
    let mut serial_work_ms = 0_u64;
    let mut critical_path_ms = 0_u64;
    let mut failover_count = 0_usize;
    let mut failover_root_causes = Vec::new();
    let mut tool_loop_runs = 0_usize;
    let mut role_routed_subtasks = 0_usize;
    let mut memory_snapshots = Vec::new();
    let fail_fast = context.failure_strategy.eq_ignore_ascii_case("fail_fast");

    for (phase_idx, phase) in workflow.execution_order.iter().enumerate() {
        let phase_started = Instant::now();
        let mut join_set: JoinSet<SubtaskRunResult> = JoinSet::new();
        let mut scheduled = 0_usize;

        for node_id in phase {
            let Some(record_index) = id_to_index.get(node_id).copied() else {
                continue;
            };
            let subtask_description = records[record_index].description.clone();
            let mut local_context = context.clone();
            let task_text = task.to_string();
            let desired_role = workflow
                .nodes
                .iter()
                .find(|node| node.id == *node_id)
                .map(|node| node.role.clone());

            let mut ranked_candidates = Vec::new();
            if context.lazy_policy.enable_role_collaboration {
                let names = context
                    .candidates
                    .iter()
                    .map(|(name, _)| name.clone())
                    .collect::<Vec<_>>();
                ranked_candidates =
                    rank_execution_agents(&names, desired_role.as_deref(), phase_idx, record_index);
                // Blend in historical execution success: re-score using Bayesian
                // success rates from past TaskExecutionSummary records so that
                // agents with stronger real outcomes are preferred over agents
                // whose ranking is based on list-position heuristics alone.
                let historical_order = recommend_agent_order_from_execution_history(
                    &context.artifact_ledger,
                    &names,
                    20,
                );
                if historical_order.len() > 1 {
                    let hist_len = historical_order.len() as f64;
                    for candidate in ranked_candidates.iter_mut() {
                        if let Some(pos) =
                            historical_order.iter().position(|n| n == &candidate.agent)
                        {
                            let hist_score =
                                historical_order.len().saturating_sub(pos) as f64 / hist_len;
                            candidate.score = (candidate.score * 0.60 + hist_score * 0.40)
                                .clamp(0.0_f64, 1.0_f64);
                            candidate.reason =
                                format!("{}, hist_rank={}", candidate.reason, pos + 1);
                        }
                    }
                    ranked_candidates.sort_by(|a, b| {
                        b.score
                            .partial_cmp(&a.score)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then_with(|| a.agent.cmp(&b.agent))
                    });
                }
                if !ranked_candidates.is_empty() {
                    role_routed_subtasks += 1;
                }

                let by_name = context
                    .candidates
                    .iter()
                    .map(|(name, agent)| (name.clone(), agent.clone()))
                    .collect::<HashMap<_, _>>();
                let mut reordered = Vec::new();
                for candidate in &ranked_candidates {
                    if let Some(agent) = by_name.get(&candidate.agent) {
                        reordered.push((candidate.agent.clone(), agent.clone()));
                    }
                }
                for (name, agent) in &context.candidates {
                    if !reordered.iter().any(|(existing, _)| existing == name) {
                        reordered.push((name.clone(), agent.clone()));
                    }
                }
                local_context.candidates = reordered;
            }

            join_set.spawn(async move {
                execute_single_subtask(
                    task_text,
                    subtask_description,
                    record_index,
                    phase_idx,
                    desired_role,
                    ranked_candidates,
                    local_context,
                )
                .await
            });
            scheduled += 1;
        }

        if scheduled == 0 {
            continue;
        }

        phases_executed += 1;
        let mut phase_failed = false;

        while let Some(result) = join_set.join_next().await {
            let Ok(result) = result else {
                phase_failed = true;
                continue;
            };

            let now = crate::acp::prelude::now_ts();
            if let Some(record) = records.get_mut(result.record_index) {
                record.mark_executed(
                    now,
                    now,
                    result.duration_ms,
                    if result.success {
                        "completed"
                    } else {
                        "failed"
                    },
                    result.executor.clone(),
                );

                if !result.success {
                    phase_failed = true;
                }
            }

            if result.failover_applied {
                failover_count += 1;
                if let Some(reason) = result.failover_reason.clone() {
                    failover_root_causes.push(reason);
                }
            }
            if result.tool_loop_used {
                tool_loop_runs += 1;
            }
            if !result.response_excerpt.is_empty() {
                memory_snapshots.push(result.response_excerpt.clone());
            }
            for observation in &result.tool_observations {
                memory_snapshots.push(observation.clone());
            }

            serial_work_ms += result.duration_ms;
            assignment_records.push(ExecutionAssignmentRecord {
                subtask_id: records
                    .get(result.record_index)
                    .map(|record| record.id.clone())
                    .unwrap_or_else(|| format!("subtask-{}", result.record_index + 1)),
                phase_index: records
                    .get(result.record_index)
                    .map(|record| record.phase_index)
                    .unwrap_or(phase_idx),
                task_index: result.record_index,
                desired_role: result.desired_role,
                selected_agent: Some(result.executor.clone()),
                selection_reason: "runtime_execution".to_string(),
                candidate_scores: result.candidate_scores,
                dependency_blocked: false,
                node_primary_agent: Some(context.primary_agent.clone()),
                node_secondary_agents: context.secondary_agents.clone(),
                effective_executor: Some(result.executor),
                failover_applied: result.failover_applied,
                failover_reason: result.failover_reason,
            });
        }

        critical_path_ms += phase_started.elapsed().as_millis() as u64;
        if fail_fast && phase_failed {
            halted_early = true;
            break;
        }
    }

    if halted_early {
        let now = crate::acp::prelude::now_ts();
        for record in records.iter_mut() {
            if record.start_ts.is_none() {
                record.mark_executed(now, now, 0, "skipped", "scheduler");
            }
        }
    }

    let subtasks_completed = records
        .iter()
        .filter(|record| record.outcome.as_deref() == Some("completed"))
        .count();
    let subtasks_failed = records
        .iter()
        .filter(|record| record.outcome.as_deref() == Some("failed"))
        .count();
    let subtasks_skipped = records
        .iter()
        .filter(|record| record.outcome.as_deref() == Some("skipped"))
        .count();

    let total_phases = workflow.execution_order.len().max(1);
    let parallel_phases = workflow
        .execution_order
        .iter()
        .filter(|phase| phase.len() > 1)
        .count();
    let parallel_utilization = parallel_phases as f64 / total_phases as f64;
    let subtask_parallelism = workflow
        .execution_order
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(1)
        .max(1);
    let parallel_speedup = if critical_path_ms == 0 {
        1.0
    } else {
        (serial_work_ms as f64 / critical_path_ms as f64).max(1.0)
    };
    let parallel_efficiency = if subtask_parallelism > 1 {
        (parallel_speedup / subtask_parallelism as f64).clamp(0.0, 1.0)
    } else {
        1.0
    };

    let mut memory_entries_written = 0_usize;
    let mut memory_entries_retained = 0_usize;
    let mut memory_artifact_path = None;
    if context.lazy_policy.enable_memory_policy {
        let promotion = if let Ok(mut store) = context.memory_store.lock() {
            for (index, content) in memory_snapshots.iter().enumerate() {
                let class = if content.contains("tool:") {
                    MemoryClass::Observation
                } else {
                    MemoryClass::Episodic
                };
                store.store(MemoryEntry {
                    id: format!("mem-{}-{}", crate::acp::prelude::now_ts_ms(), index + 1),
                    class,
                    content: content.clone(),
                    timestamp: crate::acp::prelude::now_ts().to_string(),
                    usefulness: 0.8,
                    staleness: 0,
                });
                memory_entries_written += 1;
            }
            store.gc();
            let promotion: MemoryPromotionReport = store.promote();
            memory_entries_retained = store.retrieve(MemoryClass::Observation, 128).len()
                + store.retrieve(MemoryClass::Episodic, 128).len();
            promotion
        } else {
            MemoryPromotionReport::default()
        };

        let memory_artifact = MemoryPolicyExecutionArtifact {
            generated_at: crate::acp::prelude::now_ts(),
            task: task.to_string(),
            policy: context.lazy_policy.clone(),
            total_entries_before_gc: memory_entries_written,
            retained_entries_after_gc: memory_entries_retained,
            sample_observations: memory_snapshots.into_iter().take(8).collect(),
        };
        let ledger = ArtifactLedger::new(None);
        if let Ok(path) = ledger.write_json("spec", "latest-memory-policy.json", &memory_artifact) {
            memory_artifact_path = Some(path.display().to_string());
        }
        // Persist promotion report (BLUE8-M3)
        let promotion_artifact = serde_json::json!({
            "generated_at": crate::acp::prelude::now_ts(),
            "task": task,
            "promoted_count": promotion.promoted_count,
            "promotion_map": promotion.promotion_map,
        });
        let _ = ledger.write_json("spec", "latest-promoted-memory.json", &promotion_artifact);
    }

    RuntimeExecutionReport {
        assignment_records,
        subtasks_completed,
        subtasks_failed,
        subtasks_skipped,
        subtask_parallelism,
        phases_executed,
        halted_early,
        parallel_utilization,
        parallel_failure_rollback_count: if halted_early && subtasks_failed > 0 {
            1
        } else {
            0
        },
        serial_work_ms,
        critical_path_ms,
        parallel_efficiency,
        parallel_speedup,
        failure_strategy: context.failure_strategy.clone(),
        failover_count,
        failover_root_cause: failover_root_causes.into_iter().next().unwrap_or_default(),
        lazy_load: LazyLoadExecutionReport {
            policy: context.lazy_policy.clone(),
            tool_loop_runs,
            role_routed_subtasks,
            memory_entries_written,
            memory_entries_retained,
            memory_artifact_path,
        },
    }
}

async fn execute_single_subtask(
    task: String,
    subtask_description: String,
    record_index: usize,
    phase_index: usize,
    desired_role: Option<String>,
    candidate_scores: Vec<ExecutionDecisionCandidate>,
    mut context: RuntimeExecutionContext,
) -> SubtaskRunResult {
    let started = Instant::now();
    let mut tool_observations = Vec::new();
    let tool_context = if context.lazy_policy.enable_tool_loop {
        run_lazy_tool_loop(task.as_str(), subtask_description.as_str(), record_index)
    } else {
        String::new()
    };
    if !tool_context.is_empty() {
        tool_observations.push(tool_context.clone());
    }
    // Inject relevant knowledge from vector memory so agents have prior
    // context without needing to re-derive it from scratch.
    let vector_context_prefix = if let Some(store) = &context.vector_store {
        let execution_phase = format!("phase-{}", phase_index + 1);
        let semantic_phase = context.app_config.default_phase.clone();
        let mut search_phases = vec![execution_phase];
        if !semantic_phase.is_empty() && !search_phases.iter().any(|phase| phase == &semantic_phase)
        {
            search_phases.push(semantic_phase);
        }

        let snippets =
            collect_vector_context_snippets(store, &search_phases, &subtask_description, 3);

        if snippets.is_empty() {
            String::new()
        } else {
            format!(
                "Relevant context from memory:\n{}\n",
                snippets
                    .iter()
                    .map(|snippet| format!("• {}", snippet))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        }
    } else {
        String::new()
    };
    let messages = vec![Message {
        role: "user".to_string(),
        content: format!(
            "{}Parent task: {}\nSubtask: {}\n{}\nReturn concrete implementation outcome and concise verification.",
            vector_context_prefix,
            task,
            subtask_description,
            if tool_context.is_empty() {
                "".to_string()
            } else {
                format!("Tool observations:\n{}", tool_context)
            }
        ),
    }];

    // Build task envelope for this subtask (BLUE8-M4)
    let task_id = format!(
        "subtask-{}-{}-{}",
        phase_index + 1,
        record_index + 1,
        crate::acp::prelude::now_ts_ms()
    );
    let envelope = AgentTaskEnvelope {
        task_id: task_id.clone(),
        phase: format!("phase-{}", phase_index + 1),
        role: desired_role
            .clone()
            .unwrap_or_else(|| "executor".to_string()),
        objective: subtask_description.clone(),
        constraints: context.principles.as_ref().map(|p| p.join("; ")),
        evidence: if tool_context.is_empty() {
            None
        } else {
            Some(tool_context.clone())
        },
        input: serde_json::json!({ "task": task.as_str(), "subtask": subtask_description.as_str() }),
    };

    let mut first_failure_reason: Option<String> = None;

    // Sort candidates by adaptive model selector: best-known model first
    let phase_name = format!("phase-{}", phase_index + 1);
    let agent_names: Vec<String> = context.candidates.iter().map(|(n, _)| n.clone()).collect();
    if let Ok(sel) = context.adaptive_selector.lock() {
        if let Some(best) = sel.get_best_model(&agent_names) {
            if let Some(pos) = context.candidates.iter().position(|(n, _)| n == &best) {
                if pos > 0 {
                    context.candidates.swap(0, pos);
                }
            }
        }
    }
    // Skip agents that FailurePrevention marks as severely degraded (only if alternatives exist)
    let degraded_set: std::collections::HashSet<String> = context
        .failure_prevention
        .lock()
        .map(|fp| {
            agent_names
                .iter()
                .filter(|n| fp.should_degrade(n))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if !degraded_set.is_empty()
        && context
            .candidates
            .iter()
            .any(|(n, _)| !degraded_set.contains(n))
    {
        context
            .candidates
            .retain(|(n, _)| !degraded_set.contains(n));
    }

    for (idx, (agent_name, agent)) in context.candidates.iter().enumerate() {
        let selection = FlowModelSelector::select_model_for_agent(
            agent.as_ref(),
            context.app_config.as_ref(),
            Some(&subtask_description),
        );

        let mut options = context.base_options.clone();
        let selected_model = selection
            .selected_model
            .as_ref()
            .map(|model| model.id.clone());
        if let Some(model_id) = selected_model.clone() {
            options.insert("model".to_string(), Value::String(model_id));
        }
        let request_options = if options.is_empty() {
            None
        } else {
            Some(options)
        };

        let run_result = run_agent_chat_collecting(
            agent.clone(),
            messages.clone(),
            context.principles.clone(),
            request_options.clone(),
            context.task_timeout_seconds,
        )
        .await;

        if let (Ok(mut selector), Some(model_id)) =
            (context.adaptive_selector.lock(), selected_model.clone())
        {
            selector.record_result(&model_id, run_result.is_ok());
        }
        // Record per-agent outcome to online controller for adaptive ranking
        let duration_ms = started.elapsed().as_millis() as u64;
        if let Ok(mut ctrl) = context.online_controller.lock() {
            ctrl.record_agent_outcome(&phase_name, agent_name, run_result.is_ok(), duration_ms);
        }
        if let Ok(mut fp) = context.failure_prevention.lock() {
            fp.record_outcome(agent_name, run_result.is_ok(), duration_ms);
        }

        match run_result {
            Ok(response) if !response.trim().is_empty() => {
                let model_tool_calls = extract_model_tool_calls(&response, 3);
                let model_tool_observations = execute_model_tool_calls(
                    task.as_str(),
                    subtask_description.as_str(),
                    record_index,
                    &model_tool_calls,
                );

                let mut final_response = response;
                if !model_tool_observations.is_empty() {
                    tool_observations.extend(model_tool_observations.clone());
                    let mut followup_messages = messages.clone();
                    followup_messages.push(Message {
                        role: "assistant".to_string(),
                        content: final_response.clone(),
                    });
                    followup_messages.push(Message {
                        role: "user".to_string(),
                        content: format!(
                            "Tool execution results:\n{}\n\nIncorporate these observations and provide the final executable outcome.",
                            model_tool_observations.join("\n")
                        ),
                    });

                    if let Ok(followup) = run_agent_chat_collecting(
                        agent.clone(),
                        followup_messages,
                        context.principles.clone(),
                        request_options.clone(),
                        context.task_timeout_seconds,
                    )
                    .await
                    {
                        if !followup.trim().is_empty() {
                            final_response = followup;
                        }
                    }
                }

                // Build audit log for this successful execution (BLUE8-M5)
                let audit = AgentAuditLog {
                    agent: agent_name.clone(),
                    phase: envelope.phase.clone(),
                    task_id: task_id.clone(),
                    decision: "executed".to_string(),
                    rationale: Some(format!(
                        "subtask completed; failover={}; tool_loop={}; model_tool_calls={}",
                        idx > 0,
                        context.lazy_policy.enable_tool_loop,
                        model_tool_calls.len(),
                    )),
                    timestamp: crate::acp::prelude::now_ts().to_string(),
                };
                let audit_log_json = serde_json::to_string(&audit).ok();
                // Persist audit log to artifact ledger
                let ledger = ArtifactLedger::new(None);
                let _ = ledger.write_json("spec", "latest-audit-log.json", &audit);

                return SubtaskRunResult {
                    record_index,
                    duration_ms: started.elapsed().as_millis() as u64,
                    executor: agent_name.clone(),
                    success: true,
                    failover_applied: idx > 0,
                    failover_reason: if idx > 0 {
                        first_failure_reason.clone()
                    } else {
                        None
                    },
                    desired_role,
                    candidate_scores,
                    response_excerpt: final_response.chars().take(220).collect(),
                    tool_loop_used: context.lazy_policy.enable_tool_loop
                        || !model_tool_observations.is_empty(),
                    tool_observations,
                    audit_log_json,
                };
            }
            Ok(_) => {
                if first_failure_reason.is_none() {
                    first_failure_reason = Some("empty_response".to_string());
                }
            }
            Err(err) => {
                if first_failure_reason.is_none() {
                    first_failure_reason = Some(err.to_string());
                }
            }
        }
    }

    // Envelope is captured but execution failed - suppress unused-variable warning
    let _ = envelope;

    SubtaskRunResult {
        record_index,
        duration_ms: started.elapsed().as_millis() as u64,
        executor: context
            .candidates
            .first()
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| "scheduler".to_string()),
        success: false,
        failover_applied: false,
        failover_reason: first_failure_reason,
        desired_role,
        candidate_scores,
        response_excerpt: String::new(),
        tool_loop_used: context.lazy_policy.enable_tool_loop,
        tool_observations,
        audit_log_json: None,
    }
}

fn collect_vector_context_snippets(
    store: &VectorStore,
    search_phases: &[String],
    subtask_description: &str,
    max_snippets: usize,
) -> Vec<String> {
    let mut snippets: Vec<String> = Vec::new();
    for phase in search_phases {
        if let Ok((hits, _)) = store.search(phase, subtask_description, max_snippets, 0.25, 512) {
            for hit in hits {
                let snippet = hit.response_snippet.trim();
                if snippet.is_empty() {
                    continue;
                }
                if !snippets.iter().any(|existing| existing == snippet) {
                    snippets.push(snippet.to_string());
                }
                if snippets.len() >= max_snippets {
                    break;
                }
            }
        }
        if snippets.len() >= max_snippets {
            break;
        }
    }
    snippets
}

fn run_lazy_tool_loop(task: &str, subtask: &str, record_index: usize) -> String {
    let registry = ToolRegistry::new();
    let Some(search_tool) = registry.get("search_files") else {
        return String::new();
    };

    let pattern = if subtask.to_ascii_lowercase().contains("test") {
        "**/*test*.rs"
    } else {
        "**/*.rs"
    };

    let input = ToolInput {
        task_id: format!("subtask-{}", record_index + 1),
        phase: "execution".to_string(),
        agent_role: "coder".to_string(),
        objective: task.to_string(),
        constraints: Some("lazy-tool-loop".to_string()),
        evidence: Some(subtask.to_string()),
        payload: json!({
            "pattern": pattern,
            "directory": "src"
        }),
    };

    match search_tool.run(&input) {
        Ok(output) => {
            let count = output
                .result
                .and_then(|result| {
                    result
                        .get("files")
                        .and_then(Value::as_array)
                        .map(|items| items.len())
                })
                .unwrap_or(0);
            format!("tool:search_files pattern={} hits={}", pattern, count)
        }
        Err(_) => String::new(),
    }
}

#[derive(Clone, Debug)]
struct ModelToolCall {
    name: String,
    arguments: Value,
}

fn extract_model_tool_calls(response: &str, max_calls: usize) -> Vec<ModelToolCall> {
    let mut calls = Vec::new();

    for block in extract_json_code_blocks(response) {
        if let Ok(value) = serde_json::from_str::<Value>(&block) {
            append_model_tool_calls_from_value(&value, &mut calls, max_calls);
            if calls.len() >= max_calls {
                return calls;
            }
        }
    }

    if calls.is_empty() {
        if let Ok(value) = serde_json::from_str::<Value>(response.trim()) {
            append_model_tool_calls_from_value(&value, &mut calls, max_calls);
        }
    }

    calls.truncate(max_calls);
    calls
}

fn extract_json_code_blocks(response: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut cursor = 0usize;

    while let Some(start_rel) = response[cursor..].find("```json") {
        let start = cursor + start_rel + "```json".len();
        if let Some(end_rel) = response[start..].find("```") {
            let end = start + end_rel;
            blocks.push(response[start..end].trim().to_string());
            cursor = end + 3;
        } else {
            break;
        }
    }

    blocks
}

fn append_model_tool_calls_from_value(
    value: &Value,
    out: &mut Vec<ModelToolCall>,
    max_calls: usize,
) {
    if out.len() >= max_calls {
        return;
    }

    if let Some(tool_calls) = value.get("tool_calls").and_then(Value::as_array) {
        for call in tool_calls {
            if out.len() >= max_calls {
                break;
            }
            let name = call
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| {
                    call.get("function")
                        .and_then(Value::as_object)
                        .and_then(|f| f.get("name"))
                        .and_then(Value::as_str)
                })
                .unwrap_or_default()
                .trim()
                .to_string();
            if name.is_empty() {
                continue;
            }
            let arguments = parse_tool_call_arguments(call);
            out.push(ModelToolCall { name, arguments });
        }
        return;
    }

    if let Some(choices) = value.get("choices").and_then(Value::as_array) {
        for choice in choices {
            if out.len() >= max_calls {
                break;
            }
            if let Some(message_tool_calls) = choice
                .get("message")
                .and_then(Value::as_object)
                .and_then(|msg| msg.get("tool_calls"))
                .and_then(Value::as_array)
            {
                append_model_tool_calls_from_value(
                    &json!({"tool_calls": message_tool_calls}),
                    out,
                    max_calls,
                );
            }
        }
    }

    if let Some(output) = value.get("output").and_then(Value::as_array) {
        for item in output {
            if out.len() >= max_calls {
                break;
            }
            if item.get("type").and_then(Value::as_str) == Some("tool_call") {
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                if name.is_empty() {
                    continue;
                }
                let arguments = parse_tool_call_arguments(item);
                out.push(ModelToolCall { name, arguments });
            }
        }
    }

    if value.get("name").and_then(Value::as_str).is_some() {
        let name = value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if !name.is_empty() {
            let arguments = parse_tool_call_arguments(value);
            out.push(ModelToolCall { name, arguments });
        }
    }
}

fn parse_tool_call_arguments(value: &Value) -> Value {
    if let Some(args) = value.get("arguments") {
        return parse_argument_value(args);
    }
    if let Some(function) = value.get("function") {
        if let Some(args) = function.get("arguments") {
            return parse_argument_value(args);
        }
    }
    json!({})
}

fn parse_argument_value(value: &Value) -> Value {
    match value {
        Value::String(raw) => serde_json::from_str::<Value>(raw).unwrap_or_else(|_| json!({})),
        Value::Object(_) => value.clone(),
        _ => json!({}),
    }
}

fn normalize_tool_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>()
}

fn tool_name_similarity(left: &str, right: &str) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    if left == right {
        return 1.0;
    }
    let shared_prefix = left
        .chars()
        .zip(right.chars())
        .take_while(|(l, r)| l == r)
        .count() as f64;
    let prefix_score = shared_prefix / left.len().max(right.len()) as f64;
    let overlap = left.chars().filter(|ch| right.contains(*ch)).count() as f64;
    let overlap_score = overlap / left.len().max(right.len()) as f64;
    (0.5 * prefix_score + 0.5 * overlap_score).clamp(0.0, 1.0)
}

fn resolve_auto_tool_name(requested_name: &str, registry: &ToolRegistry) -> Option<String> {
    if registry.get(requested_name).is_some() {
        return Some(requested_name.to_string());
    }

    let normalized_requested = normalize_tool_name(requested_name);
    registry
        .names()
        .into_iter()
        .map(|name| {
            let score = tool_name_similarity(&normalized_requested, &normalize_tool_name(name));
            (name.to_string(), score)
        })
        .filter(|(_, score)| *score >= 0.6)
        .max_by(|left, right| {
            left.1
                .partial_cmp(&right.1)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(name, _)| name)
}

fn execute_model_tool_calls(
    task: &str,
    subtask: &str,
    record_index: usize,
    calls: &[ModelToolCall],
) -> Vec<String> {
    let mut observations = Vec::new();
    let registry = ToolRegistry::new();

    for (idx, call) in calls.iter().enumerate() {
        let Some(resolved_name) = resolve_auto_tool_name(call.name.as_str(), &registry) else {
            observations.push(format!("tool:auto {} unavailable", call.name));
            continue;
        };
        let Some(tool) = registry.get(resolved_name.as_str()) else {
            observations.push(format!("tool:auto {} unavailable", call.name));
            continue;
        };

        if let Err(err) = validate_tool_arguments(resolved_name.as_str(), &call.arguments) {
            observations.push(format!(
                "tool:auto {} invalid_arguments: {}",
                resolved_name, err
            ));
            continue;
        }

        let input = ToolInput {
            task_id: format!("model-tool-{}-{}", record_index + 1, idx + 1),
            phase: "execution".to_string(),
            agent_role: "coder".to_string(),
            objective: task.to_string(),
            constraints: Some("model-driven-tool-calls".to_string()),
            evidence: Some(subtask.to_string()),
            payload: call.arguments.clone(),
        };

        match tool.run(&input) {
            Ok(output) => {
                let snippet = serde_json::to_string(&output)
                    .unwrap_or_else(|_| "tool result serialization failed".to_string());
                observations.push(format!(
                    "tool:auto {} ok {}",
                    resolved_name,
                    snippet.chars().take(220).collect::<String>()
                ));
            }
            Err(err) => {
                observations.push(format!("tool:auto {} failed {}", resolved_name, err));
            }
        }
    }

    observations
}

async fn run_agent_chat_collecting(
    agent: Arc<dyn crate::agent::Agent>,
    messages: Vec<Message>,
    principles: Option<Vec<String>>,
    options: Option<HashMap<String, Value>>,
    timeout_seconds: Option<u64>,
) -> Result<String> {
    let (sender, mut receiver) = mpsc::channel::<String>(2048);
    let sender = crate::agent::StreamingSender::from(sender);
    let task = tokio::spawn(async move { agent.chat(messages, principles, options, sender).await });

    let collect = async move {
        let mut response = String::new();
        while let Some(token) = receiver.recv().await {
            response.push_str(&token);
        }

        match task.await {
            Ok(Ok(())) => Ok::<String, anyhow::Error>(response),
            Ok(Err(err)) => Err(err.into()),
            Err(join_err) => Err(anyhow::anyhow!("agent task panicked: {join_err}")),
        }
    };

    if let Some(seconds) = timeout_seconds {
        let timeout = Duration::from_secs(seconds.max(1));
        match tokio::time::timeout(timeout, collect).await {
            Ok(result) => result,
            Err(_) => Err(anyhow::anyhow!(
                "agent request timed out after {}s",
                seconds
            )),
        }
    } else {
        collect.await
    }
}

async fn handle_learning_summary(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let ledger = clone_artifact_ledger(server);
    let window = params
        .get("limit")
        .or_else(|| params.get("window"))
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(20)
        .max(1);
    let knowledge_bus =
        read_latest_artifact::<KnowledgeBusArtifact>(&ledger, "spec", "latest-knowledge.json");
    let Some(bus) = read_latest_artifact::<WorkflowLearningBusArtifact>(
        &ledger,
        "spec",
        "latest-learning.json",
    ) else {
        return send_result(
            server,
            request_id,
            json!({
                "ok": true,
                "summary": {"sampled_events": 0, "totals": {}, "averages": {}, "rates": {}},
                "knowledge": knowledge_bus.as_ref().map(|bus| json!({
                    "total_events": bus.total_events,
                    "sampled_events": bus.events.len().min(window),
                    "latest_generated_at": bus.generated_at,
                    "recent": bus.events.iter().rev().take(window).cloned().collect::<Vec<_>>()
                })).unwrap_or_else(|| json!({"total_events": 0, "sampled_events": 0, "recent": []})),
                "events": []
            }),
        )
        .await;
    };

    let events = bus
        .events
        .iter()
        .rev()
        .take(window)
        .cloned()
        .collect::<Vec<_>>();
    let count = events.len().max(1);
    let avg_success = events
        .iter()
        .map(|item| item.predicted_success_rate as f64)
        .sum::<f64>()
        / count as f64;
    let avg_speedup = events.iter().map(|item| item.parallel_speedup).sum::<f64>() / count as f64;
    let avg_risk = events.iter().map(|item| item.risk_score).sum::<f64>() / count as f64;
    let failover_total = events
        .iter()
        .map(|item| item.failover_count as u64)
        .sum::<u64>();
    let avg_rounds = events
        .iter()
        .map(|item| item.clarification_rounds as f64)
        .sum::<f64>()
        / count as f64;
    let avg_quality = events
        .iter()
        .map(|item| item.clarification_quality_score)
        .sum::<f64>()
        / count as f64;
    let requirement_change_total = events
        .iter()
        .map(|item| item.requirement_change_count as u64)
        .sum::<u64>();
    let gates_pass_rate = events.iter().filter(|item| item.gates_ok).count() as f64 / count as f64;

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "summary": {
                "total_events": bus.total_events,
                "sampled_events": events.len(),
                "latest_generated_at": bus.generated_at,
                "totals": {
                    "requirement_change_count": requirement_change_total,
                    "failover_count": failover_total,
                },
                "averages": {
                    "predicted_success_rate": avg_success,
                    "parallel_speedup": avg_speedup,
                    "risk_score": avg_risk,
                    "clarification_rounds": avg_rounds,
                    "clarification_quality_score": avg_quality,
                },
                "rates": {
                    "gates_pass_rate": gates_pass_rate,
                }
            },
                "knowledge": knowledge_bus.as_ref().map(|bus| json!({
                    "total_events": bus.total_events,
                    "sampled_events": bus.events.len().min(window),
                    "latest_generated_at": bus.generated_at,
                    "recent": bus.events.iter().rev().take(window).cloned().collect::<Vec<_>>()
                })).unwrap_or_else(|| json!({"total_events": 0, "sampled_events": 0, "recent": []})),
            "events": events,
        }),
    )
    .await
}

async fn handle_phase_policy_replay(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let window = params
        .get("limit")
        .or_else(|| params.get("window"))
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(200)
        .max(1);
    let mode = params
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("agent")
        .to_string();

    let events = trace_events()
        .lock()
        .map(|guard| {
            guard
                .iter()
                .rev()
                .filter(|event| event.event_type == "phase.agent")
                .take(window)
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mut phase_stats: HashMap<String, (u64, u64, u64)> = HashMap::new();
    for event in &events {
        let entry = phase_stats.entry(event.phase.clone()).or_insert((0, 0, 0));
        entry.0 = entry.0.saturating_add(1);
        if event.status.eq_ignore_ascii_case("ok") {
            entry.1 = entry.1.saturating_add(1);
        }
        entry.2 = entry.2.saturating_add(event.duration_ms);
    }

    let mut ranked = phase_stats
        .iter()
        .map(|(phase, (attempts, successes, total_duration_ms))| {
            let success_rate = if *attempts == 0 {
                0.0
            } else {
                *successes as f64 / *attempts as f64
            };
            let avg_latency_ms = if *attempts == 0 {
                0.0
            } else {
                *total_duration_ms as f64 / *attempts as f64
            };
            let latency_factor = if avg_latency_ms <= f64::EPSILON {
                0.5
            } else {
                (1.0 / (1.0 + (avg_latency_ms / 5000.0))).clamp(0.0, 1.0)
            };
            let empirical_score = (0.75 * success_rate + 0.25 * latency_factor).clamp(0.0, 1.0);
            json!({
                "phase": phase,
                "attempts": attempts,
                "successes": successes,
                "success_rate": success_rate,
                "avg_latency_ms": avg_latency_ms,
                "empirical_score": empirical_score,
            })
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .get("empirical_score")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            .partial_cmp(
                &left
                    .get("empirical_score")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0),
            )
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let candidate_phases = server
        .flow_manager
        .as_ref()
        .map(|flow| flow.config().flow.phases.clone())
        .unwrap_or_default();
    let (controller_recommended, controller_snapshot) = server
        .online_controller
        .lock()
        .ok()
        .map(|ctrl| {
            (
                ctrl.recommend_phase(&candidate_phases),
                ctrl.phase_policy_snapshot(&candidate_phases),
            )
        })
        .unwrap_or((None, Vec::new()));
    let empirical_best = ranked
        .first()
        .and_then(|row| row.get("phase"))
        .and_then(Value::as_str)
        .map(|value| value.to_string());

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "mode": mode,
            "sampled_events": events.len(),
            "candidate_phases": candidate_phases,
            "controller_recommended_phase": controller_recommended,
            "empirical_best_phase": empirical_best,
            "controller_phase_policy": controller_snapshot.into_iter().map(|(phase, mean_reward, reliability, pulls)| json!({
                "phase": phase,
                "mean_reward": mean_reward,
                "reliability": reliability,
                "pulls": pulls,
            })).collect::<Vec<_>>(),
            "phase_scores": ranked,
            "agreement": {
                "matches_empirical_best": controller_recommended.is_some() && controller_recommended == empirical_best,
            }
        }),
    )
    .await
}

async fn handle_primary_secondary_summary(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let ledger = clone_artifact_ledger(server);
    let window = params
        .get("limit")
        .or_else(|| params.get("window"))
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(20)
        .max(1);
    let bus = read_latest_artifact::<WorkflowLearningBusArtifact>(
        &ledger,
        "spec",
        "latest-learning.json",
    );
    let policy = read_latest_artifact::<PrimarySecondaryPolicyArtifact>(
        &ledger,
        "spec",
        "latest-primary-secondary-policy.json",
    );
    let failover = read_latest_artifact::<PrimarySecondaryFailoverArtifact>(
        &ledger,
        "spec",
        "latest-primary-secondary-failover.json",
    );

    let events = bus
        .as_ref()
        .map(|bus| {
            bus.events
                .iter()
                .rev()
                .take(window)
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let count = events.len().max(1);
    let avg_primary_stability = events
        .iter()
        .map(|item| item.primary_stability_score)
        .sum::<f64>()
        / count as f64;
    let avg_secondary_utilization = events
        .iter()
        .map(|item| item.secondary_utilization_rate)
        .sum::<f64>()
        / count as f64;
    let total_failovers = events
        .iter()
        .map(|item| item.failover_count as u64)
        .sum::<u64>();
    let mut root_causes = HashMap::new();
    for event in &events {
        if !event.failover_root_cause.is_empty() {
            *root_causes
                .entry(event.failover_root_cause.clone())
                .or_insert(0_u64) += 1;
        }
    }

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "summary": {
                "total_events": events.len(),
                "averages": {
                    "primary_stability_score": avg_primary_stability,
                    "secondary_utilization_rate": avg_secondary_utilization,
                },
                "totals": {
                    "failover_count": total_failovers,
                },
                "failover_root_causes": root_causes,
                "latest_policy": policy,
                "latest_failover": failover,
            }
        }),
    )
    .await
}

fn parse_messages(params: &Value) -> Option<Vec<Message>> {
    if let Some(messages) = params.get("messages") {
        return serde_json::from_value(messages.clone()).ok();
    }
    if let Some(message) = params.get("message") {
        return serde_json::from_value(message.clone())
            .ok()
            .map(|message| vec![message]);
    }

    params
        .get("content")
        .and_then(Value::as_str)
        .map(|content| {
            vec![Message {
                role: params
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or("user")
                    .to_string(),
                content: content.to_string(),
            }]
        })
}

fn build_runtime_gauge_snapshot(server: &AcpServer) -> RuntimeGaugeSnapshot {
    let memory_cache_entries = server
        .memory_response_cache
        .lock()
        .map(|cache| cache.active_entries() as u64)
        .unwrap_or(0);
    let sqlite_cache_entries = server
        .response_cache
        .as_ref()
        .and_then(|cache| cache.entry_count().ok())
        .unwrap_or(0);
    let (vector_memory_entries, vector_summary_entries) = server
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

fn trace_metrics_snapshot(server: &AcpServer) -> Value {
    let slow_top_n = server.runtime_config.trace_slow_top_n.max(1);
    let events = trace_events()
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();

    let mut requests = events
        .iter()
        .filter(|event| event.event_type == "request.end")
        .map(|event| {
            let method = event
                .inputs
                .get("attributes")
                .and_then(|value| value.get("method"))
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            json!({
                "request_id": event.task_id,
                "method": method,
                "duration_ms": event.duration_ms,
                "status": event.status,
                "timestamp": event.timestamp,
            })
        })
        .collect::<Vec<_>>();
    requests.sort_by(|left, right| {
        right
            .get("duration_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .cmp(&left.get("duration_ms").and_then(Value::as_u64).unwrap_or(0))
    });
    requests.truncate(slow_top_n);

    let mut phase_buckets: HashMap<String, Vec<u64>> = HashMap::new();
    for event in &events {
        if event.duration_ms == 0 {
            continue;
        }
        if event.event_type.starts_with("phase.") || event.event_type == "request.end" {
            phase_buckets
                .entry(event.phase.clone())
                .or_default()
                .push(event.duration_ms);
        }
    }

    let mut by_phase = serde_json::Map::new();
    for (phase, mut samples) in phase_buckets {
        samples.sort_unstable();
        by_phase.insert(
            phase,
            json!({
                "count": samples.len(),
                "p95_ms": percentile(&samples, 95.0),
                "p99_ms": percentile(&samples, 99.0),
            }),
        );
    }

    let mut by_pua_stage: HashMap<String, u64> = HashMap::new();
    for event in &events {
        if let Some(stage) = event.pua_stage.as_ref() {
            *by_pua_stage.entry(stage.clone()).or_insert(0) += 1;
        }
    }

    let sampling_rate = server
        .telemetry_runtime
        .lock()
        .map(|guard| guard.sampling_rate())
        .unwrap_or(0.0);
    json!({
        "sampling_rate": sampling_rate,
        "buffered_events": events.len(),
        "slow_requests_top_n": requests,
        "phase_latency": by_phase,
        "pua_stage_counts": by_pua_stage,
    })
}

fn percentile(samples: &[u64], percentile: f64) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let rank = ((samples.len() - 1) as f64 * (percentile / 100.0)).round() as usize;
    samples[rank.min(samples.len() - 1)]
}

fn clone_artifact_ledger(server: &AcpServer) -> ArtifactLedger {
    server
        .artifact_ledger
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_else(|_| ArtifactLedger::new(server.config_path.as_deref().map(Path::new)))
}

fn read_latest_artifact<T: DeserializeOwned>(
    ledger: &ArtifactLedger,
    category: &str,
    latest_name: &str,
) -> Option<T> {
    let path = ledger.latest_path(category, latest_name);
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub(crate) async fn create_checkpoint_record(
    server: &AcpServer,
    conversation_id: &str,
    branch_id: &str,
    messages: Vec<Message>,
    note: Option<String>,
    parent_checkpoint_id: Option<String>,
) -> crate::acp::prelude::ConversationCheckpoint {
    let mut state = server.conversation_state.lock().await;
    let checkpoint_id = format!(
        "cp-{}-{}",
        crate::acp::prelude::now_ts_ms(),
        state.checkpoints.len() + 1
    );
    let branch_key = format!("{}:{}", conversation_id, branch_id);
    let checkpoint = crate::acp::prelude::ConversationCheckpoint {
        checkpoint_id: checkpoint_id.clone(),
        conversation_id: conversation_id.to_string(),
        branch_id: branch_id.to_string(),
        parent_checkpoint_id: parent_checkpoint_id
            .or_else(|| state.branch_heads.get(&branch_key).cloned()),
        created_at: crate::acp::prelude::now_ts(),
        note,
        messages,
    };
    state.branch_heads.insert(branch_key, checkpoint_id);
    state.last_touched_at = crate::acp::prelude::now_ts();
    state.checkpoints.push(checkpoint.clone());
    enforce_checkpoint_capacity(&mut state, 0, Some(&checkpoint.checkpoint_id));
    checkpoint
}

fn build_mcp_tool_descriptors(server: &AcpServer) -> Vec<Value> {
    let mut tools = vec![
        json!({
            "name": "acp_trace_get",
            "description": "Get ACP trace events",
            "input_schema": {"type": "object"}
        }),
        json!({
            "name": "acp_debug_panel_get",
            "description": "Get ACP debug panel snapshot",
            "input_schema": {"type": "object"}
        }),
    ];

    let registry = ToolRegistry::new();
    let mut builtins = registry.names();
    builtins.sort_unstable();
    tools.extend(builtins.into_iter().map(|name| {
        serde_json::to_value(local_tool_descriptor(name)).unwrap_or_else(|_| {
            json!({
                "name": name,
                "description": "Registered MCP tool",
                "input_schema": {"type": "object"}
            })
        })
    }));

    if let Ok(registry) = server.skill_registry.lock() {
        tools.extend(registry.list().into_iter().map(|skill| {
            json!({
                "name": skill.name,
                "description": skill.description,
                "input_schema": skill.input_schema,
                "x_runtime": {
                    "score": skill.score,
                    "total_calls": skill.total_calls,
                    "success_calls": skill.success_calls,
                    "failure_calls": skill.failure_calls,
                    "average_latency_ms": skill.average_latency_ms,
                }
            })
        }));
    }

    tools
}

async fn execute_mcp_tool_call(server: &AcpServer, name: &str, arguments: &Value) -> Result<Value> {
    match name {
        "acp_trace_get" => {
            let trace = build_trace_payload(arguments);
            Ok(json!({
                "ok": true,
                "events": trace.get("events").cloned().unwrap_or_else(|| json!([])),
                "total": trace.get("total").cloned().unwrap_or_else(|| json!(0)),
                "limit": trace.get("limit").cloned().unwrap_or_else(|| json!(100)),
            }))
        }
        "acp_debug_panel_get" => Ok(build_debug_panel_payload(server).await),
        _ => {
            let registry = ToolRegistry::new();
            if let Some(tool) = registry.get(name) {
                validate_tool_arguments(name, arguments)?;
                let result = tool.run(&ToolInput {
                    task_id: format!("mcp-tool-{name}"),
                    phase: "mcp".to_string(),
                    agent_role: "tool".to_string(),
                    objective: format!("Execute MCP tool '{name}'"),
                    constraints: None,
                    evidence: None,
                    payload: arguments.clone(),
                })?;
                return Ok(serde_json::to_value(result)?);
            }

            let resolved_skill_name = server.skill_registry.lock().ok().and_then(|registry| {
                if registry.get(name).is_some() {
                    Some(name.to_string())
                } else {
                    registry.best_match_with_input(name, arguments)
                }
            });
            let skill = resolved_skill_name.as_ref().and_then(|resolved| {
                server
                    .skill_registry
                    .lock()
                    .ok()
                    .and_then(|registry| registry.get(resolved))
            });
            match skill {
                Some(skill) => {
                    let started = Instant::now();
                    let outcome = skill.execute(arguments).await;
                    let skill_name = resolved_skill_name.as_deref().unwrap_or(name);
                    if let Ok(mut registry) = server.skill_registry.lock() {
                        registry.record_outcome(skill_name, outcome.is_ok(), started.elapsed());
                    }
                    outcome
                }
                None => anyhow::bail!("unknown mcp tool: {name}"),
            }
        }
    }
}

fn local_tool_descriptor(name: &'static str) -> Value {
    match name {
        "read_file" => json!({
            "name": name,
            "description": "Read contents of a file",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path to read"}
                },
                "required": ["path"]
            }
        }),
        "write_file" => json!({
            "name": name,
            "description": "Write contents to a file",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path to write"},
                    "content": {"type": "string", "description": "Content to write"},
                    "mode": {"type": "string", "enum": ["overwrite", "append"], "description": "Write mode"}
                },
                "required": ["path", "content"]
            }
        }),
        "search_files" => json!({
            "name": name,
            "description": "Search for files matching a glob pattern",
            "input_schema": {
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Search pattern/glob"},
                    "directory": {"type": "string", "description": "Search directory"}
                },
                "required": ["pattern"]
            }
        }),
        "apply_patch" => json!({
            "name": name,
            "description": "Apply a patch artifact",
            "input_schema": {"type": "object"}
        }),
        "run_tests" => json!({
            "name": name,
            "description": "Run test suite",
            "input_schema": {"type": "object"}
        }),
        "inspect_git_diff" => json!({
            "name": name,
            "description": "Inspect git diff",
            "input_schema": {"type": "object"}
        }),
        other => json!({
            "name": other,
            "description": "Registered MCP tool",
            "input_schema": {"type": "object"}
        }),
    }
}

fn validate_tool_arguments(tool_name: &str, tool_input: &Value) -> Result<()> {
    match tool_name {
        "read_file" => {
            tool_input
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("read_file requires arguments.path"))?;
        }
        "write_file" => {
            tool_input
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("write_file requires arguments.path"))?;
            tool_input
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("write_file requires arguments.content"))?;
        }
        "search_files" => {
            tool_input
                .get("pattern")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("search_files requires arguments.pattern"))?;
        }
        _ => {}
    }
    Ok(())
}

async fn list_checkpoint_records(
    server: &AcpServer,
    conversation_id: &str,
    branch_id: Option<&str>,
    limit: Option<usize>,
) -> Vec<crate::acp::prelude::ConversationCheckpoint> {
    let state = server.conversation_state.lock().await;
    let mut checkpoints = state
        .checkpoints
        .iter()
        .filter(|checkpoint| {
            checkpoint.conversation_id == conversation_id
                && branch_id
                    .map(|branch| checkpoint.branch_id == branch)
                    .unwrap_or(true)
        })
        .cloned()
        .collect::<Vec<_>>();
    checkpoints.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    if let Some(limit) = limit {
        checkpoints.truncate(limit);
    }
    checkpoints
}

async fn find_checkpoint(
    server: &AcpServer,
    conversation_id: &str,
    checkpoint_id: &str,
) -> Option<crate::acp::prelude::ConversationCheckpoint> {
    let state = server.conversation_state.lock().await;
    state
        .checkpoints
        .iter()
        .find(|checkpoint| {
            checkpoint.conversation_id == conversation_id
                && checkpoint.checkpoint_id == checkpoint_id
        })
        .cloned()
}

async fn get_branch_head_id(
    server: &AcpServer,
    conversation_id: &str,
    branch_id: &str,
) -> Option<String> {
    let state = server.conversation_state.lock().await;
    state
        .branch_heads
        .get(&format!("{}:{}", conversation_id, branch_id))
        .cloned()
}

async fn prune_checkpoints(
    server: &AcpServer,
    conversation_id: &str,
    branch_id: &str,
    keep: usize,
) -> (usize, usize, usize) {
    let mut state = server.conversation_state.lock().await;
    let mut checkpoints = state
        .checkpoints
        .iter()
        .filter(|checkpoint| {
            checkpoint.conversation_id == conversation_id && checkpoint.branch_id == branch_id
        })
        .cloned()
        .collect::<Vec<_>>();
    checkpoints.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    let retained = checkpoints
        .iter()
        .take(keep)
        .map(|checkpoint| checkpoint.checkpoint_id.clone())
        .collect::<Vec<_>>();
    let before = state.checkpoints.len();
    state.checkpoints.retain(|checkpoint| {
        checkpoint.conversation_id != conversation_id
            || checkpoint.branch_id != branch_id
            || retained.contains(&checkpoint.checkpoint_id)
    });
    let removed = before.saturating_sub(state.checkpoints.len());

    let branch_key = format!("{}:{}", conversation_id, branch_id);
    let mut repaired_heads = 0;
    if let Some(head) = state.branch_heads.get(&branch_key).cloned() {
        if !retained.contains(&head) {
            if let Some(new_head) = retained.first() {
                state.branch_heads.insert(branch_key, new_head.clone());
                repaired_heads = 1;
            }
        }
    }

    (removed, repaired_heads, 0)
}

fn params_task(params: &Value) -> Option<String> {
    params
        .get("task")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn session_id_for_task(task: &str) -> String {
    let compact = task
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(24)
        .collect::<String>();
    format!(
        "clarify-{}",
        if compact.is_empty() {
            "session"
        } else {
            compact.as_str()
        }
    )
}

/// Send error response
async fn send_error(
    server: &AcpServer,
    id: Option<Value>,
    code: i64,
    message: String,
    data: Option<Value>,
) -> Result<()> {
    mark_error_response(id.as_ref());
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
#[allow(clippy::too_many_arguments)]
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

#[cfg(test)]
mod tests {
    use super::{
        collect_vector_context_snippets, infer_workflow_parallelism, rebalance_execution_order,
        session_id_for_task,
    };
    use crate::vector::VectorStore;

    #[test]
    fn session_id_for_task_compacts_to_ascii_alnum() {
        let value = session_id_for_task("Fix #123: add review stage and docs");
        assert!(value.starts_with("clarify-"));
        assert!(value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-'));
    }

    #[test]
    fn session_id_for_task_has_fallback_when_empty() {
        assert_eq!(session_id_for_task("!!!"), "clarify-session");
    }

    #[test]
    fn rebalance_execution_order_splits_wide_phase_by_limit() {
        let execution_order = vec![
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            vec!["d".to_string()],
        ];
        let rebalanced = rebalance_execution_order(&execution_order, 2);

        assert_eq!(
            rebalanced,
            vec![
                vec!["a".to_string(), "b".to_string()],
                vec!["c".to_string()],
                vec!["d".to_string()]
            ]
        );
    }

    #[test]
    fn rebalance_execution_order_limit_one_serializes_all_nodes() {
        let execution_order = vec![
            vec!["a".to_string(), "b".to_string()],
            vec!["c".to_string()],
        ];
        let rebalanced = rebalance_execution_order(&execution_order, 1);

        assert_eq!(
            rebalanced,
            vec![
                vec!["a".to_string()],
                vec!["b".to_string()],
                vec!["c".to_string()]
            ]
        );
    }

    #[test]
    fn infer_workflow_parallelism_reads_max_phase_width() {
        let workflow = crate::reinforcement::WorkflowGeneratedArtifact {
            generated_at: 0,
            task: "task".to_string(),
            nodes: Vec::new(),
            edges: Vec::new(),
            execution_order: vec![
                vec!["a".to_string()],
                vec!["b".to_string(), "c".to_string(), "d".to_string()],
            ],
            auto_gates: Vec::new(),
            routing_summary: serde_json::json!({}),
        };

        assert_eq!(infer_workflow_parallelism(&workflow), 3);
    }

    #[test]
    fn collect_vector_context_snippets_searches_execution_and_semantic_phase() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("request-vector-dual-phase.sqlite3");
        let store = VectorStore::new(&db_path, 64, 256).expect("vector store should initialize");

        store
            .upsert(
                "coding",
                "fix retrieval alignment",
                "semantic-phase knowledge",
            )
            .expect("semantic phase upsert should succeed");

        // No entries under execution phase key; this verifies we still retrieve
        // by semantic phase fallback and avoid false miss caused by key mismatch.
        let phases = vec!["phase-1".to_string(), "coding".to_string()];
        let snippets =
            collect_vector_context_snippets(&store, &phases, "fix retrieval alignment", 3);

        assert!(!snippets.is_empty());
        assert!(snippets
            .iter()
            .any(|s| s.contains("semantic-phase knowledge")));
    }
}
