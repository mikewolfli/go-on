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
use tokio::task::JoinSet;
use tokio::time::Duration;
use tracing::{debug, info};

// Task-local: carries the current dispatch method through send_result for universal profile injection
tokio::task_local! {
    static DISPATCH_REQUEST_METHOD: String;
}

use crate::acp::background::run_maintenance_cycle;

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
pub(crate) use self::checkpoint_pack::create_checkpoint_record;
pub(crate) use self::checkpoint_pack::persist_checkpoint_metacognitive_loop;
use self::governance_pack::*;
use self::hardness_pack::*;
use self::lifecycle_pack::*;
use self::ops_pack::*;
pub use self::protocol_pack::record_tool_call_audit_with_protocol;
use self::pua_pack::*;
use self::learning_pack::*;
use self::runtime_pack::*;
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
    server.observability.metrics.inc_active_requests();
    let trace = new_request_trace(server, &request);
    let _request_span = if let Ok(telemetry_guard) = server.observability.telemetry_runtime.lock() {
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
        .observability
        .metrics
        .record_request_outcome(success, duration_ms as f64);
    server.observability.metrics.dec_active_requests();

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
    let snapshot = serde_json::to_value(server.observability.metrics.snapshot())?;
    // Keep flat fields for backward compat AND add wrapper keys for new consumers
    let mut result = snapshot.clone();
    if let Value::Object(ref mut map) = result {
        map.insert("ok".to_string(), json!(true));
        map.insert("metrics".to_string(), snapshot);
    }
    send_result(server, request_id, result).await
}

async fn handle_metrics_prometheus(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    let metrics = server.observability.metrics.snapshot();
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
    server.observability.metrics.reset_all();
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
                "total": server.observability.metrics.snapshot().review_gate_total,
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
    let metrics = server.observability.metrics.snapshot();
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
    let metrics = server.observability.metrics.snapshot();

    let config_path = server.config_path.as_deref().map(Path::new);
    let report = build_runtime_healthcheck_report(
        config_path,
        server.cache.response_cache.as_deref(),
        server.cache.vector_store.as_deref(),
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
        server.observability.lock_monitor.as_ref(),
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

    let lock_components = server.observability.lock_monitor.snapshot();
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
    let _metrics = server.observability.metrics.snapshot();
    let config_path = server.config_path.as_deref().map(Path::new);
    let report = build_runtime_healthcheck_report(
        config_path,
        server.cache.response_cache.as_deref(),
        server.cache.vector_store.as_deref(),
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
        server.cache.response_cache.as_deref(),
        server.cache.vector_store.as_deref(),
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
    let runtime_snapshot = server.observability.metrics.snapshot();

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
        server.observability.lock_monitor.as_ref(),
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
