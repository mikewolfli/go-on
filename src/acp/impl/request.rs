use crate::protocol::access_mode::{request_dispatch_mode, RequestDispatchMode};

/// Read protocol mode from config.toml / runtime_config.
fn get_protocol_mode(server: &AcpServer) -> RequestDispatchMode {
    // Try reading protocol_mode from runtime_config.
    request_dispatch_mode(server.runtime_config.protocol_mode.as_deref())
}

/// Returns true if the method belongs to the MCP protocol.
/// Standard MCP methods (initialize, tools/list, tools/call, etc.)
/// may be sent without the "mcp." prefix in MCP-only mode.
fn is_mcp_request(method: &str) -> bool {
    method.starts_with("mcp.")
        || method == "mcp.initialize"
        || method == "initialize"
        || method == "notifications/initialized"
        || method.starts_with("tools/")
        || method.starts_with("resources/")
        || method.starts_with("prompts/")
        || method.starts_with("logging/")
        || method.starts_with("sampling/")
        || method.starts_with("completion/")
        || method == "ping"
}

/// Convert a standard MCP method name to its "mcp." prefixed form
/// if it isn't already prefixed. Used in Mcp dispatch mode so that
/// standard MCP clients (which send `initialize`, `tools/list`, etc.)
/// are routed to the ACP dispatch's `mcp.*` handler.
fn normalize_mcp_method(method: &str) -> String {
    if method.starts_with("mcp.") {
        return method.to_string();
    }
    match method {
        "initialize" => "mcp.initialize".to_string(),
        "notifications/initialized" | "notifications_initialized" => {
            "mcp.notifications_initialized".to_string()
        }
        "ping" => "mcp.ping".to_string(),
        _ if method.starts_with("tools/") => format!("mcp.tools.{}", &method[6..]),
        _ if method.starts_with("resources/") => format!("mcp.resources.{}", &method[10..]),
        _ if method.starts_with("prompts/") => format!("mcp.prompts.{}", &method[8..]),
        _ if method.starts_with("logging/") => format!("mcp.logging.{}", &method[8..]),
        _ if method.starts_with("sampling/") => format!("mcp.sampling.{}", &method[9..]),
        _ if method.starts_with("completion/") => format!("mcp.completion.{}", &method[11..]),
        _ => method.to_string(),
    }
}

/// Returns true if the method belongs to the ACP/A2A protocol.
fn is_acp_request(method: &str) -> bool {
    // Common ACP/A2A JSON-RPC methods.
    matches!(
        method,
        "initialize"
            // Standard ACP lifecycle methods
            | "authenticate"
            | "logout"
            // Standard ACP session lifecycle methods
            | "session/new"
            | "session/load"
            | "session/prompt"
            | "session/cancel"
            | "session/list"
            | "session/set_mode"
            | "session/set_config_option"
            // Protocol-level notifications
            | "$/cancel_request"
            // Go-On custom methods (backward compat)
            | "chat"
            | "phase"
            | "phase.status"
            | "metrics.get"
            | "metrics"
            | "metrics.prometheus"
            | "metrics.window.query"
            | "metrics.errors.summary"
            | "shutdown"
            | "health"
            | "runtime.health"
            | "health.probes"
            | "lock.status"
            | "runtime.self_model"
            | "provider.status"
            | "release.readiness"
            | "runtime.stability"
            | "runtime.features"
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
            | "checkpoint.list"
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
            | "workflow.ask"
            | "workflow.consult"
            | "workflow.generate"
            | "workflow.generate_from_chat"
            | "workflow.execute"
            | "workflow.run.list"
            | "workflow.run.get"
            | "workflow.run.cancel"
            | "workflow.run.pause"
            | "workflow.run.resume"
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
                | "skill.list"
                | "skill.remove"
                | "skill.create"
                | "skill.update"
                | "skill.version.list"
                | "skill.version.rollback"
                | "provider.test_connection"
                | "provider.test_completion"
                | "provider.capabilities"
                | "provider.copilot_device_code"
                | "provider.copilot_device_code_poll"
                | "provider.catalog"
                | "runtime.restart"
                | "phase.policy.replay"
            | "primary_secondary.summary"
            | "summary/primary_secondary"
            | "governance.status"
            | "governance.remediate"
            | "governance.config.save"
            | "capabilities.list"
            | "health.check"
             // diagnostics / ops also used by vscode-addon in ACP mode
             | "metrics.reset"
             | "trace.get"
             | "trace.metrics"
             | "debug_panel.get"
             | "debug.panel.get"
            // MCP-bridge methods that ACP stdio also dispatches
            | "mcp.tools.list"
            | "mcp.tools.call"
            | "models.list"
            | "models/list"
            | "provider.configure"
            | "provider.list_models"
            // Prompt template methods
            | "prompts.list"
            | "prompts.search"
            | "prompts.get"
            | "prompts.create"
            | "prompts.update"
            | "prompts.delete"
    )
}
// Request handling implementation functions for ACP server
//
// This module contains standalone functions that implement request handling
// functionality previously in the `impl AcpServer` block in `impl/request.rs`.
// These functions take `AcpServer` as the first parameter to maintain
// compatibility with the original implementation.

use std::borrow::Cow;
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

use crate::acp::background::{run_health_check, run_maintenance_cycle};

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
    load_learning_records, DynamicQualityCompass, LearningRecord, PuaEnforcementPlan,
    PuaExecutionReport, PuaFeedbackCollector, PuaRuleEngine, PuaStageRequirement, TaskContext,
    TaskType,
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
pub(crate) mod exec_pack;
mod governance_pack;
mod hardness_pack;
mod learning_pack;
mod lifecycle_pack;
mod ops_pack;
pub mod prompts_pack;
mod protocol_pack;
mod pua_pack;
mod repro_pack;
mod runtime_pack;
pub(crate) mod tools_pack;
mod trace_pack;
pub(crate) mod workflow_pack;
use self::chat_pack::{parse_messages, send_error, send_result};
pub(crate) use self::checkpoint_pack::create_checkpoint_record;
pub(crate) use self::checkpoint_pack::persist_checkpoint_metacognitive_loop;
use self::checkpoint_pack::*;
use self::config_pack::*;
use self::exec_pack::*;
pub use self::governance_pack::build_knowledge_refinement_profile;
pub use self::governance_pack::build_learning_profile;
pub(crate) use self::governance_pack::inject_platform_profiles_if_absent;
use self::governance_pack::*;
use self::hardness_pack::*;
use self::learning_pack::*;
use self::lifecycle_pack::*;
use self::ops_pack::*;
pub use self::protocol_pack::record_tool_call_audit_with_protocol;
use self::protocol_pack::*;
use self::pua_pack::*;
use self::runtime_pack::*;
use self::tools_pack::*;
use self::trace_pack::*;

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
    let request_method = request.method.clone();
    let mut method: Cow<'_, str> = Cow::Borrowed(&request_method);
    match protocol_mode {
        RequestDispatchMode::Acp => {
            if !is_acp_request(method.as_ref()) {
                return send_error(
                    server,
                    request.id,
                    -32601,
                    tf("error.acp_mode_unsupported", &[("method", method.as_ref())]),
                    None,
                )
                .await;
            }
        }
        RequestDispatchMode::Mcp => {
            if !is_mcp_request(method.as_ref()) {
                return send_error(
                    server,
                    request.id,
                    -32601,
                    tf("error.mcp_mode_unsupported", &[("method", method.as_ref())]),
                    None,
                )
                .await;
            }
            // Normalize standard MCP method names (e.g. `tools/list` -> `mcp.tools.list`)
            // so the dispatch switch below can route them to the correct handler.
            method = Cow::Owned(normalize_mcp_method(method.as_ref()));
        }
        RequestDispatchMode::Auto => {
            // If MCP method, prefer MCP branch; otherwise fall through to ACP.
            // Mixed-protocol requests are allowed in Auto mode.
        }
    }

    let pua_engine = PuaRuleEngine::new(server.pua_enforcement_plan.clone());
    let task_type = infer_task_type(method.as_ref(), &request.params);
    let task_context = TaskContext {
        task_type: task_type.clone(),
        file_count: infer_file_count(&request.params),
        risk_score: infer_risk_score(method.as_ref(), &task_type),
    };
    let dynamic_compass = DynamicQualityCompass::default();
    let dynamic_checks = dynamic_compass.get_checks(&task_context);
    let dynamic_check_descriptions = dynamic_checks
        .iter()
        .map(|check| check.description.clone())
        .collect::<Vec<_>>();

    if let Err(violation) = pua_engine.check_red_lines(method.as_ref()) {
        return send_error(
            server,
            request.id,
            -32003,
            format!("PUA red line violation: {}", violation.detail),
            Some(json!({
                "type": "pua_violation",
                "kind": format!("{:?}", violation.kind),
                "method": method.as_ref(),
                "detail": violation.detail,
                "quality_compass": dynamic_check_descriptions,
            })),
        )
        .await;
    }
    if let Some(stage) = infer_pua_stage(method.as_ref()) {
        let completed_actions = extract_pua_completed_actions(&request.params, method.as_ref());
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
                        "method": method.as_ref(),
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
    // Use the potentially normalized method for dispatch.
    let result = DISPATCH_REQUEST_METHOD
        .scope(method.to_string(), async {
            match method.as_ref() {
                "initialize" => protocol_pack::handle_initialize(server, request_id).await,
                // Standard ACP session lifecycle methods
                "session/new" => {
                    protocol_pack::handle_session_new(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "session/load" => {
                    protocol_pack::handle_session_load(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "session/prompt" => {
                    protocol_pack::handle_session_prompt(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "session/cancel" => {
                    protocol_pack::handle_session_cancel(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "session/list" => {
                    protocol_pack::handle_session_list(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "session/set_mode" => {
                    protocol_pack::handle_session_set_mode(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "session/set_config_option" => {
                    protocol_pack::handle_session_set_config_option(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                // Standard ACP authentication methods
                "authenticate" => {
                    protocol_pack::handle_authenticate(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "logout" => {
                    protocol_pack::handle_logout(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                // Protocol-level notifications
                "$/cancel_request" => {
                    protocol_pack::handle_cancel_request(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                // MCP methods bridged through ACP dispatch
                "mcp.initialize" => protocol_pack::handle_mcp_initialize(server, request_id).await,
                "mcp.notifications_initialized" => {
                    // MCP notification — no response expected per JSON-RPC spec
                    Ok(())
                }
                "mcp.ping" => protocol_pack::handle_mcp_ping(server, request_id).await,
                "mcp.tools.list" => protocol_pack::handle_mcp_tools_list(server, request_id).await,
                "mcp.tools.call" => {
                    protocol_pack::handle_mcp_tools_call(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "mcp.resources.list" => {
                    protocol_pack::handle_mcp_resources_list(server, request_id).await
                }
                "mcp.resources.read" => {
                    protocol_pack::handle_mcp_resources_read(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "mcp.resources.subscribe" => {
                    protocol_pack::handle_mcp_resources_subscribe(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "mcp.logging.setLevel" => {
                    protocol_pack::handle_mcp_logging_set_level(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "mcp.completion.complete" => {
                    protocol_pack::handle_mcp_completion_complete(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "mcp.sampling.createMessage" => {
                    protocol_pack::handle_mcp_sampling_create_message(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "prompts.list" => {
                    let lang = request
                        .params
                        .as_ref()
                        .and_then(|p| p.get("lang"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(&server.runtime_config.i18n_default_language);
                    match prompts_pack::handle_prompts_list(&server.prompt_manager, lang) {
                        Ok(v) => send_result(server, request_id, v).await,
                        Err(e) => {
                            send_error(
                                server,
                                request_id,
                                -32603,
                                format!("prompts.list failed: {}", e),
                                None,
                            )
                            .await
                        }
                    }
                }
                "prompts.search" => {
                    let lang = request
                        .params
                        .as_ref()
                        .and_then(|p| p.get("lang"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(&server.runtime_config.i18n_default_language);
                    match prompts_pack::handle_prompts_search(
                        &server.prompt_manager,
                        lang,
                        request
                            .params
                            .as_ref()
                            .and_then(|p| p.get("query"))
                            .and_then(|v| v.as_str())
                            .unwrap_or(""),
                    ) {
                        Ok(v) => send_result(server, request_id, v).await,
                        Err(e) => {
                            send_error(
                                server,
                                request_id,
                                -32603,
                                format!("prompts.search failed: {}", e),
                                None,
                            )
                            .await
                        }
                    }
                }
                "prompts.get" => {
                    let lang = request
                        .params
                        .as_ref()
                        .and_then(|p| p.get("lang"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(&server.runtime_config.i18n_default_language);
                    let id = request
                        .params
                        .as_ref()
                        .and_then(|p| p.get("id"))
                        .and_then(|v| v.as_str());
                    match id {
                        Some(id) => {
                            match prompts_pack::handle_prompts_get(&server.prompt_manager, lang, id)
                            {
                                Ok(v) => send_result(server, request_id, v).await,
                                Err(e) => {
                                    send_error(server, request_id, -32602, format!("{}", e), None)
                                        .await
                                }
                            }
                        }
                        None => {
                            send_error(
                                server,
                                request_id,
                                -32602,
                                "missing required field: id".to_string(),
                                None,
                            )
                            .await
                        }
                    }
                }
                "prompts.create" => {
                    let lang = request
                        .params
                        .as_ref()
                        .and_then(|p| p.get("lang"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(&server.runtime_config.i18n_default_language);
                    let params = request.params.clone().unwrap_or_default();
                    match prompts_pack::handle_prompts_create(&server.prompt_manager, lang, &params)
                    {
                        Ok(v) => send_result(server, request_id, v).await,
                        Err(e) => {
                            send_error(
                                server,
                                request_id,
                                -32603,
                                format!("prompts.create failed: {}", e),
                                None,
                            )
                            .await
                        }
                    }
                }
                "prompts.update" => {
                    let lang = request
                        .params
                        .as_ref()
                        .and_then(|p| p.get("lang"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(&server.runtime_config.i18n_default_language);
                    let params = request.params.clone().unwrap_or_default();
                    match prompts_pack::handle_prompts_update(&server.prompt_manager, lang, &params)
                    {
                        Ok(v) => send_result(server, request_id, v).await,
                        Err(e) => {
                            send_error(
                                server,
                                request_id,
                                -32603,
                                format!("prompts.update failed: {}", e),
                                None,
                            )
                            .await
                        }
                    }
                }
                "prompts.delete" => {
                    let lang = request
                        .params
                        .as_ref()
                        .and_then(|p| p.get("lang"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(&server.runtime_config.i18n_default_language);
                    let params = request.params.clone().unwrap_or_default();
                    match prompts_pack::handle_prompts_delete(&server.prompt_manager, lang, &params)
                    {
                        Ok(v) => send_result(server, request_id, v).await,
                        Err(e) => {
                            send_error(
                                server,
                                request_id,
                                -32603,
                                format!("prompts.delete failed: {}", e),
                                None,
                            )
                            .await
                        }
                    }
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
                "skill.list_imported" | "skill.list" => {
                    protocol_pack::handle_skill_list_imported(server, request_id).await
                }
                "skill.create" => {
                    protocol_pack::handle_skill_create(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "skill.update" => {
                    protocol_pack::handle_skill_update(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "skill.version.list" => {
                    protocol_pack::handle_skill_version_list(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "skill.version.rollback" => {
                    protocol_pack::handle_skill_version_rollback(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
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
                "metrics.window.query" => {
                    runtime_pack::handle_metrics_window_query(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "metrics.errors.summary" => {
                    runtime_pack::handle_metrics_errors_summary(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
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
                "runtime.features" => {
                    runtime_pack::handle_runtime_features(server, request_id).await
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
                "conversation.checkpoint.list" | "checkpoint.list" => {
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
                "workflow.ask" => {
                    workflow_pack::handle_workflow_ask(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                        &trace,
                    )
                    .await
                }
                "workflow.generate_from_chat" => {
                    workflow_pack::handle_workflow_generate_from_chat(
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
                "workflow.run.list" => {
                    handle_workflow_run_list(server, request.params.unwrap_or_default(), request_id)
                        .await
                }
                "workflow.run.get" => {
                    handle_workflow_run_get(server, request.params.unwrap_or_default(), request_id)
                        .await
                }
                "workflow.run.cancel" => {
                    handle_workflow_run_cancel(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "workflow.run.pause" => {
                    handle_workflow_run_pause(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "workflow.run.resume" => {
                    handle_workflow_run_resume(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
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
                "capabilities.list" => {
                    runtime_pack::handle_capabilities_list(server, request_id).await
                }
                "models.list" | "models/list" => {
                    protocol_pack::handle_models_list(
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
                "primary_secondary.summary" | "summary/primary_secondary" => {
                    learning_pack::handle_primary_secondary_summary(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "health.check" => {
                    if let Err(e) = run_health_check(server).await {
                        tracing::warn!("health.check: run_health_check failed: {}", e);
                    }
                    send_result(server, request_id, json!({ "ok": true })).await
                }
                "governance.remediate" => {
                    runtime_pack::handle_governance_remediate(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "governance.config.save" => {
                    runtime_pack::handle_governance_config_save(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "provider.configure" => {
                    runtime_pack::handle_provider_configure(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "provider.test_connection" => {
                    runtime_pack::handle_provider_test_connection(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "provider.test_completion" => {
                    runtime_pack::handle_provider_test_completion(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "provider.copilot_device_code" => {
                    runtime_pack::handle_copilot_device_code_request(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "provider.copilot_device_code_poll" => {
                    runtime_pack::handle_copilot_device_code_poll(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "provider.capabilities" => {
                    runtime_pack::handle_provider_capabilities(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "provider.catalog" => {
                    runtime_pack::handle_provider_catalog(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "provider.list_models" => {
                    runtime_pack::handle_provider_list_models(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "runtime.restart" => runtime_pack::handle_runtime_restart(server, request_id).await,
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
        .map_err(|error| attach_request_dispatch_context(error, method.as_ref()));

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
