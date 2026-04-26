use crate::protocol::access_mode::{request_dispatch_mode, RequestDispatchMode};

/// Read protocol mode from config.toml / runtime_config.
fn get_protocol_mode(server: &AcpServer) -> RequestDispatchMode {
    // Try reading protocol_mode from runtime_config.
    request_dispatch_mode(server.runtime_config.protocol_mode.as_deref())
}

/// Returns true if the method belongs to the MCP protocol.
fn is_mcp_request(method: &str) -> bool {
    method.starts_with("mcp.") || method == "mcp.initialize"
}

/// Returns true if the method belongs to the ACP/A2A protocol.
fn is_acp_request(method: &str) -> bool {
    // Common ACP/A2A JSON-RPC methods.
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
            | "health.probes"
            | "lock.status"
            | "runtime.self_model"
            | "provider.status"
            | "release.readiness"
            | "runtime.stability"
            | "observability.alerts"
            | "security.baseline"
            | "harness.status"
            | "breaker.status"
            | "breaker.reset"
            | "breaker.recovery"
            | "cache.clear"
            | "vector.clear"
            | "maintenance.gc"
                | "data.lifecycle"
               | "error.contract"
            | "action.check"
            | "conversation.checkpoint.create"
            | "conversation.checkpoint.list"
            | "conversation.rollback"
            | "conversation.checkpoint.prune"
            | "config.reload"
            | "config.baseline"
            | "build.repro"
            | "optimization.peak"
            | "autotune.get"
            | "autotune.status"
            | "autotune.reset"
            | "selector.status"
            | "hardness.status"
            | "cost.status"
            | "workflow.confirm"
            | "workflow.clarify"
            | "workflow.research"
            | "workflow.consult"
            | "workflow.generate"
            | "workflow.execute"
            | "task.plan"
            | "task.execute"
            | "learning.summary"
            | "learning.replay"
            | "learning.guardrail"
            | "knowledge.distill"
            | "rl.alignment.offline_eval"
                | "governance.plan.get"
                | "governance.plan.update"
                | "governance.audit.recent"
                | "skill.import"
                | "skill.enable"
                | "skill.disable"
                | "skill.list_imported"
                | "skill.remove"
            | "phase.policy.replay"
            | "primary_secondary.summary"
            | "governance.status"
             // diagnostics / ops also used by vscode-addon in ACP mode
             | "metrics.reset"
             | "trace.get"
             | "trace.metrics"
             | "debug_panel.get"
             | "debug.panel.get"
            // MCP-bridge methods that ACP stdio also dispatches
            | "mcp.tools.list"
            | "mcp.tools.call"
    )
}
// Request handling implementation functions for ACP server
//
// This module contains standalone functions that implement request handling
// functionality previously in the `impl AcpServer` block in `impl/request.rs`.
// These functions take `AcpServer` as the first parameter to maintain
// compatibility with the original implementation.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Instant, UNIX_EPOCH};

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::Duration;
use tracing::{debug, info};

// Task-local: carries the current dispatch method through send_result for universal profile injection
tokio::task_local! {
    static DISPATCH_REQUEST_METHOD: String;
}

use crate::acp::background::run_maintenance_cycle;
use crate::acp::helpers::context::{
    probe_agent_runtime_readiness, run_with_optional_timeout, AgentRuntimeReadiness,
};
use crate::acp::helpers::metrics::{
    build_prometheus_metrics, CircuitBreakerSnapshot as PrometheusCircuitBreakerSnapshot,
    LifecycleSnapshot as PrometheusLifecycleSnapshot,
    MaintenanceSnapshot as PrometheusMaintenanceSnapshot,
    MetricsSnapshot as PrometheusMetricsSnapshot, RuntimeGaugeSnapshot,
};
use crate::acp::prelude::{
    enforce_checkpoint_capacity, with_acp_lock, AcpLockSnapshot, ACP_LOCK_PHASE_RATE_LIMITER,
};
use crate::acp::r#impl::storage::cache_clear;
use crate::acp::server::AcpServer;
use crate::agent::{AgentAuditLog, AgentTaskEnvelope, Message};
use crate::config::{
    collect_config_warnings, collect_production_strict_violations, validate_runtime_readiness,
    AppConfig, AutoTuneState,
};
use crate::evaluation::TraceEvent;

use crate::acp::helpers::policy::{rank_execution_agents, resolve_review_policy};
use crate::acp::helpers::requirement::{
    evaluate_requirement_gate_facade, parse_requirement_contract_from_params,
    resolve_learning_clarification_metrics,
};
use crate::flow_with_models::FlowModelSelector;
use crate::governance::hardening::{
    enforce_action, policy_bundle_for_target, task_budget_for_target, AuditLogger,
    AutonomousEditAuditEntry, BudgetTracker, GovernanceAction, Idempotency, IdempotencyCache,
};
use crate::i18n::runtime::{t, tf};
use crate::memory_module::{MemoryClass, MemoryEntry, MemoryPromotionReport, MemoryStore};
use crate::orchestration::skill_import::{
    ImportedSkillRecord, SkillImportManifest, SkillImportPolicy, SkillImportRequest,
    SkillImportStore,
};
use crate::orchestration::task_router::TaskRouter;
use crate::pua::{
    load_learning_records, DynamicQualityCompass, LearningRecord, PuaExecutionReport,
    PuaFeedbackCollector, PuaRuleEngine, PuaStageRequirement, TaskContext, TaskType,
};
use crate::reinforcement::{
    build_runtime_healthcheck_report, build_task_plan, build_workflow_generated_artifact,
    persist_clarification_session_artifact, persist_consultation_artifact,
    persist_execution_decision, persist_primary_secondary_failover_artifact,
    persist_primary_secondary_policy_artifact, persist_requirement_contract,
    persist_task_execution_summary, persist_task_graph_checkpoint, persist_task_plan,
    persist_workflow_generated, persist_workflow_learning_event, persist_workflow_research,
    recommend_agent_order_from_execution_history, recommend_failure_strategy_from_learning,
    recommend_parallelism_from_learning, recommend_predicted_success_rate_from_learning,
    recommend_work_grade_from_learning, run_action_check, ActionCheckKind, ArtifactLedger,
    CheckStatus, ClarificationSessionArtifact, ConsultationArtifact, ExecutionAssignmentRecord,
    ExecutionDecisionArtifact, ExecutionDecisionCandidate, KnowledgeBusArtifact,
    ParallelPhaseDecisionRecord, PrimaryFailoverReportItem, PrimarySecondaryFailoverArtifact,
    PrimarySecondaryPolicyArtifact, RequirementContractArtifact, TaskExecutionMetrics,
    TaskExecutionSummary, WorkflowGeneratedArtifact, WorkflowLearningBusArtifact,
    WorkflowLearningEvent, WorkflowResearchArtifact,
};
use crate::tool::{ToolInput, ToolRegistry};
use crate::vector::VectorStore;

use crate::rpc_protocol::{value_to_id, JsonRpcRequest, RequestTraceContext};

mod chat_pack;
mod checkpoint_pack;
mod config_pack;
mod exec_pack;
mod governance_pack;
mod hardness_pack;
mod learning_pack;
mod lifecycle_pack;
mod ops_pack;
mod protocol_pack;
mod pua_pack;
mod repro_pack;
mod runtime_pack;
mod tools_pack;
mod trace_pack;
mod workflow_pack;
use self::chat_pack::*;
use self::checkpoint_pack::*;
use self::config_pack::*;
use self::exec_pack::*;
pub use self::governance_pack::build_knowledge_refinement_profile;
pub use self::governance_pack::build_learning_profile;
pub(crate) use self::governance_pack::inject_platform_profiles_if_absent;
use self::governance_pack::*;
use self::hardness_pack::*;
use self::lifecycle_pack::*;
use self::ops_pack::*;
pub use self::protocol_pack::record_tool_call_audit_with_protocol;
use self::protocol_pack::*;
use self::pua_pack::*;
use self::tools_pack::*;
use self::trace_pack::*;

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
    // Adaptive protocol dispatch: route to ACP, MCP, or Auto based on config.
    let protocol_mode = get_protocol_mode(server);
    let method = request.method.as_str();
    match protocol_mode {
        RequestDispatchMode::Acp => {
            if !is_acp_request(method) {
                return send_error(
                    server,
                    request.id,
                    -32601,
                    format!("ACP mode does not support method: {}", method),
                    None,
                )
                .await;
            }
        }
        RequestDispatchMode::Mcp => {
            if !is_mcp_request(method) {
                return send_error(
                    server,
                    request.id,
                    -32601,
                    format!("MCP mode does not support method: {}", method),
                    None,
                )
                .await;
            }
        }
        RequestDispatchMode::Auto => {
            // If MCP method, prefer MCP branch; otherwise fall through to ACP.
            // Mixed-protocol requests are allowed in Auto mode.
        }
    }

    let pua_engine = PuaRuleEngine::new(server.pua_enforcement_plan.clone());
    let task_type = infer_task_type(method, &request.params);
    let task_context = TaskContext {
        task_type: task_type.clone(),
        file_count: infer_file_count(&request.params),
        risk_score: infer_risk_score(method, &task_type),
    };
    let dynamic_compass = DynamicQualityCompass::default();
    let dynamic_checks = dynamic_compass.get_checks(&task_context);
    let dynamic_check_descriptions = dynamic_checks
        .iter()
        .map(|check| check.description.clone())
        .collect::<Vec<_>>();

    if let Err(violation) = pua_engine.check_red_lines(method) {
        return send_error(
            server,
            request.id,
            -32003,
            format!("PUA red line violation: {}", violation.detail),
            Some(json!({
                "type": "pua_violation",
                "kind": format!("{:?}", violation.kind),
                "method": method,
                "detail": violation.detail,
                "quality_compass": dynamic_check_descriptions,
            })),
        )
        .await;
    }
    if let Some(stage) = infer_pua_stage(method) {
        let completed_actions = extract_pua_completed_actions(&request.params, method);
        let required_actions = pua_engine.collect_evidence(stage);
        let report = if required_actions.is_empty() {
            build_pua_execution_report(
                stage,
                &completed_actions,
                &required_actions,
                task_context.risk_score,
            )
        } else {
            pua_engine.generate_report(stage, &completed_actions)
        };
        if let Err(err) = pua_feedback_collector().collect(&report) {
            debug!("failed to persist PUA feedback report: {}", err);
        }
        if pua_report_enabled(server, &request.params) {
            if let Some(encoded) = encode_pua_report(&report) {
                stash_pua_report(request.id.as_ref(), encoded);
            }
        }

        if completed_actions.len() > 1 {
            if let Err(violation) = pua_engine.validate_stage(stage, &completed_actions) {
                return send_error(
                    server,
                    request.id,
                    -32003,
                    format!("PUA stage violation: {}", violation.detail),
                    Some(json!({
                        "type": "pua_violation",
                        "kind": format!("{:?}", violation.kind),
                        "stage": stage,
                        "method": method,
                        "detail": violation.detail,
                        "quality_compass": dynamic_check_descriptions,
                    })),
                )
                .await;
            }
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
    let dispatch_method = request.method.clone();
    let result = DISPATCH_REQUEST_METHOD
        .scope(dispatch_method, async {
            match request.method.as_str() {
                "initialize" => protocol_pack::handle_initialize(server, request_id).await,
                "mcp.initialize" => protocol_pack::handle_mcp_initialize(server, request_id).await,
                "mcp.tools.list" => protocol_pack::handle_mcp_tools_list(server, request_id).await,
                "mcp.tools.call" => {
                    protocol_pack::handle_mcp_tools_call(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "skill.import" => {
                    protocol_pack::handle_skill_import(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "skill.enable" => {
                    protocol_pack::handle_skill_enabled_toggle(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                        true,
                    )
                    .await
                }
                "skill.disable" => {
                    protocol_pack::handle_skill_enabled_toggle(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                        false,
                    )
                    .await
                }
                "skill.list_imported" => {
                    protocol_pack::handle_skill_list_imported(server, request_id).await
                }
                "skill.remove" => {
                    protocol_pack::handle_skill_remove(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "chat" => {
                    protocol_pack::handle_chat(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                        &trace,
                    )
                    .await
                }
                "phase" | "phase.status" => {
                    protocol_pack::handle_phase(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                        &trace,
                    )
                    .await
                }
                "metrics.get" => runtime_pack::handle_metrics_get(server, request_id).await,
                "metrics" => runtime_pack::handle_metrics(server, request_id).await,
                "metrics.prometheus" => {
                    runtime_pack::handle_metrics_prometheus(server, request_id).await
                }
                "metrics.reset" => runtime_pack::handle_metrics_reset(server, request_id).await,
                "debug_panel.get" | "debug.panel.get" => {
                    runtime_pack::handle_debug_panel_get(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "trace.get" => {
                    runtime_pack::handle_trace_get(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "trace.metrics" => runtime_pack::handle_trace_metrics(server, request_id).await,
                "shutdown" => runtime_pack::handle_shutdown(server, request_id).await,
                "health" | "runtime.health" => {
                    runtime_pack::handle_health(server, request_id).await
                }
                "health.probes" => runtime_pack::handle_health_probes(server, request_id).await,
                "lock.status" => {
                    handle_lock_status(server, request.params.unwrap_or_default(), request_id).await
                }
                "runtime.self_model" => {
                    runtime_pack::handle_runtime_self_model(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "provider.status" => {
                    runtime_pack::handle_provider_status(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "release.readiness" => {
                    handle_release_readiness(server, request.params.unwrap_or_default(), request_id)
                        .await
                }
                "runtime.stability" => {
                    runtime_pack::handle_runtime_stability(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "observability.alerts" => {
                    handle_observability_alerts(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "security.baseline" => {
                    handle_security_baseline(server, request.params.unwrap_or_default(), request_id)
                        .await
                }
                "harness.status" => {
                    handle_harness_status(server, request.params.unwrap_or_default(), request_id)
                        .await
                }
                "breaker.status" => handle_breaker_status(server, request_id).await,
                "breaker.reset" => {
                    handle_breaker_reset(server, request.params.unwrap_or_default(), request_id)
                        .await
                }
                "breaker.recovery" => {
                    handle_breaker_recovery(server, request.params.unwrap_or_default(), request_id)
                        .await
                }
                "cache.clear" => handle_cache_clear(server, request_id).await,
                "vector.clear" => handle_vector_clear(server, request_id).await,
                "maintenance.gc" => handle_maintenance_gc(server, request_id).await,
                "data.lifecycle" => {
                    handle_data_lifecycle(server, request.params.unwrap_or_default(), request_id)
                        .await
                }
                "action.check" => {
                    runtime_pack::handle_action_check(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "conversation.checkpoint.create" => {
                    runtime_pack::handle_conversation_checkpoint_create(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "conversation.checkpoint.list" => {
                    runtime_pack::handle_conversation_checkpoint_list(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "conversation.rollback" => {
                    runtime_pack::handle_conversation_rollback(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "conversation.checkpoint.prune" => {
                    runtime_pack::handle_conversation_checkpoint_prune(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "config.reload" => handle_config_reload(server, request_id).await,
                "config.baseline" => {
                    handle_config_baseline(server, request.params.unwrap_or_default(), request_id)
                        .await
                }
                "build.repro" => repro_pack::handle_build_repro(server, request_id).await,
                "optimization.peak" => {
                    runtime_pack::handle_optimization_peak(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "error.contract" => runtime_pack::handle_error_contract(server, request_id).await,
                "autotune.get" => runtime_pack::handle_autotune_get(server, request_id).await,
                "autotune.status" => runtime_pack::handle_autotune_status(server, request_id).await,
                "autotune.reset" => {
                    runtime_pack::handle_autotune_reset(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "selector.status" => runtime_pack::handle_selector_status(server, request_id).await,
                "hardness.status" => {
                    runtime_pack::handle_hardness_status(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "cost.status" => {
                    runtime_pack::handle_cost_status(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "workflow.confirm" => {
                    workflow_pack::handle_workflow_confirm(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                        &trace,
                    )
                    .await
                }
                "workflow.clarify" => {
                    workflow_pack::handle_workflow_clarify(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                        &trace,
                    )
                    .await
                }
                "workflow.research" => {
                    workflow_pack::handle_workflow_research(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                        &trace,
                    )
                    .await
                }
                "workflow.consult" => {
                    workflow_pack::handle_workflow_consult(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                        &trace,
                    )
                    .await
                }
                "workflow.generate" => {
                    workflow_pack::handle_workflow_generate(
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
                    workflow_pack::handle_task_plan(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                        &trace,
                    )
                    .await
                }
                "task.execute" => {
                    handle_task_execute(server, request.params.unwrap_or_default(), request_id)
                        .await
                }
                "learning.summary" => {
                    learning_pack::handle_learning_summary(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "learning.replay" => {
                    learning_pack::handle_learning_replay(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "learning.guardrail" => {
                    learning_pack::handle_learning_guardrail(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "knowledge.distill" => {
                    learning_pack::handle_knowledge_distill(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "rl.alignment.offline_eval" => {
                    learning_pack::handle_rl_alignment_offline_eval(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "governance.status" => {
                    runtime_pack::handle_governance_status(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "governance.plan.get" => {
                    runtime_pack::handle_governance_plan_get(server, request_id).await
                }
                "governance.plan.update" => {
                    runtime_pack::handle_governance_plan_update(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "governance.audit.recent" => {
                    runtime_pack::handle_governance_audit_recent(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "phase.policy.replay" => {
                    learning_pack::handle_phase_policy_replay(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "primary_secondary.summary" => {
                    learning_pack::handle_primary_secondary_summary(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
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
            }
        })
        .await
        .map_err(|error| attach_request_dispatch_context(error, request.method.as_str()));

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
            record_mcp_tool_audit(name, &arguments, false, &err.to_string());
            return send_error(server, request_id, -32602, err.to_string(), None).await;
        }
    };
    record_mcp_tool_audit(name, &arguments, true, "tool executed successfully");

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
    let snapshot = serde_json::to_value(server.metrics.snapshot())?;
    // Keep flat fields for backward compat AND add wrapper keys for new consumers
    let mut result = snapshot.clone();
    if let Value::Object(ref mut map) = result {
        map.insert("ok".to_string(), json!(true));
        map.insert("metrics".to_string(), snapshot);
    }
    send_result(server, request_id, result).await
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
            agent_timeout_failures_total: metrics.agent_timeout_failures_total,
            runtime_probe_timeout_total: metrics.runtime_probe_timeout_total,
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
            "timeouts": {
                "agent_request_total": metrics.agent_timeout_failures_total,
                "review_gate_total": metrics.review_gate_timeout_total,
                "runtime_probe_total": metrics.runtime_probe_timeout_total,
            },
            "timestamp": status.timestamp,
        }),
    )
    .await
}

fn check_status_label(value: CheckStatus) -> &'static str {
    match value {
        CheckStatus::Healthy => "healthy",
        CheckStatus::Warn => "warn",
        CheckStatus::Error => "error",
        CheckStatus::Skipped => "skipped",
    }
}

fn build_health_probes_payload(server: &AcpServer) -> Result<Value> {
    let status = server.get_status();
    let metrics = server.metrics.snapshot();

    let config_path = server.config_path.as_deref().map(Path::new);
    let report = build_runtime_healthcheck_report(
        config_path,
        server.response_cache.as_deref(),
        server.vector_store.as_deref(),
    )?;

    let healthy_count = report
        .components
        .iter()
        .filter(|item| item.status == CheckStatus::Healthy)
        .count();
    let warn_count = report
        .components
        .iter()
        .filter(|item| item.status == CheckStatus::Warn)
        .count();
    let error_count = report
        .components
        .iter()
        .filter(|item| item.status == CheckStatus::Error)
        .count();
    let skipped_count = report
        .components
        .iter()
        .filter(|item| item.status == CheckStatus::Skipped)
        .count();

    let readiness_status = if error_count > 0 {
        "not_ready"
    } else if warn_count > 0 {
        "degraded"
    } else {
        "ready"
    };

    let liveness_ok = status.lifecycle.is_healthy || status.lifecycle.shutdown_requested;
    let liveness_status = if liveness_ok { "alive" } else { "degraded" };

    let circuit_breakers = status
        .circuit_breakers
        .iter()
        .map(|item| {
            json!({
                "name": item.name,
                "state": item.state,
                "failure_count": item.failure_count,
                "success_count": item.success_count,
                "last_state_change": item.last_state_change,
                "total_failures": item.total_failures,
                "total_successes": item.total_successes,
            })
        })
        .collect::<Vec<_>>();

    let rate_limiter_buckets = with_acp_lock(
        server.lock_monitor.as_ref(),
        ACP_LOCK_PHASE_RATE_LIMITER,
        server.phase_rate_limiter.as_ref(),
        |guard| {
            guard
                .snapshot()
                .into_iter()
                .map(|(phase, (tokens, capacity))| {
                    json!({
                        "phase": phase,
                        "tokens": tokens,
                        "capacity": capacity,
                        "used_percent": if capacity > 0.0 { ((capacity - tokens) / capacity * 100.0).clamp(0.0, 100.0) } else { 0.0 },
                    })
                })
                .collect::<Vec<_>>()
        },
    );

    let lock_components = server.lock_monitor.snapshot();
    let lock_summary = summarize_lock_health(&lock_components);
    let timeout_status = if metrics.agent_timeout_failures_total > 0
        || metrics.review_gate_timeout_total > 0
        || metrics.runtime_probe_timeout_total > 0
    {
        "warn"
    } else {
        "healthy"
    };

    let mut dependencies = report
        .components
        .iter()
        .map(|item| {
            json!({
                "name": item.name,
                "status": check_status_label(item.status),
                "message": item.message,
                "details": item.details,
            })
        })
        .collect::<Vec<_>>();
    dependencies.push(json!({
        "name": "locks",
        "status": lock_summary.status,
        "message": format!(
            "poisoned={}, recovered={}, slow_waits={}",
            lock_summary.poisoned_total, lock_summary.recovered_total, lock_summary.slow_wait_total
        ),
        "details": {
            "poisoned_total": lock_summary.poisoned_total,
            "recovered_total": lock_summary.recovered_total,
            "slow_wait_total": lock_summary.slow_wait_total,
            "max_wait_ms": lock_summary.max_wait_ms,
            "components_tracked": lock_summary.components_tracked,
        }
    }));
    dependencies.push(json!({
        "name": "timeouts",
        "status": timeout_status,
        "message": format!(
            "agent={}, review_gate={}, runtime_probe={}",
            metrics.agent_timeout_failures_total,
            metrics.review_gate_timeout_total,
            metrics.runtime_probe_timeout_total,
        ),
        "details": {
            "agent_request_total": metrics.agent_timeout_failures_total,
            "review_gate_total": metrics.review_gate_timeout_total,
            "runtime_probe_total": metrics.runtime_probe_timeout_total,
        }
    }));

    Ok(json!({
        "ok": true,
        "probes": {
            "liveness": {
                "status": liveness_status,
                "ok": liveness_ok,
                "shutting_down": status.lifecycle.shutdown_requested,
                "uptime_seconds": status.lifecycle.uptime_seconds,
            },
            "readiness": {
                "status": readiness_status,
                "ok": error_count == 0,
                "overall_status": check_status_label(report.overall_status),
                "generated_at": report.generated_at,
            },
            "summary": {
                "healthy": healthy_count,
                "warn": warn_count,
                "error": error_count,
                "skipped": skipped_count,
            },
            "dependencies": dependencies,
            "circuit_breakers": circuit_breakers,
            "rate_limiter": {
                "tracked": rate_limiter_buckets.len(),
                "buckets": rate_limiter_buckets,
            },
            "locks": {
                "status": lock_summary.status,
                "poisoned_total": lock_summary.poisoned_total,
                "recovered_total": lock_summary.recovered_total,
                "slow_wait_total": lock_summary.slow_wait_total,
                "max_wait_ms": lock_summary.max_wait_ms,
                "components_tracked": lock_summary.components_tracked,
                "components": lock_components,
            },
            "timeouts": {
                "status": timeout_status,
                "agent_request_total": metrics.agent_timeout_failures_total,
                "review_gate_total": metrics.review_gate_timeout_total,
                "runtime_probe_total": metrics.runtime_probe_timeout_total,
            },
            "timestamp": status.timestamp,
        }
    }))
}

async fn handle_health_probes(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    send_result(server, request_id, build_health_probes_payload(server)?).await
}

fn build_runtime_stability_payload(server: &AcpServer) -> Result<Value> {
    let status = server.get_status();
    let _metrics = server.metrics.snapshot();
    let config_path = server.config_path.as_deref().map(Path::new);
    let report = build_runtime_healthcheck_report(
        config_path,
        server.response_cache.as_deref(),
        server.vector_store.as_deref(),
    )?;

    // Load config to check for warnings and production-strict violations.
    let mut config_warnings = Vec::new();
    let mut strict_violations = Vec::new();

    if let Some(cfg_path) = config_path {
        if let Ok(cfg) = AppConfig::load(cfg_path) {
            config_warnings = collect_config_warnings(cfg_path, &cfg);
            strict_violations = collect_production_strict_violations(&cfg);
        }
    }

    // Summarise health check component counts.
    let error_count = report
        .components
        .iter()
        .filter(|item| item.status == CheckStatus::Error)
        .count();
    let warn_count = report
        .components
        .iter()
        .filter(|item| item.status == CheckStatus::Warn)
        .count();

    // Check graceful-shutdown readiness (shutdown_requested + uptime).
    let graceful_shutdown_ready = !status.lifecycle.shutdown_requested;
    let uptime_seconds = status.lifecycle.uptime_seconds;

    // Verify config validity by attempting to load it.
    let config_valid = if let Some(cfg_path) = config_path {
        AppConfig::load(cfg_path).is_ok()
    } else {
        true // No config path provided — treat as valid.
    };

    // Compute stability score (0–100).
    let mut stability_score = 100;
    if error_count > 0 {
        stability_score -= (error_count as i32).min(30);
    }
    if warn_count > 0 {
        stability_score -= ((warn_count as i32) / 2).min(20);
    }
    if !graceful_shutdown_ready {
        stability_score -= 15;
    }
    if !config_valid {
        stability_score -= 25;
    }
    if !strict_violations.is_empty() {
        stability_score -= (strict_violations.len() as i32 * 5).min(30);
    }
    stability_score = stability_score.clamp(0, 100);

    // Determine stability tier.
    let stability_level = match stability_score {
        90..=100 => "excellent",
        75..=89 => "good",
        60..=74 => "fair",
        40..=59 => "poor",
        _ => "critical",
    };

    // Safe-restart requires graceful-shutdown support, valid config, and no strict violations.
    let safe_restart_ready =
        graceful_shutdown_ready && config_valid && strict_violations.is_empty();

    let mut checks = vec![
        json!({
            "name": "health_check",
            "status": if error_count == 0 { "pass" } else { "fail" },
            "errors": error_count,
            "warnings": warn_count,
            "description": format!("Health check: {} errors, {} warnings", error_count, warn_count),
        }),
        json!({
            "name": "graceful_shutdown",
            "status": if graceful_shutdown_ready { "pass" } else { "fail" },
            "uptime_seconds": uptime_seconds,
            "shutdown_requested": status.lifecycle.shutdown_requested,
            "description": if graceful_shutdown_ready {
                "Graceful shutdown capability ready".to_string()
            } else {
                "Graceful shutdown in progress or unavailable".to_string()
            },
        }),
        json!({
            "name": "config_validation",
            "status": if config_valid { "pass" } else { "fail" },
            "warning_count": config_warnings.len(),
            "description": format!("Config validation: {} warnings", config_warnings.len()),
        }),
    ];

    if !strict_violations.is_empty() {
        checks.push(json!({
            "name": "production_strict_mode",
            "status": "fail",
            "violation_count": strict_violations.len(),
            "violations": strict_violations.iter().take(5).map(|v| {
                json!({
                    "code": "strict_violation",
                    "message": v,
                })
            }).collect::<Vec<_>>(),
            "description": format!("Production strict mode: {} violations", strict_violations.len()),
        }));
    } else {
        checks.push(json!({
            "name": "production_strict_mode",
            "status": "pass",
            "violation_count": 0,
            "description": "No production strict mode violations".to_string(),
        }));
    }

    Ok(json!({
        "ok": true,
        "stability": {
            "score": stability_score,
            "level": stability_level,
            "safe_restart_ready": safe_restart_ready,
            "summary": {
                "health_errors": error_count,
                "health_warnings": warn_count,
                "uptime_seconds": uptime_seconds,
                "config_warnings": config_warnings.len(),
                "strict_violations": strict_violations.len(),
            },
            "checks": checks,
            "recommendation": if stability_score >= 75 {
                "System is stable. Safe to operate.".to_string()
            } else if stability_score >= 60 {
                "System has degraded capability. Review warnings before critical operations.".to_string()
            } else {
                "System is unstable. Address errors before restart or upgrades.".to_string()
            },
            "timestamp": status.timestamp,
        }
    }))
}

async fn handle_runtime_stability(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(server, request_id, build_runtime_stability_payload(server)?).await
}

async fn handle_runtime_self_model(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let payload = build_runtime_self_model_payload(server, &params)?;
    send_result(server, request_id, payload).await
}

fn build_runtime_self_model_payload(server: &AcpServer, params: &Value) -> Result<Value> {
    let probes_payload = build_health_probes_payload(server)?;
    let stability_payload = build_runtime_stability_payload(server)?;
    let offline_eval_payload = build_rl_alignment_offline_eval_payload(params);

    let probes = probes_payload
        .get("probes")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let stability = stability_payload
        .get("stability")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let offline_eval = offline_eval_payload
        .get("offline_eval")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let readiness_status = probes
        .get("readiness")
        .and_then(|value| value.get("status"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let safe_restart_ready = stability
        .get("safe_restart_ready")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let stability_level = stability
        .get("level")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let recommended_mode = offline_eval
        .get("decision")
        .and_then(|value| value.get("recommended_mode"))
        .and_then(Value::as_str)
        .unwrap_or("conservative");
    let fallback_triggered = offline_eval
        .get("decision")
        .and_then(|value| value.get("fallback_triggered"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let drift_alert = offline_eval
        .get("drift")
        .and_then(|value| value.get("alert"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let summary = stability
        .get("summary")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let warnings = offline_eval
        .get("warnings")
        .cloned()
        .unwrap_or_else(|| json!([]));

    let mut recommendations = Vec::new();
    if readiness_status != "ready" {
        recommendations.push(
            "Review runtime dependencies, probes, and breaker state before serving critical traffic."
                .to_string(),
        );
    }
    if !safe_restart_ready {
        recommendations.push(
            "Avoid restart or rollout until config validation and strict-mode constraints are green."
                .to_string(),
        );
    }
    if drift_alert || fallback_triggered {
        recommendations.push(
            "Keep runtime in conservative mode until reward drift and safety regressions recover."
                .to_string(),
        );
    }
    if recommendations.is_empty() {
        recommendations.push(
            "System is operating within the expected envelope; continue normal runtime supervision."
                .to_string(),
        );
    }

    let timestamp = probes
        .get("timestamp")
        .cloned()
        .or_else(|| stability.get("timestamp").cloned())
        .unwrap_or_else(|| json!(0));

    Ok(json!({
        "ok": true,
        "self_model": {
            "health": probes,
            "stability": stability,
            "drift": offline_eval.get("drift").cloned().unwrap_or_else(|| json!({})),
            "decision": {
                "recommended_mode": recommended_mode,
                "fallback_triggered": fallback_triggered,
                "readiness_status": readiness_status,
                "stability_level": stability_level,
                "safe_restart_ready": safe_restart_ready,
            },
            "constraints": {
                "shutdown_requested": probes
                    .get("liveness")
                    .and_then(|value| value.get("shutting_down"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                "health_errors": summary.get("health_errors").cloned().unwrap_or_else(|| json!(0)),
                "health_warnings": summary.get("health_warnings").cloned().unwrap_or_else(|| json!(0)),
                "config_warnings": summary.get("config_warnings").cloned().unwrap_or_else(|| json!(0)),
                "strict_violations": summary.get("strict_violations").cloned().unwrap_or_else(|| json!(0)),
            },
            "warnings": warnings,
            "recommendations": recommendations,
            "source_methods": ["health.probes", "runtime.stability", "rl.alignment.offline_eval"],
            "timestamp": timestamp,
        }
    }))
}

fn build_provider_status_payload(server: &AcpServer) -> Result<Value> {
    let status = server.get_status();
    let config_path = server.config_path.as_deref().map(Path::new);
    let report = build_runtime_healthcheck_report(
        config_path,
        server.response_cache.as_deref(),
        server.vector_store.as_deref(),
    )?;

    let provider_component = report
        .components
        .iter()
        .find(|item| item.name == "provider_dependencies");

    let provider_status = provider_component
        .map(|item| check_status_label(item.status))
        .unwrap_or("skipped");
    let provider_message = provider_component
        .map(|item| item.message.clone())
        .unwrap_or_else(|| "provider dependency snapshot unavailable".to_string());
    let provider_details = provider_component
        .map(|item| item.details.clone())
        .unwrap_or_else(|| json!({}));

    let ready = provider_details
        .get("ready")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let degraded = provider_details
        .get("degraded")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total = provider_details
        .get("total")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    let configured_agents = provider_details
        .get("agents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let registry_catalog = server
        .agent_registry()
        .map(|registry| {
            registry
                .models()
                .into_iter()
                .map(|(name, default_model, available_models)| {
                    json!({
                        "agent": name,
                        "default_model": default_model.as_ref().map(|item| item.id.clone()),
                        "available_models": available_models.len(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let configured_total = configured_agents.len() as u64;
    let catalog_total = registry_catalog.len() as u64;

    Ok(json!({
        "ok": true,
        "provider_status": {
            "status": provider_status,
            "message": provider_message,
            "summary": {
                "ready": ready,
                "degraded": degraded,
                "configured": total.max(configured_total),
                "registry": catalog_total,
                "coverage_percent": if total > 0 {
                    ((ready as f64 / total as f64) * 100.0).round()
                } else {
                    0.0
                },
            },
            "configured_agents": configured_agents,
            "registry_catalog": registry_catalog,
            "timestamp": status.timestamp,
        }
    }))
}

async fn handle_provider_status(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(server, request_id, build_provider_status_payload(server)?).await
}

async fn handle_governance_status(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let status = server.get_status();
    let runtime_snapshot = server.metrics.snapshot();

    let pua_plan = server
        .pua_enforcement_plan
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();

    let pua_learning = pua_feedback_collector()
        .extract_learning_data(200)
        .unwrap_or_default();
    let recent_failed = pua_learning.iter().filter(|record| !record.passed).count();
    let governance_audit = load_governance_audit_events(20).unwrap_or_default();

    let rules = governance_rule_fingerprint(server.config_path.as_deref());
    let config_summary = config_pack::governance_config_summary(server.config_path.as_deref());

    let entry_rate_snapshot = with_acp_lock(
        server.lock_monitor.as_ref(),
        ACP_LOCK_PHASE_RATE_LIMITER,
        server.phase_rate_limiter.as_ref(),
        |guard| guard.snapshot(),
    );
    let entry_sources_tracked = entry_rate_snapshot
        .keys()
        .filter(|name| name.starts_with("entry:"))
        .count();

    let breaker_open_count = status
        .circuit_breakers
        .iter()
        .filter(|item| item.state.eq_ignore_ascii_case("open"))
        .count();

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "governance": {
                "status": if status.lifecycle.is_healthy && recent_failed == 0 && breaker_open_count == 0 {
                    "healthy"
                } else {
                    "degraded"
                },
                "runtime": {
                    "is_healthy": status.lifecycle.is_healthy,
                    "shutting_down": status.lifecycle.shutdown_requested,
                    "uptime_seconds": status.lifecycle.uptime_seconds,
                },
                "rules": rules,
                "pua": {
                    "escalation_level": pua_plan.escalation_level,
                    "red_line_count": pua_plan.red_lines.len(),
                    "stage_requirement_count": pua_plan.stage_requirements.len(),
                    "mandatory_safeguards_count": pua_plan.mandatory_safeguards.len(),
                    "mandatory_evidence_count": pua_plan.mandatory_evidence.len(),
                },
                "violations": {
                    "pua_recent_total": pua_learning.len(),
                    "pua_recent_failed": recent_failed,
                    "review_gate_rejected_total": runtime_snapshot.review_gate_rejected_total,
                    "breaker_open_count": breaker_open_count,
                },
                "dynamic_rules": {
                    "runtime_mutable": true,
                    "red_line_count": pua_plan.red_lines.len(),
                    "stage_requirement_count": pua_plan.stage_requirements.len(),
                    "quality_compass_count": pua_plan.quality_compass.len(),
                },
                "audit": {
                    "recent_total": governance_audit.len(),
                    "recent": governance_audit,
                },
                "config": config_summary,
                "entry_guard": {
                    "auth_enabled": server.runtime_config.entry_auth_enabled,
                    "auth_key_env": server.runtime_config.entry_auth_api_key_env,
                    "auth_key_configured": std::env::var(&server.runtime_config.entry_auth_api_key_env)
                        .ok()
                        .map(|value| !value.trim().is_empty())
                        .unwrap_or(false),
                    "rate_limit_rpm": server.runtime_config.entry_rate_limit_rpm,
                    "rate_limit_burst": server.runtime_config.entry_rate_limit_burst,
                    "sources_tracked": entry_sources_tracked,
                },
                "timestamp": status.timestamp,
            }
        }),
    )
    .await
}

async fn handle_optimization_peak(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let status = server.get_status();
    let runtime_snapshot = server.metrics.snapshot();
    let config_summary = config_pack::governance_config_summary(server.config_path.as_deref());
    let repro_summary = repro_pack::reproducible_build_summary(server.config_path.as_deref());
    let pua_learning = pua_feedback_collector()
        .extract_learning_data(200)
        .unwrap_or_default();

    let total_requests = runtime_snapshot.total_requests.max(1) as f64;
    let failed_ratio = runtime_snapshot.failed_requests as f64 / total_requests;
    let review_reject_ratio = runtime_snapshot.review_gate_rejected_total as f64 / total_requests;
    let timeout_total = runtime_snapshot.agent_timeout_failures_total
        + runtime_snapshot.review_gate_timeout_total
        + runtime_snapshot.runtime_probe_timeout_total;
    let breaker_open_count = status
        .circuit_breakers
        .iter()
        .filter(|item| item.state.eq_ignore_ascii_case("open"))
        .count() as u64;

    let strict_enabled = config_summary
        .get("production_strict")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let strict_violation_count = config_summary
        .get("strict_violation_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let entry_auth_enabled = config_summary
        .get("entry_auth_enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let entry_auth_key_configured = config_summary
        .get("entry_auth_key_configured")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let required_total = repro_summary
        .get("reproducibility")
        .and_then(|value| value.get("required_total"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let required_present = repro_summary
        .get("reproducibility")
        .and_then(|value| value.get("required_present"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let missing_required = repro_summary
        .get("reproducibility")
        .and_then(|value| value.get("missing_required"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let recent_failed = pua_learning.iter().filter(|record| !record.passed).count() as u64;

    let task = params
        .get("task")
        .and_then(Value::as_str)
        .or_else(|| params.get("objective").and_then(Value::as_str))
        .unwrap_or("One-shot optimization peak validation");
    let hardness = summarize_hardness(task, &params);
    let cost = summarize_token_cost_governance(task, &params, hardness.clone(), &runtime_snapshot);
    let estimated_total_cost = cost.telemetry.estimated_total_cost;
    let budget_class = cost.budget.budget_class.clone();

    let max_failure_ratio = params
        .get("max_failure_ratio")
        .and_then(Value::as_f64)
        .unwrap_or(0.10);
    let max_review_reject_ratio = params
        .get("max_review_reject_ratio")
        .and_then(Value::as_f64)
        .unwrap_or(0.10);
    let max_timeout_total = params
        .get("max_timeout_total")
        .and_then(Value::as_u64)
        .unwrap_or(10);
    let max_estimated_cost = params
        .get("max_estimated_cost")
        .and_then(Value::as_f64)
        .unwrap_or(1.50);

    let quality_pass =
        failed_ratio <= max_failure_ratio && review_reject_ratio <= max_review_reject_ratio;
    let cost_pass = estimated_total_cost <= max_estimated_cost;
    let stability_pass = status.lifecycle.is_healthy
        && breaker_open_count == 0
        && timeout_total <= max_timeout_total;
    let security_pass = strict_enabled
        && strict_violation_count == 0
        && entry_auth_enabled
        && entry_auth_key_configured;
    let repro_pass = required_total == required_present && missing_required.is_empty();
    let governance_pass = recent_failed == 0;

    let gates = vec![
        json!({
            "name": "quality",
            "passed": quality_pass,
            "failure_ratio": failed_ratio,
            "max_failure_ratio": max_failure_ratio,
            "review_reject_ratio": review_reject_ratio,
            "max_review_reject_ratio": max_review_reject_ratio,
        }),
        json!({
            "name": "cost",
            "passed": cost_pass,
            "estimated_total_cost": estimated_total_cost,
            "max_estimated_cost": max_estimated_cost,
            "budget_class": budget_class,
        }),
        json!({
            "name": "stability",
            "passed": stability_pass,
            "runtime_healthy": status.lifecycle.is_healthy,
            "breaker_open_count": breaker_open_count,
            "timeout_total": timeout_total,
            "max_timeout_total": max_timeout_total,
        }),
        json!({
            "name": "security",
            "passed": security_pass,
            "production_strict": strict_enabled,
            "strict_violation_count": strict_violation_count,
            "entry_auth_enabled": entry_auth_enabled,
            "entry_auth_key_configured": entry_auth_key_configured,
        }),
        json!({
            "name": "reproducibility",
            "passed": repro_pass,
            "required_total": required_total,
            "required_present": required_present,
            "missing_required": missing_required,
        }),
        json!({
            "name": "governance",
            "passed": governance_pass,
            "pua_recent_total": pua_learning.len(),
            "pua_recent_failed": recent_failed,
        }),
    ];

    let overall_pass = gates
        .iter()
        .all(|gate| gate.get("passed").and_then(Value::as_bool) == Some(true));

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "peak": {
                "version": "x11-one-shot-optimization-peak-v1",
                "overall_pass": overall_pass,
                "status": if overall_pass { "peak_ready" } else { "needs_action" },
                "frozen_scope": ["X1", "X2", "X3", "X4", "X5", "X6", "X7", "X8", "X9", "X10"],
                "window": {
                    "sprint": params
                        .get("sprint")
                        .and_then(Value::as_str)
                        .unwrap_or("blue15-x11"),
                    "freeze_mode": params
                        .get("freeze_mode")
                        .and_then(Value::as_str)
                        .unwrap_or("strict"),
                },
                "task": task,
                "hardness": hardness,
                "cost": cost,
                "gates": gates,
                "summary": {
                    "total_requests": runtime_snapshot.total_requests,
                    "failed_requests": runtime_snapshot.failed_requests,
                    "review_gate_rejected_total": runtime_snapshot.review_gate_rejected_total,
                    "agent_timeout_failures_total": runtime_snapshot.agent_timeout_failures_total,
                    "review_gate_timeout_total": runtime_snapshot.review_gate_timeout_total,
                    "runtime_probe_timeout_total": runtime_snapshot.runtime_probe_timeout_total,
                    "uptime_seconds": status.lifecycle.uptime_seconds,
                },
                "timestamp": status.timestamp,
            }
        }),
    )
    .await
}

const GOVERNANCE_AUDIT_DIR: &str = ".goon/governance";
const GOVERNANCE_AUDIT_FILE: &str = "audit.ndjson";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GovernanceAuditEvent {
    timestamp: u64,
    action: String,
    actor: String,
    result: String,
    detail: Value,
}

fn append_governance_audit_event(event: &GovernanceAuditEvent) -> Result<()> {
    let dir = Path::new(GOVERNANCE_AUDIT_DIR);
    fs::create_dir_all(dir)?;
    let path = dir.join(GOVERNANCE_AUDIT_FILE);
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let line = serde_json::to_string(event)?;
    use std::io::Write;
    writeln!(file, "{}", line)?;
    Ok(())
}

fn load_governance_audit_events(limit: usize) -> Result<Vec<GovernanceAuditEvent>> {
    if limit == 0 {
        return Ok(Vec::new());
    }

    let path = Path::new(GOVERNANCE_AUDIT_DIR).join(GOVERNANCE_AUDIT_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(path)?;
    let mut events = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let event: GovernanceAuditEvent = serde_json::from_str(trimmed)?;
        events.push(event);
    }

    if events.len() > limit {
        Ok(events.split_off(events.len() - limit))
    } else {
        Ok(events)
    }
}

async fn handle_governance_plan_get(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    let plan = server
        .pua_enforcement_plan
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "plan": plan,
        }),
    )
    .await
}

async fn handle_governance_plan_update(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let mut plan = server
        .pua_enforcement_plan
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();

    if let Some(level) = params.get("escalation_level").and_then(Value::as_str) {
        plan.escalation_level = level.to_string();
    }
    if let Some(items) = params.get("red_lines").and_then(Value::as_array) {
        plan.red_lines = items
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect();
    }
    if let Some(items) = params.get("quality_compass").and_then(Value::as_array) {
        plan.quality_compass = items
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect();
    }
    if let Some(items) = params.get("mandatory_safeguards").and_then(Value::as_array) {
        plan.mandatory_safeguards = items
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect();
    }
    if let Some(items) = params.get("mandatory_evidence").and_then(Value::as_array) {
        plan.mandatory_evidence = items
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect();
    }
    if let Some(stage_requirements) = params.get("stage_requirements") {
        plan.stage_requirements =
            serde_json::from_value::<Vec<PuaStageRequirement>>(stage_requirements.clone())?;
    }

    if let Ok(mut guard) = server.pua_enforcement_plan.lock() {
        *guard = plan.clone();
    }

    let event = GovernanceAuditEvent {
        timestamp: crate::acp::prelude::now_ts().max(0) as u64,
        action: "governance.plan.update".to_string(),
        actor: "rpc".to_string(),
        result: "success".to_string(),
        detail: json!({
            "escalation_level": plan.escalation_level,
            "red_line_count": plan.red_lines.len(),
            "stage_requirement_count": plan.stage_requirements.len(),
            "mandatory_safeguards_count": plan.mandatory_safeguards.len(),
            "mandatory_evidence_count": plan.mandatory_evidence.len(),
        }),
    };
    let _ = append_governance_audit_event(&event);

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "plan": plan,
        }),
    )
    .await
}

async fn handle_governance_audit_recent(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(20)
        .clamp(1, 200);
    let events = load_governance_audit_events(limit).unwrap_or_default();

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "audit": {
                "limit": limit,
                "events": events,
            }
        }),
    )
    .await
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct LockHealthSummary {
    status: &'static str,
    poisoned_total: u64,
    recovered_total: u64,
    slow_wait_total: u64,
    max_wait_ms: f64,
    components_tracked: usize,
}

fn summarize_lock_health(components: &[AcpLockSnapshot]) -> LockHealthSummary {
    let poisoned_total = components
        .iter()
        .map(|item| item.poisoned_total)
        .sum::<u64>();
    let recovered_total = components
        .iter()
        .map(|item| item.recovered_total)
        .sum::<u64>();
    let slow_wait_total = components
        .iter()
        .map(|item| item.slow_wait_total)
        .sum::<u64>();
    let max_wait_ms = components
        .iter()
        .map(|item| item.max_wait_ms)
        .fold(0.0_f64, f64::max);
    let status = if poisoned_total > 0 || slow_wait_total > 0 || max_wait_ms >= 5.0 {
        "warn"
    } else {
        "healthy"
    };

    LockHealthSummary {
        status,
        poisoned_total,
        recovered_total,
        slow_wait_total,
        max_wait_ms,
        components_tracked: components.len(),
    }
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
    let mut rollback = create_checkpoint_record(
        server,
        conversation_id,
        branch_id,
        checkpoint.messages.clone(),
        Some(format!("rollback:{}", checkpoint_id)),
        Some(checkpoint_id.to_string()),
    )
    .await;
    let metacognitive_loop = persist_checkpoint_metacognitive_loop(
        server,
        conversation_id,
        branch_id,
        &rollback.checkpoint_id,
        checkpoint.metacognitive_loop.clone().unwrap_or_else(|| {
            json!({
                "active": true,
                "schema_version": "blue25-metacognitive-loop-v1",
                "last_reflection": format!("rollback:{}", checkpoint_id),
                "reflection_trigger": "rollback_restore",
            })
        }),
    )
    .await;
    rollback.metacognitive_loop = Some(metacognitive_loop.clone());

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "conversation_id": conversation_id,
            "branch_id": branch_id,
            "checkpoint": rollback,
            "metacognitive_loop": metacognitive_loop,
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

/// Handle autotune status request
async fn handle_autotune_status(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    let autotune_state = if let Some(autotune) = server.autotune.as_ref() {
        let lock = autotune.lock().await;
        Some(lock.clone())
    } else {
        None
    };

    let autotune_config = server.autotune_config.as_ref().cloned();
    let enabled = autotune_config
        .as_ref()
        .map(|cfg| cfg.enabled)
        .unwrap_or(false);

    send_result(
        server,
        request_id,
        // Keep "state" (backward compat) AND add "autotune" wrapper (new consumers)
        json!({
            "enabled": enabled,
            "state": autotune_state,
            "autotune": {
                "enabled": enabled,
                "state": autotune_state,
            },
        }),
    )
    .await
}

async fn handle_autotune_get(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    let Some(autotune) = server.autotune.as_ref() else {
        return send_result(
            server,
            request_id,
            json!({
                "enabled": false,
                "autotune": null,
                "params": null,
            }),
        )
        .await;
    };

    let state = autotune.lock().await;
    let snap = state.snapshot();
    // Keep flat fields for backward compat AND add wrapper keys for new consumers
    let mut result = snap.clone();
    if let Value::Object(ref mut map) = result {
        map.insert("enabled".to_string(), json!(true));
        map.insert("autotune".to_string(), snap.clone());
        map.insert("params".to_string(), snap);
    }
    send_result(server, request_id, result).await
}

async fn handle_selector_status(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    let snapshot = server
        .adaptive_model_selector
        .lock()
        .map(|selector| selector.snapshot())
        .unwrap_or_default();

    send_result(
        server,
        request_id,
        json!({
            "selector": snapshot,
        }),
    )
    .await
}

async fn handle_hardness_status(
    _server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let task = params
        .get("task")
        .and_then(Value::as_str)
        .or_else(|| params.get("objective").and_then(Value::as_str))
        .unwrap_or("");
    let hardness = summarize_hardness(task, &params);

    send_result(
        _server,
        request_id,
        json!({
            "ok": true,
            "hardness": hardness,
            "routing": {
                "mode": hardness.budget.recommended_mode,
                "parallelism_cap": hardness.budget.parallelism_cap,
                "timeout_seconds": hardness.budget.timeout_seconds,
                "required_reviews": hardness.budget.required_reviews,
            },
        }),
    )
    .await
}

async fn handle_error_contract(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    send_result(
        server,
        request_id,
        json!({
            "contract": {
                "version": "x8-error-contract-v1",
                "kinds": [
                    {
                        "kind": "InvalidParams",
                        "codes": [-32602],
                        "retry": {"retryable": false, "strategy": "none", "max_retries": 0}
                    },
                    {
                        "kind": "MethodNotFound",
                        "codes": [-32601],
                        "retry": {"retryable": false, "strategy": "none", "max_retries": 0}
                    },
                    {
                        "kind": "AuthRequired",
                        "codes": [-32003],
                        "retry": {"retryable": false, "strategy": "none", "max_retries": 0}
                    },
                    {
                        "kind": "RateLimited",
                        "codes": [-32029],
                        "retry": {"retryable": true, "strategy": "exponential_backoff", "base_delay_ms": 500, "max_delay_ms": 10000, "max_retries": 3}
                    },
                    {
                        "kind": "UpstreamTimeout",
                        "codes": [-32603],
                        "retry": {"retryable": true, "strategy": "exponential_backoff", "base_delay_ms": 500, "max_delay_ms": 10000, "max_retries": 3}
                    },
                    {
                        "kind": "PuaViolation",
                        "codes": [-32603],
                        "retry": {"retryable": false, "strategy": "none", "max_retries": 0}
                    },
                    {
                        "kind": "BudgetExceeded",
                        "codes": [-32603],
                        "retry": {"retryable": false, "strategy": "none", "max_retries": 0}
                    },
                    {
                        "kind": "SandboxBlocked",
                        "codes": [-32603],
                        "retry": {"retryable": false, "strategy": "none", "max_retries": 0}
                    },
                    {
                        "kind": "InternalError",
                        "codes": [-32603],
                        "retry": {"retryable": false, "strategy": "none", "max_retries": 0}
                    }
                ],
                "compatibility": {
                    "request_error_context_prefix": "acp.handle_request.dispatch"
                }
            }
        }),
    )
    .await
}

async fn handle_cost_status(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let task = params
        .get("task")
        .and_then(Value::as_str)
        .or_else(|| params.get("objective").and_then(Value::as_str))
        .unwrap_or("");
    let hardness = summarize_hardness(task, &params);
    let cost = summarize_token_cost_governance(task, &params, hardness, &server.metrics.snapshot());

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "cost": cost,
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
    let requirement_gate =
        evaluate_requirement_gate_facade(&ledger, &task, &params, "workflow.research")?;
    if requirement_gate.blocked {
        return send_error(
            server,
            request_id,
            -32006,
            requirement_gate
                .reason
                .clone()
                .unwrap_or_else(|| "requirement confirmation is required".to_string()),
            Some(requirement_gate.blocked_payload()),
        )
        .await;
    }

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
            "requirement_gate": {
                "confirmed": true,
                "gate": requirement_gate.success_payload(),
            }
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
    let requirement_gate =
        evaluate_requirement_gate_facade(&ledger, &task, &params, "workflow.consult")?;
    if requirement_gate.blocked {
        return send_error(
            server,
            request_id,
            -32006,
            requirement_gate
                .reason
                .clone()
                .unwrap_or_else(|| "requirement confirmation is required".to_string()),
            Some(requirement_gate.blocked_payload()),
        )
        .await;
    }

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
            "requirement_gate": {
                "confirmed": true,
                "gate": requirement_gate.success_payload(),
            }
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
    let requirement_gate =
        evaluate_requirement_gate_facade(&ledger, task, &params, "workflow.generate")?;
    if requirement_gate.blocked {
        return send_error(
            server,
            request_id,
            -32006,
            requirement_gate
                .reason
                .clone()
                .unwrap_or_else(|| "requirement confirmation is required".to_string()),
            Some(requirement_gate.blocked_payload()),
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
                "gate": requirement_gate.success_payload(),
            }
        }),
    )
    .await
}

/// Handle workflow execute request
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
    let requirement_gate = evaluate_requirement_gate_facade(&ledger, task, &params, "task.plan")?;
    if requirement_gate.blocked {
        return send_error(
            server,
            request_id,
            -32006,
            requirement_gate
                .reason
                .clone()
                .unwrap_or_else(|| "requirement confirmation is required".to_string()),
            Some(requirement_gate.blocked_payload()),
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
                "gate": requirement_gate.success_payload(),
            }
        }),
    )
    .await
}

async fn filter_unavailable_agents(
    server: &AcpServer,
    config: &AppConfig,
    candidates: &mut Vec<(String, Arc<dyn crate::agent::Agent>)>,
) -> Vec<String> {
    let mut unavailable = Vec::new();
    let mut retained = Vec::with_capacity(candidates.len());
    for (name, agent) in std::mem::take(candidates) {
        match probe_agent_runtime_readiness(config, &name, Duration::from_millis(250)).await {
            AgentRuntimeReadiness::Ready => retained.push((name, agent)),
            AgentRuntimeReadiness::EndpointTimedOut => {
                server.metrics.inc_runtime_probe_timeout();
                unavailable.push(name);
            }
            AgentRuntimeReadiness::MissingSecret | AgentRuntimeReadiness::EndpointUnavailable => {
                unavailable.push(name);
            }
        }
    }
    *candidates = retained;
    unavailable
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
        allowed_base_dir: None,
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
            allowed_base_dir: None,
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

    run_with_optional_timeout(
        timeout_seconds.map(|value| Duration::from_secs(value.max(1))),
        collect,
        |duration| {
            anyhow::anyhow!(
                "agent request timed out after {}s",
                duration.as_secs().max(1)
            )
        },
    )
    .await
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
    let guardrail = summarize_learning_guardrail(window, &params)?;
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
                "guardrail": guardrail,
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
            "guardrail": guardrail,
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

#[derive(Debug, Clone, Copy)]
struct LearningGuardrailConfig {
    window: usize,
    min_samples: usize,
    dedup_similarity_threshold: f64,
    high_risk_threshold: f64,
    min_parseable_ratio: f64,
    min_quality_ratio: f64,
    cooldown_seconds: i64,
}

#[derive(Debug, Clone, Copy, Default)]
struct LearningGuardrailStats {
    records_total: usize,
    parseable_records: usize,
    parse_errors: usize,
    evidence_complete: usize,
    attributable: usize,
    high_risk_records: usize,
    high_risk_complete: usize,
    duplicate_records: usize,
    weighted_total: f64,
    weighted_pass: f64,
    last_high_risk_incomplete_at: i64,
}

fn parse_learning_guardrail_config(window: usize, params: &Value) -> LearningGuardrailConfig {
    LearningGuardrailConfig {
        window,
        min_samples: params
            .get("min_samples")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(8)
            .max(1),
        dedup_similarity_threshold: params
            .get("dedup_similarity_threshold")
            .and_then(Value::as_f64)
            .unwrap_or(0.92)
            .clamp(0.75, 0.99),
        high_risk_threshold: params
            .get("high_risk_threshold")
            .and_then(Value::as_f64)
            .unwrap_or(0.7)
            .clamp(0.3, 0.99),
        min_parseable_ratio: params
            .get("min_parseable_ratio")
            .and_then(Value::as_f64)
            .unwrap_or(0.95)
            .clamp(0.5, 1.0),
        min_quality_ratio: params
            .get("min_quality_ratio")
            .and_then(Value::as_f64)
            .unwrap_or(0.75)
            .clamp(0.4, 1.0),
        cooldown_seconds: params
            .get("cooldown_seconds")
            .and_then(Value::as_i64)
            .unwrap_or(300)
            .max(0),
    }
}

fn extract_record_signature(record: &LearningRecord) -> String {
    match record {
        LearningRecord::Workflow(payload) => {
            let task = payload
                .get("task")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let executor = payload
                .get("executor")
                .and_then(Value::as_str)
                .unwrap_or_default();
            format!("{}::{}", task.trim().to_ascii_lowercase(), executor)
        }
        LearningRecord::Pua(payload) => {
            let status = if payload.passed { "pass" } else { "fail" };
            format!(
                "{}::{}::{}",
                payload.stage.trim().to_ascii_lowercase(),
                status,
                payload.escalation_level
            )
        }
    }
}

fn signature_similarity(left: &str, right: &str) -> f64 {
    if left.is_empty() && right.is_empty() {
        return 1.0;
    }

    let lhs = left
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|item| !item.is_empty())
        .collect::<HashSet<_>>();
    let rhs = right
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|item| !item.is_empty())
        .collect::<HashSet<_>>();

    if lhs.is_empty() || rhs.is_empty() {
        return if left == right { 1.0 } else { 0.0 };
    }

    let overlap = lhs.intersection(&rhs).count() as f64;
    overlap / (lhs.len().max(rhs.len()) as f64)
}

fn scan_learning_records_with_parseability(
    window: usize,
) -> Result<(Vec<LearningRecord>, usize, usize)> {
    let storage_dir = Path::new(".goon").join("learning");
    let records_path = storage_dir.join(crate::pua::LEARNING_RECORDS_FILE);
    if !records_path.exists() {
        return Ok((Vec::new(), 0, 0));
    }

    let content = fs::read_to_string(&records_path)?;
    let mut parsed = Vec::new();
    let mut parse_errors = 0usize;
    let mut total_lines = 0usize;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        total_lines = total_lines.saturating_add(1);
        match serde_json::from_str::<LearningRecord>(trimmed) {
            Ok(record) => parsed.push(record),
            Err(_) => parse_errors = parse_errors.saturating_add(1),
        }
    }

    if parsed.len() > window {
        let split_at = parsed.len() - window;
        parsed = parsed.split_off(split_at);
    }

    Ok((parsed, total_lines, parse_errors))
}

fn summarize_learning_guardrail(window: usize, params: &Value) -> Result<Value> {
    let cfg = parse_learning_guardrail_config(window, params);
    let (records, total_lines, parse_errors) = scan_learning_records_with_parseability(cfg.window)?;

    let mut stats = LearningGuardrailStats {
        records_total: records.len(),
        parseable_records: records.len(),
        parse_errors,
        ..LearningGuardrailStats::default()
    };
    let mut signatures: Vec<String> = Vec::new();

    for record in &records {
        let signature = extract_record_signature(record);
        let duplicate = signatures.iter().any(|existing| {
            signature_similarity(existing, &signature) >= cfg.dedup_similarity_threshold
        });
        if duplicate {
            stats.duplicate_records = stats.duplicate_records.saturating_add(1);
        }

        let (evidence_complete, attributable, high_risk, generated_at) = match record {
            LearningRecord::Workflow(payload) => {
                let task_ok = payload
                    .get("task")
                    .and_then(Value::as_str)
                    .map(|item| !item.trim().is_empty())
                    .unwrap_or(false);
                let executor_ok = payload
                    .get("executor")
                    .and_then(Value::as_str)
                    .map(|item| !item.trim().is_empty())
                    .unwrap_or(false);
                let source_ok = payload
                    .get("source")
                    .and_then(Value::as_str)
                    .map(|item| !item.trim().is_empty())
                    .unwrap_or(false);
                let complexity_ok = payload.get("complexity").and_then(Value::as_u64).is_some();
                let totals_ok = payload
                    .get("subtasks_total")
                    .and_then(Value::as_u64)
                    .is_some();
                let risk_score = payload
                    .get("risk_score")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                let failed = payload
                    .get("subtasks_failed")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    > 0;
                let gates_ok = payload
                    .get("gates_ok")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                let generated_at = payload
                    .get("generated_at")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                (
                    task_ok && executor_ok && source_ok && complexity_ok && totals_ok,
                    task_ok && executor_ok,
                    risk_score >= cfg.high_risk_threshold || failed || !gates_ok,
                    generated_at,
                )
            }
            LearningRecord::Pua(payload) => {
                let stage_ok = !payload.stage.trim().is_empty();
                let checks_ok = !payload
                    .missing_checks
                    .iter()
                    .any(|item| item.trim().is_empty());
                (
                    stage_ok && checks_ok,
                    stage_ok,
                    !payload.passed || payload.escalation_level >= 2,
                    0,
                )
            }
        };

        if evidence_complete {
            stats.evidence_complete = stats.evidence_complete.saturating_add(1);
        }
        if attributable {
            stats.attributable = stats.attributable.saturating_add(1);
        }
        if high_risk {
            stats.high_risk_records = stats.high_risk_records.saturating_add(1);
            if evidence_complete && attributable {
                stats.high_risk_complete = stats.high_risk_complete.saturating_add(1);
            }
            if !(evidence_complete && attributable)
                && generated_at > stats.last_high_risk_incomplete_at
            {
                stats.last_high_risk_incomplete_at = generated_at;
            }
        }

        let weight = if high_risk { 2.0 } else { 1.0 };
        stats.weighted_total += weight;
        if evidence_complete && attributable && !duplicate {
            stats.weighted_pass += weight;
        }

        signatures.push(signature);
    }

    let parseable_ratio = if total_lines == 0 {
        1.0
    } else {
        stats.parseable_records as f64 / total_lines as f64
    };
    let quality_ratio = if stats.weighted_total <= f64::EPSILON {
        1.0
    } else {
        stats.weighted_pass / stats.weighted_total
    };
    let high_risk_coverage = if stats.high_risk_records == 0 {
        1.0
    } else {
        stats.high_risk_complete as f64 / stats.high_risk_records as f64
    };
    let dedup_ratio = if stats.records_total == 0 {
        0.0
    } else {
        stats.duplicate_records as f64 / stats.records_total as f64
    };

    let sample_ready = stats.records_total >= cfg.min_samples;
    let now_ts = crate::acp::prelude::now_ts();
    let cooldown_active = stats.last_high_risk_incomplete_at > 0
        && (now_ts - stats.last_high_risk_incomplete_at) <= cfg.cooldown_seconds;

    let mut warnings = Vec::new();
    if !sample_ready {
        warnings.push(format!(
            "learning sample volume below threshold: {}/{}",
            stats.records_total, cfg.min_samples
        ));
    }
    if parseable_ratio < cfg.min_parseable_ratio {
        warnings.push(format!(
            "learning parseability below threshold: {:.2}% < {:.2}%",
            parseable_ratio * 100.0,
            cfg.min_parseable_ratio * 100.0
        ));
    }
    if quality_ratio < cfg.min_quality_ratio {
        warnings.push(format!(
            "learning quality gate below threshold: {:.2}% < {:.2}%",
            quality_ratio * 100.0,
            cfg.min_quality_ratio * 100.0
        ));
    }
    if cooldown_active {
        warnings.push(
            "learning cooldown active due to recent high-risk incomplete evidence".to_string(),
        );
    }

    let status = if !warnings.is_empty() {
        if !sample_ready
            || parseable_ratio < cfg.min_parseable_ratio
            || quality_ratio < cfg.min_quality_ratio
        {
            "block"
        } else {
            "warn"
        }
    } else {
        "pass"
    };

    Ok(json!({
        "status": status,
        "window": cfg.window,
        "sample_ready": sample_ready,
        "cooldown_active": cooldown_active,
        "thresholds": {
            "min_samples": cfg.min_samples,
            "dedup_similarity_threshold": cfg.dedup_similarity_threshold,
            "high_risk_threshold": cfg.high_risk_threshold,
            "min_parseable_ratio": cfg.min_parseable_ratio,
            "min_quality_ratio": cfg.min_quality_ratio,
            "cooldown_seconds": cfg.cooldown_seconds,
        },
        "stats": {
            "records_total": stats.records_total,
            "parseable_records": stats.parseable_records,
            "parse_errors": stats.parse_errors,
            "evidence_complete": stats.evidence_complete,
            "attributable": stats.attributable,
            "high_risk_records": stats.high_risk_records,
            "high_risk_complete": stats.high_risk_complete,
            "duplicate_records": stats.duplicate_records,
            "parseable_ratio": parseable_ratio,
            "quality_ratio": quality_ratio,
            "high_risk_coverage": high_risk_coverage,
            "dedup_ratio": dedup_ratio,
        },
        "warnings": warnings,
    }))
}

async fn handle_learning_guardrail(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let window = params
        .get("limit")
        .or_else(|| params.get("window"))
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(50)
        .max(1);
    let guardrail = summarize_learning_guardrail(window, &params)?;

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "guardrail": guardrail,
        }),
    )
    .await
}

async fn handle_learning_replay(
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

    let storage_dir = Path::new(".goon").join("learning");
    let records = load_learning_records(&storage_dir, window).unwrap_or_default();
    let workflow_count = records
        .iter()
        .filter(|record| matches!(record, LearningRecord::Workflow(_)))
        .count();
    let pua_count = records
        .iter()
        .filter(|record| matches!(record, LearningRecord::Pua(_)))
        .count();
    let learning_bus = read_latest_artifact::<WorkflowLearningBusArtifact>(
        &ledger,
        "spec",
        "latest-learning.json",
    );

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "replay": {
                "source": storage_dir.display().to_string(),
                "window": window,
                "records_total": records.len(),
                "workflow_records": workflow_count,
                "pua_records": pua_count,
                "records": records,
                "learning_bus": learning_bus.as_ref().map(|bus| json!({
                    "generated_at": bus.generated_at,
                    "total_events": bus.total_events,
                    "sampled_events": bus.events.len().min(window),
                    "recent": bus.events.iter().rev().take(window).cloned().collect::<Vec<_>>()
                })).unwrap_or_else(|| json!({
                    "generated_at": 0,
                    "total_events": 0,
                    "sampled_events": 0,
                    "recent": []
                }))
            }
        }),
    )
    .await
}

const KNOWLEDGE_TOMBSTONE_FILE: &str = "tombstones.ndjson";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct KnowledgeTombstoneEntry {
    timestamp: i64,
    key: String,
    reason: String,
    replaced_by: Option<String>,
    superseded: Value,
}

fn knowledge_storage_dir() -> PathBuf {
    Path::new(".goon").join("knowledge")
}

fn knowledge_tombstone_path() -> PathBuf {
    knowledge_storage_dir().join(KNOWLEDGE_TOMBSTONE_FILE)
}

fn load_knowledge_tombstones(limit: usize) -> Vec<KnowledgeTombstoneEntry> {
    let path = knowledge_tombstone_path();
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };

    let mut items = raw
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            serde_json::from_str::<KnowledgeTombstoneEntry>(trimmed).ok()
        })
        .collect::<Vec<_>>();

    if items.len() > limit {
        let split_at = items.len() - limit;
        items = items.split_off(split_at);
    }
    items
}

fn append_knowledge_tombstones(entries: &[KnowledgeTombstoneEntry]) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }

    let dir = knowledge_storage_dir();
    fs::create_dir_all(&dir)?;
    let path = knowledge_tombstone_path();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;

    for entry in entries {
        let encoded = serde_json::to_string(entry)?;
        file.write_all(encoded.as_bytes())?;
        file.write_all(b"\n")?;
    }

    Ok(())
}

fn detect_knowledge_conflicts(
    events: &[crate::reinforcement::KnowledgeInsightArtifact],
    apply_tombstone: bool,
) -> (Vec<Value>, Vec<KnowledgeTombstoneEntry>) {
    let mut grouped: HashMap<String, Vec<&crate::reinforcement::KnowledgeInsightArtifact>> =
        HashMap::new();

    for event in events {
        let key = format!(
            "{}::{}",
            event.task.trim().to_ascii_lowercase(),
            event.phase.trim().to_ascii_lowercase()
        );
        grouped.entry(key).or_default().push(event);
    }

    let mut conflicts = Vec::new();
    let mut tombstones = Vec::new();

    for (key, mut items) in grouped {
        if items.len() < 2 {
            continue;
        }
        items.sort_by(|left, right| {
            right
                .confidence
                .partial_cmp(&left.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.generated_at.cmp(&left.generated_at))
        });

        let primary = items[0];
        let conflicting = items
            .iter()
            .skip(1)
            .filter(|item| {
                item.agent != primary.agent
                    || item.response_excerpt != primary.response_excerpt
                    || (primary.confidence - item.confidence).abs() > f64::EPSILON
            })
            .copied()
            .collect::<Vec<_>>();

        if conflicting.is_empty() {
            continue;
        }

        conflicts.push(json!({
            "key": key,
            "primary": {
                "agent": primary.agent,
                "confidence": primary.confidence,
                "source": primary.source,
                "generated_at": primary.generated_at,
            },
            "conflicting": conflicting.iter().map(|item| json!({
                "agent": item.agent,
                "confidence": item.confidence,
                "source": item.source,
                "generated_at": item.generated_at,
            })).collect::<Vec<_>>(),
        }));

        if apply_tombstone {
            for item in conflicting {
                tombstones.push(KnowledgeTombstoneEntry {
                    timestamp: crate::acp::prelude::now_ts(),
                    key: key.clone(),
                    reason: "knowledge_conflict_superseded".to_string(),
                    replaced_by: Some(primary.agent.clone()),
                    superseded: json!({
                        "task": item.task,
                        "phase": item.phase,
                        "agent": item.agent,
                        "source": item.source,
                        "confidence": item.confidence,
                        "generated_at": item.generated_at,
                        "response_excerpt": item.response_excerpt,
                    }),
                });
            }
        }
    }

    conflicts.sort_by(|left, right| {
        left.get("key")
            .and_then(Value::as_str)
            .cmp(&right.get("key").and_then(Value::as_str))
    });

    (conflicts, tombstones)
}

async fn handle_knowledge_distill(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let ledger = clone_artifact_ledger(server);
    let window = params
        .get("limit")
        .or_else(|| params.get("window"))
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(20)
        .max(1);
    let strategy_limit = params
        .get("strategy_limit")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(12)
        .clamp(1, 64);
    let tombstone_limit = params
        .get("tombstone_limit")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(20)
        .clamp(1, 200);
    let apply_tombstone = params
        .get("apply_tombstone")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let learning_dir = Path::new(".goon").join("learning");
    let evidence_records = load_learning_records(&learning_dir, window).unwrap_or_default();
    let workflow_records = evidence_records
        .iter()
        .filter(|record| matches!(record, LearningRecord::Workflow(_)))
        .count();
    let pua_records = evidence_records
        .iter()
        .filter(|record| matches!(record, LearningRecord::Pua(_)))
        .count();

    let knowledge_bus =
        read_latest_artifact::<KnowledgeBusArtifact>(&ledger, "spec", "latest-knowledge.json");
    let summary_events = knowledge_bus
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

    let (conflicts, new_tombstones) = detect_knowledge_conflicts(&summary_events, apply_tombstone);
    if apply_tombstone {
        append_knowledge_tombstones(&new_tombstones)?;
    }
    let tombstones = load_knowledge_tombstones(tombstone_limit);

    let mut strategy_rules = Vec::new();
    for event in summary_events.iter().take(strategy_limit) {
        let then_action = event
            .reusable_insights
            .first()
            .cloned()
            .or_else(|| {
                event.verification_steps.first().map(|step| {
                    format!(
                        "Prioritize verification step '{}' for phase '{}'",
                        step, event.phase
                    )
                })
            })
            .unwrap_or_else(|| {
                format!(
                    "Use '{}' insights as baseline strategy for task '{}'",
                    event.agent, event.task
                )
            });

        strategy_rules.push(json!({
            "rule_id": format!("k-rule-{}", strategy_rules.len() + 1),
            "when": {
                "task": event.task,
                "phase": event.phase,
                "agent": event.agent,
            },
            "then": then_action,
            "confidence": event.confidence,
            "source": event.source,
        }));
    }

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "distillation": {
                "window": window,
                "layers": {
                    "evidence": {
                        "source": learning_dir.display().to_string(),
                        "records_total": evidence_records.len(),
                        "workflow_records": workflow_records,
                        "pua_records": pua_records,
                        "records": evidence_records.into_iter().map(|record| serde_json::to_value(record).unwrap_or_else(|_| json!({}))).collect::<Vec<_>>()
                    },
                    "summary": {
                        "source": "spec/latest-knowledge.json",
                        "total_events": knowledge_bus.as_ref().map(|bus| bus.total_events).unwrap_or(0),
                        "sampled_events": summary_events.len(),
                        "latest_generated_at": knowledge_bus.as_ref().map(|bus| bus.generated_at).unwrap_or(0),
                        "recent": summary_events,
                    },
                    "strategy": {
                        "rules_total": strategy_rules.len(),
                        "rules": strategy_rules,
                    },
                    "conflicts": {
                        "count": conflicts.len(),
                        "items": conflicts,
                    },
                    "tombstones": {
                        "added_count": new_tombstones.len(),
                        "stored_count": tombstones.len(),
                        "items": tombstones,
                    }
                }
            }
        }),
    )
    .await
}

#[derive(Debug, Clone, Serialize)]
struct RlOfflineEvalSample {
    timestamp: i64,
    success: bool,
    latency_cost: f64,
    tool_error_rate: f64,
    safety_penalty: f64,
    reward: f64,
}

#[derive(Debug, Clone, Copy)]
struct RlRewardWeights {
    success: f64,
    latency: f64,
    tool_error: f64,
    safety: f64,
}

fn parse_rl_reward_weights(params: &Value) -> RlRewardWeights {
    RlRewardWeights {
        success: params
            .get("success_weight")
            .and_then(Value::as_f64)
            .unwrap_or(0.55)
            .clamp(0.0, 2.0),
        latency: params
            .get("latency_weight")
            .and_then(Value::as_f64)
            .unwrap_or(0.2)
            .clamp(0.0, 2.0),
        tool_error: params
            .get("tool_error_weight")
            .and_then(Value::as_f64)
            .unwrap_or(0.15)
            .clamp(0.0, 2.0),
        safety: params
            .get("safety_weight")
            .and_then(Value::as_f64)
            .unwrap_or(0.1)
            .clamp(0.0, 2.0),
    }
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

fn collect_rl_offline_eval_samples(
    window: usize,
    weights: RlRewardWeights,
) -> Vec<RlOfflineEvalSample> {
    let learning_dir = Path::new(".goon").join("learning");
    let records = load_learning_records(&learning_dir, window).unwrap_or_default();

    records
        .into_iter()
        .filter_map(|record| match record {
            LearningRecord::Workflow(payload) => {
                let subtasks_total = payload
                    .get("subtasks_total")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    .max(1);
                let subtasks_failed = payload
                    .get("subtasks_failed")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .min(subtasks_total);
                let success = subtasks_failed == 0;

                let explicit_duration_ms = payload
                    .get("duration_ms")
                    .or_else(|| payload.get("total_duration_ms"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0)
                    .max(0.0);
                let latency_cost = if explicit_duration_ms <= f64::EPSILON {
                    0.0
                } else {
                    (explicit_duration_ms / 5000.0).clamp(0.0, 1.0)
                };

                let tool_error_rate =
                    (subtasks_failed as f64 / subtasks_total as f64).clamp(0.0, 1.0);
                let gates_ok = payload
                    .get("gates_ok")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                let safety_penalty = if gates_ok { 0.0 } else { 1.0 };

                let reward = (weights.success * if success { 1.0 } else { 0.0 }
                    - weights.latency * latency_cost
                    - weights.tool_error * tool_error_rate
                    - weights.safety * safety_penalty)
                    .clamp(-1.0, 1.0);

                Some(RlOfflineEvalSample {
                    timestamp: payload
                        .get("generated_at")
                        .and_then(Value::as_i64)
                        .unwrap_or(0),
                    success,
                    latency_cost,
                    tool_error_rate,
                    safety_penalty,
                    reward,
                })
            }
            LearningRecord::Pua(_) => None,
        })
        .collect()
}

fn build_rl_alignment_offline_eval_payload(params: &Value) -> Value {
    let window = params
        .get("limit")
        .or_else(|| params.get("window"))
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(120)
        .clamp(20, 2000);
    let pass_threshold = params
        .get("pass_threshold")
        .and_then(Value::as_f64)
        .unwrap_or(0.05)
        .clamp(0.0, 0.5);
    let drift_threshold = params
        .get("drift_threshold")
        .and_then(Value::as_f64)
        .unwrap_or(0.12)
        .clamp(0.01, 0.6);

    let weights = parse_rl_reward_weights(params);
    let mut samples = collect_rl_offline_eval_samples(window, weights);
    samples.sort_by_key(|sample| sample.timestamp);

    let (baseline_slice, candidate_slice) = if samples.len() < 2 {
        (&samples[..], &samples[..])
    } else {
        let split_index = ((samples.len() as f64) * 0.7).floor() as usize;
        let split_index = split_index.clamp(1, samples.len() - 1);
        samples.split_at(split_index)
    };

    let baseline_rewards = baseline_slice
        .iter()
        .map(|item| item.reward)
        .collect::<Vec<_>>();
    let candidate_rewards = candidate_slice
        .iter()
        .map(|item| item.reward)
        .collect::<Vec<_>>();
    let baseline_mean = mean(&baseline_rewards);
    let candidate_mean = mean(&candidate_rewards);
    let improvement = candidate_mean - baseline_mean;

    let baseline_safety = mean(
        &baseline_slice
            .iter()
            .map(|item| item.safety_penalty)
            .collect::<Vec<_>>(),
    );
    let candidate_safety = mean(
        &candidate_slice
            .iter()
            .map(|item| item.safety_penalty)
            .collect::<Vec<_>>(),
    );

    let recent_window = samples.len().clamp(1, 20);
    let recent_rewards = samples
        .iter()
        .rev()
        .take(recent_window)
        .map(|item| item.reward)
        .collect::<Vec<_>>();
    let historical_rewards = if samples.len() > recent_window {
        samples
            .iter()
            .take(samples.len() - recent_window)
            .map(|item| item.reward)
            .collect::<Vec<_>>()
    } else {
        baseline_rewards.clone()
    };
    let recent_mean = mean(&recent_rewards);
    let historical_mean = mean(&historical_rewards);
    let reward_drift = (recent_mean - historical_mean).abs();
    let drift_alert = reward_drift > drift_threshold;

    let enough_samples = samples.len() >= 20;
    let safe_to_promote = candidate_safety <= (baseline_safety + 0.05);
    let pass = enough_samples && improvement >= pass_threshold && safe_to_promote;
    let recommended_mode = if pass && !drift_alert {
        "adaptive"
    } else {
        "conservative"
    };

    let warnings = {
        let mut items = Vec::new();
        if !enough_samples {
            items.push(format!(
                "offline replay sample size below threshold: {} < 20",
                samples.len()
            ));
        }
        if improvement < pass_threshold {
            items.push(format!(
                "candidate reward uplift below threshold: {:.4} < {:.4}",
                improvement, pass_threshold
            ));
        }
        if !safe_to_promote {
            items.push(format!(
                "candidate safety penalty regressed: {:.4} > {:.4}",
                candidate_safety,
                baseline_safety + 0.05
            ));
        }
        if drift_alert {
            items.push(format!(
                "reward drift exceeds threshold: {:.4} > {:.4}",
                reward_drift, drift_threshold
            ));
        }
        items
    };

    json!({
        "ok": true,
        "offline_eval": {
            "window": window,
            "samples_total": samples.len(),
            "weights": {
                "success": weights.success,
                "latency": weights.latency,
                "tool_error": weights.tool_error,
                "safety": weights.safety,
            },
            "baseline": {
                "samples": baseline_slice.len(),
                "mean_reward": baseline_mean,
                "mean_safety_penalty": baseline_safety,
            },
            "candidate": {
                "samples": candidate_slice.len(),
                "mean_reward": candidate_mean,
                "mean_safety_penalty": candidate_safety,
            },
            "comparison": {
                "reward_uplift": improvement,
                "pass_threshold": pass_threshold,
                "passes": pass,
            },
            "drift": {
                "recent_mean": recent_mean,
                "historical_mean": historical_mean,
                "absolute_diff": reward_drift,
                "threshold": drift_threshold,
                "alert": drift_alert,
            },
            "decision": {
                "recommended_mode": recommended_mode,
                "fallback_triggered": !pass || drift_alert,
            },
            "warnings": warnings,
        }
    })
}

async fn handle_rl_alignment_offline_eval(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(
        server,
        request_id,
        build_rl_alignment_offline_eval_payload(&params),
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
    let metrics = server.metrics.snapshot();
    json!({
        "sampling_rate": sampling_rate,
        "buffered_events": events.len(),
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
        metacognitive_loop: None,
        messages,
    };
    state.branch_heads.insert(branch_key, checkpoint_id);
    state.last_touched_at = crate::acp::prelude::now_ts();
    state.checkpoints.push(checkpoint.clone());
    enforce_checkpoint_capacity(&mut state, 0, Some(&checkpoint.checkpoint_id));
    checkpoint
}

pub(crate) async fn persist_checkpoint_metacognitive_loop(
    server: &AcpServer,
    conversation_id: &str,
    branch_id: &str,
    checkpoint_id: &str,
    mut metacognitive_loop: Value,
) -> Value {
    let mut state = server.conversation_state.lock().await;
    let cycle_count = state
        .checkpoints
        .iter()
        .filter(|checkpoint| {
            checkpoint.conversation_id == conversation_id && checkpoint.branch_id == branch_id
        })
        .count() as u64;

    if let Some(obj) = metacognitive_loop.as_object_mut() {
        obj.insert("cycle_count".to_string(), json!(cycle_count.max(1)));
        obj.insert(
            "conversation_id".to_string(),
            Value::String(conversation_id.to_string()),
        );
        obj.insert(
            "branch_id".to_string(),
            Value::String(branch_id.to_string()),
        );
        obj.insert(
            "checkpoint_id".to_string(),
            Value::String(checkpoint_id.to_string()),
        );
    }

    if let Some(checkpoint) = state
        .checkpoints
        .iter_mut()
        .find(|checkpoint| checkpoint.checkpoint_id == checkpoint_id)
    {
        checkpoint.metacognitive_loop = Some(metacognitive_loop.clone());
    }

    metacognitive_loop
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
    checkpoints.sort_by_key(|checkpoint| std::cmp::Reverse(checkpoint.created_at));
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
    checkpoints.sort_by_key(|checkpoint| std::cmp::Reverse(checkpoint.created_at));
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
    let error_data =
        inject_platform_profiles_if_absent(data.unwrap_or_else(|| json!({})), "acp.error");
    let data = Some(error_data);
    let data = match take_pua_report(id.as_ref()) {
        Some(encoded) => Some(inject_pua_report_into_error_data(data, encoded)),
        None => data,
    };
    let data = with_error_contract_data(code, &message, data);
    crate::acp::r#impl::io::send_error(server, id, code, message, data).await
}

/// Send result response
async fn send_result(server: &AcpServer, id: Option<Value>, result: Value) -> Result<()> {
    let method = DISPATCH_REQUEST_METHOD
        .try_with(|m| m.clone())
        .unwrap_or_default();
    let result = inject_platform_profiles_if_absent(result, &method);
    let result = match take_pua_report(id.as_ref()) {
        Some(encoded) => inject_pua_report_into_result(result, encoded),
        None => result,
    };
    crate::acp::r#impl::io::send_result(server, id, result).await
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    #[cfg(not(feature = "backend-postgres"))]
    use super::collect_vector_context_snippets;
    use super::{
        classify_request_error_kind, infer_workflow_parallelism, rebalance_execution_order,
        session_id_for_task, summarize_lock_health, with_error_contract_data,
    };
    use crate::acp::prelude::AcpLockSnapshot;
    #[cfg(not(feature = "backend-postgres"))]
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

    #[cfg(not(feature = "backend-postgres"))]
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

    #[test]
    fn classify_request_error_kind_detects_pua_violation() {
        let error = anyhow::anyhow!("PUA red line violation: blocked action");
        assert_eq!(classify_request_error_kind(&error), "PuaViolation");
    }

    #[test]
    fn classify_request_error_kind_detects_budget_exceeded() {
        let error = anyhow::anyhow!("budget denied tool 'x' in scope 'y': budget exceeded");
        assert_eq!(classify_request_error_kind(&error), "BudgetExceeded");
    }

    #[test]
    fn classify_request_error_kind_detects_sandbox_blocked() {
        let error = anyhow::anyhow!("hardening policy denied tool 'shell': sandbox strict");
        assert_eq!(classify_request_error_kind(&error), "SandboxBlocked");
    }

    #[test]
    fn with_error_contract_data_infers_retryable_rate_limit() {
        let data = with_error_contract_data(-32029, "rate limited", None)
            .expect("error contract data should be present");
        assert_eq!(data["kind"], Value::String("RateLimited".to_string()));
        assert_eq!(data["retry"]["retryable"], Value::Bool(true));
        assert_eq!(data["retry"]["max_retries"], Value::Number(3.into()));
    }

    #[test]
    fn with_error_contract_data_preserves_explicit_kind_and_detail() {
        let data = with_error_contract_data(
            -32603,
            "generic failure",
            Some(json!({"kind": "PuaViolation", "detail": "acp.handle_request.dispatch"})),
        )
        .expect("error contract data should be present");
        assert_eq!(data["kind"], Value::String("PuaViolation".to_string()));
        assert_eq!(
            data["detail"],
            Value::String("acp.handle_request.dispatch".to_string())
        );
        assert_eq!(data["retry"]["retryable"], Value::Bool(false));
    }

    #[test]
    fn summarize_lock_health_marks_poisoned_components_warn() {
        let summary = summarize_lock_health(&[
            AcpLockSnapshot {
                name: "phase_rate_limiter".to_string(),
                acquisitions: 4,
                poisoned_total: 1,
                recovered_total: 1,
                slow_wait_total: 0,
                avg_wait_ms: 0.4,
                max_wait_ms: 1.2,
            },
            AcpLockSnapshot {
                name: "lifecycle_state".to_string(),
                acquisitions: 8,
                poisoned_total: 0,
                recovered_total: 0,
                slow_wait_total: 0,
                avg_wait_ms: 0.2,
                max_wait_ms: 0.5,
            },
        ]);

        assert_eq!(summary.status, "warn");
        assert_eq!(summary.poisoned_total, 1);
        assert_eq!(summary.recovered_total, 1);
        assert_eq!(summary.components_tracked, 2);
    }
}
