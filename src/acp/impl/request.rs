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
use std::sync::{Mutex as StdMutex, OnceLock};
use std::time::{Instant, UNIX_EPOCH};

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{debug, info};

// Task-local: carries the current dispatch method through send_result for universal profile injection
tokio::task_local! {
    static DISPATCH_REQUEST_METHOD: String;
}

use crate::acp::background::{run_health_check, run_maintenance_cycle};

use crate::acp::prelude::{
    enforce_checkpoint_capacity, with_acp_lock, ACP_LOCK_PHASE_RATE_LIMITER,
};
use crate::acp::server::AcpServer;
use crate::agent::{AgentAuditLog, AgentTaskEnvelope, Message};
use crate::config::{
    collect_config_warnings, collect_production_strict_violations, validate_runtime_readiness,
    AppConfig, AutoTuneState,
};
use crate::evaluation::TraceEvent;

use crate::acp::helpers::policy::{rank_execution_agents, resolve_review_policy};
use crate::acp::helpers::requirement::{
    parse_requirement_contract_from_params, resolve_learning_clarification_metrics,
};
use crate::flow_with_models::FlowModelSelector;
use crate::governance::hardening::{
    enforce_action, policy_bundle_for_target, task_budget_for_target, AuditLogger,
    AutonomousEditAuditEntry, BudgetTracker, GovernanceAction,
};
use crate::i18n::runtime::{t, tf};
use crate::memory_module::{MemoryClass, MemoryEntry, MemoryPromotionReport, MemoryStore};
use crate::orchestration::orchestrator::OrchestrationContext;
use crate::orchestration::skill_import::{
    ImportedSkillRecord, SkillImportManifest, SkillImportPolicy, SkillImportRequest,
    SkillImportStore,
};
use crate::orchestration::task_router::TaskRouter;
use crate::protocol::access_mode::RequestDispatchMode;
use crate::pua::{
    load_learning_records, DynamicQualityCompass, LearningRecord, PuaEnforcementPlan,
    PuaExecutionReport, PuaFeedbackCollector, PuaRuleEngine, PuaStageRequirement, TaskContext,
    TaskType,
};
use crate::reinforcement::{
    build_runtime_healthcheck_report, build_task_plan, build_workflow_generated_artifact,
    persist_clarification_session_artifact, persist_consultation_artifact,
    persist_execution_decision, persist_primary_secondary_failover_artifact,
    persist_primary_secondary_policy_artifact, persist_requirement_contract, persist_task_plan,
    persist_workflow_generated, persist_workflow_learning_event, persist_workflow_research,
    recommend_agent_order_from_execution_history, recommend_failure_strategy_from_learning,
    recommend_parallelism_from_learning, recommend_predicted_success_rate_from_learning,
    recommend_work_grade_from_learning, run_action_check, ActionCheckKind, ArtifactLedger,
    CheckStatus, ClarificationSessionArtifact, ConsultationArtifact, ExecutionAssignmentRecord,
    ExecutionDecisionArtifact, ExecutionDecisionCandidate, KnowledgeBusArtifact,
    ParallelPhaseDecisionRecord, PrimaryFailoverReportItem, PrimarySecondaryFailoverArtifact,
    PrimarySecondaryPolicyArtifact, RequirementContractArtifact, WorkflowGeneratedArtifact,
    WorkflowLearningBusArtifact, WorkflowLearningEvent, WorkflowResearchArtifact,
};
use crate::tool::{ToolInput, ToolRegistry};

use crate::rpc_protocol::{value_to_id, JsonRpcRequest, RequestTraceContext};

mod auth_middleware;
mod chat_pack;
mod checkpoint_pack;
mod config_handlers;
mod config_pack;
mod diagnostic_pack;
pub(crate) mod exec_pack;
mod governance_handlers;
mod governance_pack;
mod hardness_pack;
mod health_pack;
mod learning_pack;
mod lifecycle_handlers;
mod lifecycle_pack;
mod method_router;
mod metrics_pack;
mod ops_pack;
pub mod prompts_pack;
mod protocol;
mod protocol_pack;
mod pua_pack;
mod repro_handlers;
mod repro_pack;
mod runtime_pack;
mod status_pack;
pub(crate) mod tools_pack;
mod trace_pack;
mod util;
pub(crate) mod workflow_pack;
use self::chat_pack::{parse_messages, send_error, send_result};
pub(crate) use self::checkpoint_pack::create_checkpoint_record;
pub(crate) use self::checkpoint_pack::persist_checkpoint_metacognitive_loop;
use self::checkpoint_pack::*;
use self::config_pack::*;
use self::diagnostic_pack::*;
#[allow(unused_imports)] // sub-modules re-export many items; parent only uses subset
use self::exec_pack::*;
pub use self::governance_pack::build_knowledge_refinement_profile;
pub use self::governance_pack::build_learning_profile;
pub(crate) use self::governance_pack::inject_platform_profiles_if_absent;
use self::governance_pack::*;
use self::hardness_pack::*;
use self::health_pack::*;
use self::learning_pack::*;
use self::lifecycle_pack::*;
use self::protocol::*;
pub use self::protocol_pack::record_tool_call_audit_with_protocol;
use self::protocol_pack::*;
use self::pua_pack::*;
use self::runtime_pack::*;
use self::status_pack::*;
use self::tools_pack::*;
use self::trace_pack::*;
use self::util::*;

pub(crate) fn append_trace_event(event: TraceEvent) {
    let mut guard = trace_events().lock().unwrap_or_else(|poisoned| {
        tracing::warn!("lock poisoned, recovering");
        poisoned.into_inner()
    });
    guard.push(event);
    if guard.len() > 2048 {
        let overflow = guard.len() - 2048;
        guard.drain(0..overflow);
    }
}

// GAP-B50-36: Authenticate a JSON-RPC request before dispatch.
/// Extract authentication token from request params or HTTP headers and validate it.
/// Returns None if auth is disabled (local profile backward compat) or the session.
/// When `http_headers` is Some, prefers HttpAuthProvider over JsonRpcAuthProvider.
pub fn authenticate_request(
    server: &AcpServer,
    request: &JsonRpcRequest,
    http_headers: Option<&str>,
) -> Result<
    Option<crate::acp::r#impl::session::UserSession>,
    Box<crate::acp::r#impl::session::TokenIntrospectResult>,
> {
    use auth_middleware::{AuthMiddleware, HttpAuthProvider, JsonRpcAuthProvider};

    let provider: &dyn auth_middleware::AuthProvider = if let Some(headers) = http_headers {
        &HttpAuthProvider { headers }
    } else {
        &JsonRpcAuthProvider {
            params: &request.params,
        }
    };

    match AuthMiddleware::authenticate(provider, server) {
        Ok(session) => Ok(session),
        Err(reason) => Err(Box::new(
            crate::acp::r#impl::session::TokenIntrospectResult {
                valid: false,
                session: None,
                reason: Some(reason),
            },
        )),
    }
}

/// Handle JSON-RPC request
///
/// Handle a JSON-RPC request.
///
/// `#[tracing::instrument]` creates the root tracing span.
/// We avoid `.entered()` to keep the future `Send` (EnteredSpan is !Send).
#[tracing::instrument(skip(server, request))]
pub async fn handle_request(
    server: &AcpServer,
    mut request: JsonRpcRequest,
    http_headers: Option<&str>,
) -> Result<()> {
    // GAP-B50-36: Authenticate request before dispatch
    match authenticate_request(server, &request, http_headers) {
        Ok(Some(session)) => {
            tracing::debug!(
                user_id = %session.user_id,
                roles = ?session.roles,
                "Request authenticated"
            );
        }
        Ok(None) => {
            // Auth disabled (local profile), proceed
        }
        Err(auth_result) => {
            let reason = auth_result
                .reason
                .unwrap_or_else(|| "Unknown auth error".into());
            tracing::warn!("Authentication failed: {}", reason);
            return send_error(
                server,
                request.id,
                AcpErrorCode::AuthRequired as i32,
                format!("Authentication failed: {}", reason),
                Some(serde_json::json!({
                    "code": "AUTH_REQUIRED",
                    "reason": reason,
                })),
            )
            .await;
        }
    }

    // GAP-B52: Rate limiting — check tenant rate before dispatch
    if server.runtime_config.entry_auth_enabled || server.runtime_config.governance_enabled {
        let tenant = request
            .params
            .as_ref()
            .and_then(|p| p.get("tenant_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("default");
        let limiter = crate::security::rate_limiter::global_rate_limiter();
        if !limiter.try_consume_tenant(tenant, 1.0).await {
            return send_error(
                server,
                request.id,
                -32029, // JSON-RPC rate limited
                format!("Rate limit exceeded for tenant '{}'", tenant),
                Some(serde_json::json!({
                    "code": "RATE_LIMITED",
                    "tenant": tenant,
                    "retry_after_ms": 1000,
                })),
            )
            .await;
        }
        // Global concurrency limiting via semaphore was removed (F-GAP-49).
        // The per-tenant rate limiter above provides sufficient throttling.
    }

    // GAP-B52-23: Request signature verification
    if server.runtime_config.request_signing_enabled {
        let signing_key = if !server.runtime_config.request_signing_public_key.is_empty() {
            use base64::Engine;
            let b64_engine = base64::engine::general_purpose::STANDARD;
            match b64_engine.decode(&server.runtime_config.request_signing_public_key) {
                Ok(key) => Some((
                    key,
                    crate::security::request_signing::SigningAlgorithm::Ed25519,
                )),
                Err(_) => {
                    tracing::warn!(
                        "request_signing: invalid base64 public key, falling back to HMAC"
                    );
                    if !server.runtime_config.request_signing_hmac_secret.is_empty() {
                        Some((
                            server
                                .runtime_config
                                .request_signing_hmac_secret
                                .as_bytes()
                                .to_vec(),
                            crate::security::request_signing::SigningAlgorithm::HmacSha256,
                        ))
                    } else {
                        None
                    }
                }
            }
        } else if !server.runtime_config.request_signing_hmac_secret.is_empty() {
            Some((
                server
                    .runtime_config
                    .request_signing_hmac_secret
                    .as_bytes()
                    .to_vec(),
                crate::security::request_signing::SigningAlgorithm::HmacSha256,
            ))
        } else {
            None
        };

        if let Some((ref key, _algo)) = signing_key {
            // Extract the _signature field from request params
            let sig_from_params = request.params.as_ref().and_then(|params| {
                params.get("_signature").and_then(|s| {
                    serde_json::from_value::<crate::security::request_signing::RequestSignature>(
                        s.clone(),
                    )
                    .ok()
                })
            });

            match sig_from_params {
                Some(ref request_sig) => {
                    // Serialize the params without the _signature field for body verification.
                    // Use mutable access to temporarily remove _signature, serialize, then restore.
                    let body_for_verification = request
                        .params
                        .as_mut()
                        .map(|params| {
                            let removed = params
                                .as_object_mut()
                                .and_then(|map| map.remove("_signature"));
                            let serialized = serde_json::to_vec(params).unwrap_or_default();
                            if let Some(sig) = removed {
                                if let serde_json::Value::Object(ref mut map) = params {
                                    map.insert("_signature".to_string(), sig);
                                }
                            }
                            serialized
                        })
                        .unwrap_or_default();

                    match crate::security::request_signing::verify_request(
                        key,
                        &body_for_verification,
                        request_sig,
                    ) {
                        Ok(true) => {
                            tracing::debug!(
                                "Request signature verified (key_id={})",
                                request_sig.key_id
                            );
                        }
                        Ok(false) | Err(_) => {
                            tracing::warn!("Request signature verification failed");
                            return send_error(
                                server,
                                request.id,
                                AcpErrorCode::AuthRequired as i32,
                                "Request signature verification failed".into(),
                                Some(serde_json::json!({
                                    "code": "SIGNATURE_INVALID",
                                    "reason": "The request signature is invalid or the body has been tampered with",
                                })),
                            )
                            .await;
                        }
                    }
                }
                None => {
                    // Signature required but missing
                    tracing::warn!("request_signing: missing _signature in request params");
                    return send_error(
                        server,
                        request.id,
                        AcpErrorCode::AuthRequired as i32,
                        "Request signature required but not provided".into(),
                        Some(serde_json::json!({
                            "code": "SIGNATURE_REQUIRED",
                            "reason": "This endpoint requires signed requests. Include a `_signature` parameter with a valid Ed25519 or HMAC-SHA256 signature.",
                        })),
                    )
                    .await;
                }
            }
        } else {
            tracing::warn!("request_signing: enabled but no signing key configured");
            return send_error(
                server,
                request.id,
                -32000,
                "Request signing is enabled but no verification key is configured".into(),
                Some(serde_json::json!({
                    "code": "SIGNING_CONFIG_ERROR",
                    "reason": "Server has request signing enabled but no Ed25519 public key or HMAC secret configured",
                })),
            )
            .await;
        }
    }

    // ── Rate limiting (S-FIX1) ───────────────────────────────────────────
    // Apply global and per-tenant rate limits before dispatching the request.
    {
        let rate_limiter = crate::security::rate_limiter::global_rate_limiter();

        // Global concurrency semaphore was removed (F-GAP-49).
        // Per-tenant throttling is retained below.

        // Extract tenant_id from request params for per-tenant throttling.
        let tenant_id = request
            .params
            .as_ref()
            .and_then(|p| p.get("tenant_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("default");

        if !rate_limiter.try_consume_tenant(tenant_id, 1.0).await {
            anyhow::bail!("rate limit exceeded for tenant: {}", tenant_id);
        }
    }

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
                    AcpErrorCode::MethodNotFound as i32,
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
                    AcpErrorCode::MethodNotFound as i32,
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

    // BLUE56-D06: RBAC access check for all endpoints
    let rbac_denied = if let Some(ref rbac) = server.governance_deps.rbac_enforcer {
        if let Ok(guard) = rbac.read() {
            let permission = method_to_permission(method.as_ref());
            let principal = request_to_principal(&request);
            match guard.check_access(&principal, &permission) {
                crate::governance::rbac::AccessDecision::Deny { ref reason } => {
                    Some(format!("Access denied: {}", reason))
                }
                crate::governance::rbac::AccessDecision::Escalate { ref required_role } => {
                    Some(format!("Access requires role: {}", required_role))
                }
                crate::governance::rbac::AccessDecision::Allow => None,
            }
        } else {
            None
        }
    } else {
        None
    };
    // Guard is dropped here — safe to await
    if let Some(error_msg) = rbac_denied {
        let is_escalation = error_msg.contains("requires role");
        tracing::warn!(target: "rbac", error = %error_msg, "RBAC check failed");
        return send_error(
            server,
            request.id,
            AcpErrorCode::AuthRequired as i32,
            error_msg,
            Some(serde_json::json!({
                "code": if is_escalation { "ESCALATION_REQUIRED" } else { "ACCESS_DENIED" },
            })),
        )
        .await;
    }

    // BLUE56-D07: Hash chain audit integrity — append to hash chain
    if let Some(ref hca) = server.governance_deps.hash_chain_auditor {
        if let Ok(mut hca_guard) = hca.lock() {
            let payload = serde_json::json!({
                "id": request.id.as_ref().map(|v| format!("{:?}", v)).unwrap_or_default(),
                "method": method.as_ref(),
                "params": request.params,
                "timestamp_ms": std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis(),
            });
            let _ = hca_guard.append(payload);
        }
    }

    let pua_engine = PuaRuleEngine::new(server.governance_deps.pua_enforcement_plan.clone());
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
            tf(
                "error.request.pua_red_line_violation",
                &[("detail", &violation.detail)],
            ),
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
                    tf(
                        "error.request.pua_stage_violation",
                        &[("detail", &violation.detail)],
                    ),
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
    let _request_span = {
        let telemetry_guard = server
            .observability
            .telemetry_runtime
            .lock()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned, recovering");
                poisoned.into_inner()
            });
        telemetry_guard.start_root_span(
            "acp.request",
            &format!("{}:{}", trace.method, trace.request_id),
            vec![],
        )
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
            // B51-28: Try the MethodRouter first; fall through to legacy match
            // if no handler is registered.
            if let Some(router_result) = method_router::global_method_router()
                .dispatch(
                    method.as_ref(),
                    server,
                    request.params.clone().unwrap_or_default(),
                    request_id.clone(),
                    &trace,
                )
                .await
            {
                return router_result;
            }

            match method.as_ref() {
                "initialize" => protocol_pack::handle_initialize(server, request_id).await,
                // Standard ACP session lifecycle methods
                "session/new" => {
                    protocol_pack::handle_session_new(
                        server,
                        request.params.clone().unwrap_or_default(),
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
                "session/resume" => {
                    protocol_pack::handle_session_resume(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "session/close" => {
                    protocol_pack::handle_session_close(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "session/request_permission" => {
                    protocol_pack::handle_session_request_permission(
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
                // Terminal methods
                "terminal/create" => {
                    protocol_pack::handle_terminal_create(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "terminal/output" => {
                    protocol_pack::handle_terminal_output(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "terminal/release" => {
                    protocol_pack::handle_terminal_release(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "terminal/kill" => {
                    protocol_pack::handle_terminal_kill(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "terminal/wait_for_exit" => {
                    protocol_pack::handle_terminal_wait_for_exit(
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
                                AcpErrorCode::InternalError as i32,
                                tf(
                                    "error.request.prompts_list_failed",
                                    &[("error", &e.to_string())],
                                ),
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
                                AcpErrorCode::InternalError as i32,
                                tf(
                                    "error.request.prompts_search_failed",
                                    &[("error", &e.to_string())],
                                ),
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
                                    send_error(
                                        server,
                                        request_id,
                                        AcpErrorCode::InvalidParams as i32,
                                        format!("{}", e),
                                        None,
                                    )
                                    .await
                                }
                            }
                        }
                        None => {
                            send_error(
                                server,
                                request_id,
                                AcpErrorCode::InvalidParams as i32,
                                tf("error.request.missing_field_id", &[]),
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
                                AcpErrorCode::InternalError as i32,
                                tf(
                                    "error.request.prompts_create_failed",
                                    &[("error", &e.to_string())],
                                ),
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
                                AcpErrorCode::InternalError as i32,
                                tf(
                                    "error.request.prompts_update_failed",
                                    &[("error", &e.to_string())],
                                ),
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
                                AcpErrorCode::InternalError as i32,
                                tf(
                                    "error.request.prompts_delete_failed",
                                    &[("error", &e.to_string())],
                                ),
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
                "metrics.get" => metrics_pack::handle_metrics_get(server, request_id).await,
                "metrics" => metrics_pack::handle_metrics(server, request_id).await,
                "metrics.prometheus" => {
                    metrics_pack::handle_metrics_prometheus(server, request_id).await
                }
                "metrics.window.query" => {
                    metrics_pack::handle_metrics_window_query(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "metrics.errors.summary" => {
                    metrics_pack::handle_metrics_errors_summary(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "metrics.reset" => metrics_pack::handle_metrics_reset(server, request_id).await,
                "debug_panel.get" | "debug.panel.get" => {
                    config_handlers::handle_debug_panel_get(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "trace.get" => {
                    config_handlers::handle_trace_get(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "trace.metrics" => config_handlers::handle_trace_metrics(server, request_id).await,
                "shutdown" => lifecycle_handlers::handle_shutdown(server, request_id).await,
                "health" | "runtime.health" => {
                    lifecycle_handlers::handle_health(server, request_id).await
                }
                "health.probes" => {
                    lifecycle_handlers::handle_health_probes(server, request_id).await
                }
                "lock.status" => {
                    handle_lock_status(server, request.params.unwrap_or_default(), request_id).await
                }
                "runtime.self_model" => {
                    lifecycle_handlers::handle_runtime_self_model(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "provider.status" => {
                    lifecycle_handlers::handle_provider_status(
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
                    lifecycle_handlers::handle_runtime_stability(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "runtime.features" => {
                    lifecycle_handlers::handle_runtime_features(server, request_id).await
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
                    repro_handlers::handle_optimization_peak(
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
                    handle_task_execute(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                        &trace,
                    )
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
                    governance_handlers::handle_governance_status(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "capabilities.list" => {
                    lifecycle_handlers::handle_capabilities_list(server, request_id).await
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
                    governance_handlers::handle_governance_plan_get(server, request_id).await
                }
                "governance.plan.update" => {
                    governance_handlers::handle_governance_plan_update(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "governance.audit.recent" => {
                    governance_handlers::handle_governance_audit_recent(
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
                    governance_handlers::handle_governance_remediate(
                        server,
                        request.params.unwrap_or_default(),
                        request_id,
                    )
                    .await
                }
                "governance.config.save" => {
                    governance_handlers::handle_governance_config_save(
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
                "runtime.restart" => {
                    lifecycle_handlers::handle_runtime_restart(server, request_id).await
                }
                _ => {
                    let localized = tf(
                        "error.request.unknown_method",
                        &[("method", &request.method)],
                    );
                    let descriptive = format!("unknown method: {}", request.method);
                    send_error(
                        server,
                        request_id,
                        AcpErrorCode::MethodNotFound as i32,
                        if localized.contains("unknown method")
                            || localized.contains("method not found")
                        {
                            localized
                        } else {
                            format!("{} ({})", descriptive, localized)
                        },
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

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    #[cfg(not(feature = "backend-postgres"))]
    use super::collect_vector_context_snippets;
    use super::{
        attach_request_dispatch_context, classify_request_error_kind, infer_workflow_parallelism,
        is_acp_request, rebalance_execution_order, session_id_for_task, summarize_lock_health,
        with_error_contract_data, AcpErrorCode,
    };
    use crate::acp::prelude::AcpLockSnapshot;
    #[cfg(not(feature = "backend-postgres"))]
    use crate::vector::VectorStore;

    #[test]
    fn is_acp_request_recognizes_known_methods() {
        // Key protocol methods
        assert!(is_acp_request("initialize"));
        assert!(is_acp_request("chat"));
        assert!(is_acp_request("session/new"));
        assert!(is_acp_request("shutdown"));
        // MCP-bridge methods
        assert!(is_acp_request("mcp.initialize"));
        assert!(is_acp_request("mcp.tools.list"));
        assert!(is_acp_request("mcp.tools.call"));
        // Skill methods
        assert!(is_acp_request("skill.import"));
        assert!(is_acp_request("skill.create"));
        // Workflow methods
        assert!(is_acp_request("workflow.execute"));
        assert!(is_acp_request("workflow.confirm"));
        // Prompt methods
        assert!(is_acp_request("prompts.list"));
        assert!(is_acp_request("prompts.get"));
        // Unknown methods return false
        assert!(!is_acp_request("unknown.method"));
        assert!(!is_acp_request(""));
        assert!(!is_acp_request("tools/list"));
    }

    #[test]
    fn session_id_for_task_compacts_to_ascii_alnum() {
        let value = session_id_for_task("Fix #123: add review stage and docs");
        // When i18n is loaded, returns formatted template with id.
        // In bare test mode, falls back to i18n key or formatted template.
        assert!(!value.is_empty());
        // The compact id should appear in the formatted result or fallback key
        let has_compact_id = value.contains("Fix123addreviewstageand");
        let has_fallback = value.contains("info.request.session_id_format");
        assert!(has_compact_id || has_fallback, "value: {value}");
    }

    #[test]
    fn session_id_for_task_has_fallback_when_empty() {
        let value = session_id_for_task("!!!");
        // Empty task chars → fallback to "session"
        let has_session = value.contains("session");
        let has_fallback = value.contains("info.request.session_id_format");
        assert!(has_session || has_fallback, "value: {value}");
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
            AcpErrorCode::InternalError as i32,
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

    // ── ACP method dispatch ───────────────────────────────────────────

    // ── get_protocol_mode ─────────────────────────────────────────────

    // ── handle_request unknown method ─────────────────────────────────

    // ── dispatch error context ────────────────────────────────────────

    #[test]
    fn attach_request_dispatch_context_adds_method() {
        let err = anyhow::anyhow!("test error");
        let wrapped = attach_request_dispatch_context(err, "test.method");
        let msg = wrapped.to_string();
        assert!(msg.contains("test.method"));
        assert!(msg.contains("acp.handle_request.dispatch"));
    }

    // ── Lock health summary ───────────────────────────────────────────

    #[test]
    fn lock_health_summary_healthy_with_no_issues() {
        let summary = summarize_lock_health(&[]);
        assert_eq!(summary.status, "healthy");
        assert_eq!(summary.components_tracked, 0);
    }

    #[test]
    fn lock_health_summary_no_poisoned_healthy() {
        let summary = summarize_lock_health(&[AcpLockSnapshot {
            name: "test".to_string(),
            acquisitions: 5,
            poisoned_total: 0,
            recovered_total: 0,
            slow_wait_total: 0,
            avg_wait_ms: 0.1,
            max_wait_ms: 0.5,
        }]);
        assert_eq!(summary.status, "healthy");
        assert_eq!(summary.poisoned_total, 0);
    }
}
