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
use std::sync::Mutex as StdMutex;
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

use crate::acp::prelude::{enforce_checkpoint_capacity, with_acp_lock};
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
use crate::i18n::runtime::{t, tf};
use crate::memory_module::{MemoryClass, MemoryEntry, MemoryPromotionReport, MemoryStore};
use crate::orchestration::orchestrator::OrchestrationContext;
use crate::orchestration::skill_import::{ImportedSkillRecord, SkillImportManifest};
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
    recommend_agent_order_from_execution_history, run_action_check, ActionCheckKind,
    ArtifactLedger, CheckStatus, ClarificationSessionArtifact, ConsultationArtifact,
    ExecutionAssignmentRecord, ExecutionDecisionArtifact, ExecutionDecisionCandidate,
    KnowledgeBusArtifact, ParallelPhaseDecisionRecord, PrimaryFailoverReportItem,
    PrimarySecondaryFailoverArtifact, PrimarySecondaryPolicyArtifact, RequirementContractArtifact,
    WorkflowGeneratedArtifact, WorkflowLearningBusArtifact, WorkflowLearningEvent,
    WorkflowResearchArtifact,
};
use crate::tool::{ToolInput, ToolRegistry};

use crate::rpc_protocol::{value_to_id, JsonRpcRequest, RequestTraceContext};

mod auth_middleware;
mod chat_pack;
mod checkpoint_pack;
mod config_handlers;
mod config_pack;
mod diagnostic_pack;
mod dispatch;
pub(crate) mod exec_pack;
mod governance_handlers;
use self::governance_handlers::governance_pack;
mod hardness_pack;
mod health_pack;
mod learning_pack;
mod lifecycle_handlers;
mod lifecycle_pack;
mod metrics_pack;
pub mod prompts_pack;
pub(crate) mod protocol;
pub(crate) mod protocol_pack;
pub(crate) use self::trace_pack::tool_budget_trackers;
mod pua_pack;
mod repro_handlers;
pub(crate) use dispatch::{dispatch_to_client, DispatchOutput};
mod repro_pack;
mod runtime_pack;
mod status_pack;
pub(crate) mod tools_pack;
mod trace_pack;
mod util;
pub(crate) mod workflow_pack;
use self::chat_pack::{parse_messages, send_error};
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
pub(crate) use self::protocol::is_acp_request;
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

/// Record a structured trace event via the trace sink.
///
/// Public wrapper over `trace_pack::record_trace_event` so sibling modules
/// (e.g. the chat session handler) can emit lifecycle trace events.
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_trace_event(
    server: &AcpServer,
    trace: &RequestTraceContext,
    event_type: &str,
    status: &str,
    stage: &str,
    inputs: Value,
    outputs: Option<Value>,
    duration_ms: u64,
) {
    self::trace_pack::record_trace_event(
        server,
        trace,
        event_type,
        status,
        stage,
        inputs,
        outputs,
        duration_ms,
    );
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

    // GAP-B52: Rate limiting is enforced once, unconditionally, below (S-FIX1).
    // A previous conditional gate here consumed a second token from the same
    // tenant bucket whenever entry-auth or governance was enabled, silently
    // halving the effective RPM. The single gate below keeps the same
    // per-tenant throttling for every request and returns a structured
    // JSON-RPC error (-32029) when the limit is exceeded.

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
                AcpErrorCode::ServerError as i32,
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
    // Apply per-tenant rate limits once, before dispatching the request.
    // This is the single rate-limit gate: the previous conditional gate
    // (GAP-B52) was removed because it charged the same tenant bucket twice
    // per request and used an unstructured bail that produced no JSON-RPC
    // error for the client.
    {
        // Extract tenant_id from request params for per-tenant throttling.
        let tenant_id = request
            .params
            .as_ref()
            .and_then(|p| p.get("tenant_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("default");

        // Rate limiting is optional: when no middleware is configured, allow.
        if server
            .rate_limiting
            .rate_limit_middleware
            .as_ref()
            .is_some_and(|r| !r.try_consume_tenant(tenant_id, 1.0))
        {
            return send_error(
                server,
                request.id,
                -32029, // JSON-RPC rate limited
                format!("Rate limit exceeded for tenant '{}'", tenant_id),
                Some(serde_json::json!({
                    "code": "RATE_LIMITED",
                    "tenant": tenant_id,
                    "retry_after_ms": 1000,
                })),
            )
            .await;
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

    // BLUE56-D07: request ledger — every accepted request leaves a tamper-
    // evident record in the canonical audit sink. The sink's writer thread
    // appends each record to the hash chain, so no separate auditor plumbing
    // or blocking-pool offload is needed on this hot path (the old
    // spawn_blocking + HashChainAuditor append is subsumed by the sink).
    // Only request identity and timing are retained — the full payload is
    // deliberately NOT kept in memory/on disk to keep the bounded buffer lean;
    // content-level evidence lives in conversation checkpoints and memory.
    let ledger_tenant = request
        .params
        .as_ref()
        .and_then(|p| p.get("tenant_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("default")
        .to_string();
    let ledger_id = request
        .id
        .as_ref()
        .map(|v| format!("{:?}", v))
        .unwrap_or_default();
    crate::governance::audit::global_audit_log().record(crate::governance::audit::AuditLogEntry {
        timestamp: crate::governance::audit::chrono_now(),
        task_id: ledger_id,
        phase: "request".to_string(),
        agent: None,
        tool: None,
        decision: method.as_ref().to_string(),
        inputs: serde_json::Value::Null,
        outputs: None,
        error: None,
        confidence: None,
        data_classification: None,
        compliance_tags: vec![],
        retention_policy: None,
        correlation_id: Some(ledger_tenant),
    });

    let pua_engine = PuaRuleEngine::new(server.governance_deps.pua_enforcement_plan.clone());
    let task_type = infer_task_type(method.as_ref(), &request.params);
    let task_context = TaskContext {
        task_type: task_type.clone(),
        file_count: infer_file_count(&request.params),
        risk_score: infer_risk_score(method.as_ref(), &task_type),
    };
    // NOTE: DynamicQualityCompass is only needed on the PUA violation error
    // paths below, so it is built lazily inside those branches instead of
    // on every passing request.

    if let Err(violation) = pua_engine.check_red_lines(method.as_ref()) {
        return send_error(
            server,
            request.id,
            AcpErrorCode::PuaViolation as i32,
            tf(
                "error.request.pua_red_line_violation",
                &[("detail", &violation.detail)],
            ),
            Some(json!({
                "type": "pua_violation",
                "kind": format!("{:?}", violation.kind),
                "method": method.as_ref(),
                "detail": violation.detail,
                "quality_compass": DynamicQualityCompass::default()
                    .get_checks(&task_context)
                    .into_iter()
                    .map(|check| check.description)
                    .collect::<Vec<_>>(),
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
        // Persist the learning record off the request hot path: the write is
        // synchronous open-append-close disk I/O, so run it on the blocking
        // pool. The collector is a process-wide singleton, so the record is
        // cloned and moved into the task.
        {
            let collector = pua_feedback_collector();
            let record = report.clone();
            tokio::task::spawn_blocking(move || {
                if let Err(err) = collector.collect(&record) {
                    debug!("failed to persist PUA feedback report: {}", err);
                }
            });
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
                    AcpErrorCode::PuaViolation as i32,
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
                        "quality_compass": DynamicQualityCompass::default()
                            .get_checks(&task_context)
                            .into_iter()
                            .map(|check| check.description)
                            .collect::<Vec<_>>(),
                    })),
                )
                .await;
            }
        }
    }

    let started = Instant::now();
    server.observability.metrics.inc_active_requests();
    // DrainGuard: track this request so graceful shutdown can wait for it.
    // The RAII permit is released on every exit path (including the early
    // returns above and the dispatch below).
    let _drain_permit = server.drain_guard.acquire().await;
    if _drain_permit.is_none() {
        // Server is draining — reject new requests immediately.
        return send_error(
            server,
            request.id,
            AcpErrorCode::ServerError as i32,
            "Server is shutting down".into(),
            Some(serde_json::json!({
                "code": "SERVER_DRAINING",
                "reason": "The server is draining in-flight requests before shutdown",
            })),
        )
        .await;
    }
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

    // Single dispatch table (the match below). The former registration-based
    // MethodRouter (B51-28) was merged back into this match: every handler it
    // registered was a thin forward to the same protocol_pack payload functions
    // used here, and its early `return` skipped the request-complete metrics /
    // trace tail, leaking active_requests. Merging restores a single dispatch
    // path with correct accounting.

    // Use the potentially normalized method for dispatch.
    let result = DISPATCH_REQUEST_METHOD
        .scope(method.to_string(), async {
            match method.as_ref() {
                "initialize" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::initialize_payload(server, &request.params).await,
                    )
                    .await
                }
                // Standard ACP session lifecycle methods
                "session/new" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::session_new_payload(
                            server,
                            request.params.clone().unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "session/load" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::session_load_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "session/prompt" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::session_prompt_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "session/cancel" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::session_cancel_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "session/list" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::session_list_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "session/set_mode" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::session_set_mode_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "session/set_config_option" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::session_set_config_option_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "session/resume" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::session_resume_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "session/close" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::session_close_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "session/request_permission" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::session_request_permission_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                // Former MethodRouter-only session methods (kept reachable in
                // Auto mode; ACP mode still gates them via is_acp_request).
                "session/delete" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::session_delete_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "session/config/set" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::session_config_set_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "session/config/get" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::session_config_get_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "session/config/favorite/toggle" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::session_config_favorite_toggle_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                // Standard ACP authentication methods
                "authenticate" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::authenticate_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "logout" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::logout_payload(server, request.params.unwrap_or_default())
                            .await,
                    )
                    .await
                }
                // Protocol-level notifications
                "$/cancel_request" => {
                    // $/cancel_request is a notification per JSON-RPC spec — no response
                    let target_id = request
                        .params
                        .as_ref()
                        .and_then(|p| p.get("id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    tracing::warn!(
                        target: "acp::protocol_pack",
                        target_request = %target_id,
                        "cancel_request_payload: cancelling request {}",
                        target_id
                    );
                    dispatch_to_client(server, request_id, Ok(DispatchOutput::silent())).await
                }
                // MCP methods bridged through ACP dispatch
                "mcp.initialize" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::mcp_initialize_payload(server).await,
                    )
                    .await
                }
                "mcp.notifications_initialized" => {
                    // MCP notification — no response expected per JSON-RPC spec
                    dispatch_to_client(server, request_id, Ok(DispatchOutput::silent())).await
                }
                "mcp.ping" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::mcp_ping_payload(server).await,
                    )
                    .await
                }
                "mcp.tools.list" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::mcp_tools_list_payload(server).await,
                    )
                    .await
                }
                "mcp.tools.call" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::mcp_tools_call_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "mcp.resources.list" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::mcp_resources_list_payload(server).await,
                    )
                    .await
                }
                "mcp.resources.read" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::mcp_resources_read_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "mcp.resources.subscribe" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::mcp_resources_subscribe_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "mcp.logging.setLevel" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::mcp_logging_set_level_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "mcp.completion.complete" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::mcp_completion_complete_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "mcp.sampling.createMessage" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::mcp_sampling_create_message_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "mcp.prompts.list" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::mcp_prompts_list_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "mcp.prompts.get" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::mcp_prompts_get_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                // Terminal methods
                "terminal/create" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::terminal_create_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "terminal/output" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::terminal_output_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "terminal/release" => {
                    dispatch_to_client(
                        server,
                        request_id,
                        protocol_pack::handle_terminal_release(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "terminal/kill" => {
                    dispatch_to_client(
                        server,
                        request_id,
                        protocol_pack::handle_terminal_kill(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "terminal/wait_for_exit" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::terminal_wait_for_exit_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
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
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        prompts_pack::handle_prompts_list(&server.prompt_manager, lang)
                            .map_err(|e| anyhow::anyhow!("{}", e)),
                    )
                    .await
                }
                "prompts.search" => {
                    let lang = request
                        .params
                        .as_ref()
                        .and_then(|p| p.get("lang"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(&server.runtime_config.i18n_default_language);
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        prompts_pack::handle_prompts_search(
                            &server.prompt_manager,
                            lang,
                            request
                                .params
                                .as_ref()
                                .and_then(|p| p.get("query"))
                                .and_then(|v| v.as_str())
                                .unwrap_or(""),
                        )
                        .map_err(|e| anyhow::anyhow!("{}", e)),
                    )
                    .await
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
                            crate::acp::r#impl::io::respond(
                                server,
                                request_id,
                                prompts_pack::handle_prompts_get(&server.prompt_manager, lang, id)
                                    .map_err(|e| anyhow::anyhow!("{}", e)),
                            )
                            .await
                        }
                        None => {
                            dispatch_to_client(
                                server,
                                request_id,
                                Ok(DispatchOutput::error(
                                    AcpErrorCode::InvalidParams as i32,
                                    tf("error.request.missing_field_id", &[]),
                                )),
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
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        prompts_pack::handle_prompts_create(&server.prompt_manager, lang, &params)
                            .map_err(|e| anyhow::anyhow!("{}", e)),
                    )
                    .await
                }
                "prompts.update" => {
                    let lang = request
                        .params
                        .as_ref()
                        .and_then(|p| p.get("lang"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(&server.runtime_config.i18n_default_language);
                    let params = request.params.clone().unwrap_or_default();
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        prompts_pack::handle_prompts_update(&server.prompt_manager, lang, &params)
                            .map_err(|e| anyhow::anyhow!("{}", e)),
                    )
                    .await
                }
                "prompts.delete" => {
                    let lang = request
                        .params
                        .as_ref()
                        .and_then(|p| p.get("lang"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(&server.runtime_config.i18n_default_language);
                    let params = request.params.clone().unwrap_or_default();
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        prompts_pack::handle_prompts_delete(&server.prompt_manager, lang, &params)
                            .map_err(|e| anyhow::anyhow!("{}", e)),
                    )
                    .await
                }
                "skill.import" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::skill_import_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "skill.enable" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::skill_enabled_toggle_payload(
                            server,
                            request.params.unwrap_or_default(),
                            true,
                        )
                        .await,
                    )
                    .await
                }
                "skill.disable" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::skill_enabled_toggle_payload(
                            server,
                            request.params.unwrap_or_default(),
                            false,
                        )
                        .await,
                    )
                    .await
                }
                "skill.list_imported" | "skill.list" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::skill_list_imported_payload(server).await,
                    )
                    .await
                }
                "skill.create" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::skill_create_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "skill.update" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::skill_update_payload(
                            server,
                            &request.params.clone().unwrap_or_default(),
                        ),
                    )
                    .await
                }
                "skill.version.list" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::skill_version_list_payload(
                            server,
                            &request.params.clone().unwrap_or_default(),
                        ),
                    )
                    .await
                }
                "skill.version.rollback" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::skill_version_rollback_payload(
                            server,
                            &request.params.clone().unwrap_or_default(),
                        ),
                    )
                    .await
                }
                "skill.remove" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::skill_remove_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "tools/list" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::acp_tools_list_payload(server).await,
                    )
                    .await
                }
                "tools/call" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::acp_tools_call_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "chat" => {
                    dispatch_to_client(
                        server,
                        request_id.clone(),
                        protocol_pack::handle_chat(
                            server,
                            request_id,
                            request.params.unwrap_or_default(),
                            &trace,
                        )
                        .await,
                    )
                    .await
                }
                "phase" | "phase.status" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::phase_payload(
                            server,
                            request.params.unwrap_or_default(),
                            &trace,
                        )
                        .await,
                    )
                    .await
                }
                "metrics.get" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        metrics_pack::metrics_get_payload(server).await,
                    )
                    .await
                }
                "metrics" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        metrics_pack::metrics_payload(server).await,
                    )
                    .await
                }
                "metrics.prometheus" => {
                    dispatch_to_client(
                        server,
                        request_id,
                        metrics_pack::handle_metrics_prometheus(server).await,
                    )
                    .await
                }
                "metrics.window.query" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        Ok(runtime_pack::metrics_window_query_payload(
                            server,
                            &request.params.unwrap_or_default(),
                        )),
                    )
                    .await
                }
                "metrics.errors.summary" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        Ok(runtime_pack::metrics_errors_summary_payload(
                            server,
                            &request.params.unwrap_or_default(),
                        )),
                    )
                    .await
                }
                "metrics.reset" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        metrics_pack::metrics_reset_payload(server).await,
                    )
                    .await
                }
                "debug_panel.get" | "debug.panel.get" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        config_handlers::debug_panel_payload(server).await,
                    )
                    .await
                }
                "trace.get" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        config_handlers::trace_payload_result(&request.params.unwrap_or_default()),
                    )
                    .await
                }
                "trace.metrics" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        Ok(trace_metrics_snapshot(server)),
                    )
                    .await
                }
                "shutdown" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        lifecycle_handlers::shutdown_payload(server),
                    )
                    .await
                }
                "health" | "runtime.health" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        lifecycle_handlers::health_payload(server).await,
                    )
                    .await
                }
                "health.probes" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        lifecycle_handlers::build_health_probes_payload(server).await,
                    )
                    .await
                }
                "lock.status" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        lock_status_payload(server, request.params.unwrap_or_default()).await,
                    )
                    .await
                }
                "runtime.self_model" => {
                    let params = request.params.unwrap_or_default();
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        lifecycle_handlers::build_runtime_self_model_payload(server, &params).await,
                    )
                    .await
                }
                "provider.status" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        lifecycle_handlers::build_provider_status_payload(server).await,
                    )
                    .await
                }
                "release.readiness" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        release_readiness_payload(server, request.params.unwrap_or_default()).await,
                    )
                    .await
                }
                "runtime.stability" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        lifecycle_handlers::build_runtime_stability_payload(server).await,
                    )
                    .await
                }
                "runtime.features" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        lifecycle_handlers::runtime_features_payload(server),
                    )
                    .await
                }
                "observability.alerts" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        observability_alerts_payload(server, request.params.unwrap_or_default())
                            .await,
                    )
                    .await
                }
                "security.baseline" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        security_baseline_payload(server, request.params.unwrap_or_default()).await,
                    )
                    .await
                }
                "harness.status" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        harness_status_payload(server, request.params.unwrap_or_default()).await,
                    )
                    .await
                }
                "breaker.status" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        breaker_status_payload(server).await,
                    )
                    .await
                }
                "breaker.reset" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        breaker_reset_payload(server, request.params.unwrap_or_default()).await,
                    )
                    .await
                }
                "breaker.recovery" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        breaker_recovery_payload(server, request.params.unwrap_or_default()).await,
                    )
                    .await
                }
                "cache.clear" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        cache_clear_payload(server).await,
                    )
                    .await
                }
                "vector.clear" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        vector_clear_payload(server).await,
                    )
                    .await
                }
                "maintenance.gc" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        maintenance_gc_payload(server).await,
                    )
                    .await
                }
                "data.lifecycle" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        data_lifecycle_payload(server, request.params.unwrap_or_default()).await,
                    )
                    .await
                }
                "action.check" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        runtime_pack::action_check_payload(
                            server,
                            request.params.unwrap_or_default(),
                        ),
                    )
                    .await
                }
                "conversation.checkpoint.create" => {
                    dispatch_to_client(
                        server,
                        request_id,
                        runtime_pack::handle_conversation_checkpoint_create(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "conversation.checkpoint.list" | "checkpoint.list" => {
                    dispatch_to_client(
                        server,
                        request_id,
                        runtime_pack::handle_conversation_checkpoint_list(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "conversation.rollback" => {
                    dispatch_to_client(
                        server,
                        request_id,
                        runtime_pack::handle_conversation_rollback(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "conversation.checkpoint.prune" => {
                    dispatch_to_client(
                        server,
                        request_id,
                        runtime_pack::handle_conversation_checkpoint_prune(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "config.reload" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        config_reload_payload(server).await,
                    )
                    .await
                }
                "config.baseline" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        config_baseline_payload(server, request.params.unwrap_or_default()).await,
                    )
                    .await
                }
                "build.repro" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        repro_pack::build_repro_payload(server).await,
                    )
                    .await
                }
                "optimization.peak" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        repro_handlers::optimization_peak_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "error.contract" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        runtime_pack::error_contract_payload(server),
                    )
                    .await
                }
                "autotune.get" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        runtime_pack::autotune_get_payload(server).await,
                    )
                    .await
                }
                "autotune.status" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        runtime_pack::autotune_status_payload(server).await,
                    )
                    .await
                }
                "autotune.reset" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        runtime_pack::autotune_reset_payload(server).await,
                    )
                    .await
                }
                "selector.status" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        runtime_pack::selector_status_payload(server),
                    )
                    .await
                }
                "hardness.status" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        runtime_pack::hardness_status_payload(
                            server,
                            request.params.unwrap_or_default(),
                        ),
                    )
                    .await
                }
                "cost.status" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        runtime_pack::cost_status_payload(
                            server,
                            request.params.unwrap_or_default(),
                        ),
                    )
                    .await
                }
                "workflow.confirm" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        workflow_pack::workflow_confirm_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "workflow.clarify" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        workflow_pack::workflow_clarify_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "workflow.research" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        workflow_pack::workflow_research_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "workflow.consult" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        workflow_pack::workflow_consult_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "workflow.ask" => {
                    dispatch_to_client(
                        server,
                        request_id,
                        workflow_pack::handle_workflow_ask(
                            server,
                            request.params.unwrap_or_default(),
                            &trace,
                        )
                        .await,
                    )
                    .await
                }
                "workflow.generate_from_chat" => {
                    dispatch_to_client(
                        server,
                        request_id,
                        workflow_pack::handle_workflow_generate_from_chat(
                            server,
                            request.params.unwrap_or_default(),
                            &trace,
                        )
                        .await,
                    )
                    .await
                }
                "workflow.generate" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        workflow_pack::workflow_generate_payload(
                            server,
                            request.params.unwrap_or_default(),
                            &trace,
                        )
                        .await,
                    )
                    .await
                }
                "workflow.execute" => {
                    dispatch_to_client(
                        server,
                        request_id,
                        handle_workflow_execute(server, request.params.unwrap_or_default(), &trace)
                            .await,
                    )
                    .await
                }
                "workflow.run.list" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        exec_pack::workflow_run_list_payload(&request.params.unwrap_or_default()),
                    )
                    .await
                }
                "workflow.run.get" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        exec_pack::workflow_run_get_payload(&request.params.unwrap_or_default()),
                    )
                    .await
                }
                "workflow.run.cancel" => {
                    let params = request.params.unwrap_or_default();
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        exec_pack::workflow::workflow_run_transition_payload(&params, "cancelled"),
                    )
                    .await
                }
                "workflow.run.pause" => {
                    let params = request.params.unwrap_or_default();
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        exec_pack::workflow::workflow_run_transition_payload(&params, "paused"),
                    )
                    .await
                }
                "workflow.run.resume" => {
                    let params = request.params.unwrap_or_default();
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        exec_pack::workflow::workflow_run_transition_payload(&params, "running"),
                    )
                    .await
                }
                "task.plan" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        workflow_pack::task_plan_payload(
                            server,
                            request.params.unwrap_or_default(),
                            &trace,
                        )
                        .await,
                    )
                    .await
                }
                "task.execute" => {
                    dispatch_to_client(
                        server,
                        request_id,
                        handle_task_execute(server, request.params.unwrap_or_default(), &trace)
                            .await,
                    )
                    .await
                }
                "learning.summary" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        learning_summary_payload(server, request.params.unwrap_or_default()).await,
                    )
                    .await
                }
                "learning.replay" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        learning_pack::learning_replay_payload(
                            server,
                            request.params.unwrap_or_default(),
                        ),
                    )
                    .await
                }
                "learning.guardrail" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        learning_pack::learning_guardrail_payload(
                            server,
                            request.params.unwrap_or_default(),
                        ),
                    )
                    .await
                }
                "knowledge.distill" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        learning_pack::knowledge_distill_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "rl.alignment.offline_eval" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        learning_pack::rl_alignment_offline_eval_payload(
                            server,
                            request.params.unwrap_or_default(),
                        ),
                    )
                    .await
                }
                "governance.status" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        governance_handlers::governance_status_payload(
                            server,
                            request.params.unwrap_or_default(),
                        ),
                    )
                    .await
                }
                "capabilities.list" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        lifecycle_handlers::capabilities_list_payload(server).await,
                    )
                    .await
                }
                "models.list" | "models/list" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        protocol_pack::models_list_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "governance.plan.get" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        governance_handlers::governance_plan_get_payload(server),
                    )
                    .await
                }
                "governance.plan.update" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        governance_handlers::governance_plan_update_payload(
                            server,
                            request.params.unwrap_or_default(),
                        ),
                    )
                    .await
                }
                "governance.audit.recent" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        governance_handlers::governance_audit_recent_payload(
                            server,
                            request.params.unwrap_or_default(),
                        ),
                    )
                    .await
                }
                "governance.audit.verify" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        governance_handlers::governance_audit_verify_payload(
                            server,
                            request.params.unwrap_or_default(),
                        ),
                    )
                    .await
                }
                "phase.policy.replay" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        learning_pack::phase_policy_replay_payload(
                            server,
                            request.params.unwrap_or_default(),
                        ),
                    )
                    .await
                }
                "primary_secondary.summary" | "summary/primary_secondary" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        learning_pack::primary_secondary_summary_payload(
                            server,
                            request.params.unwrap_or_default(),
                        ),
                    )
                    .await
                }
                "health.check" => match run_health_check(server).await {
                    Ok(_) => {
                        crate::acp::r#impl::io::respond(
                            server,
                            request_id,
                            Ok(json!({ "ok": true })),
                        )
                        .await
                    }
                    Err(e) => {
                        tracing::warn!("health.check: run_health_check failed: {}", e);
                        crate::acp::r#impl::io::respond(
                            server,
                            request_id,
                            Ok(json!({ "ok": false, "error": e.to_string() })),
                        )
                        .await
                    }
                },
                "governance.remediate" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        governance_handlers::governance_remediate_payload(
                            server,
                            request.params.unwrap_or_default(),
                        ),
                    )
                    .await
                }
                "governance.config.save" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        governance_handlers::governance_config_save_payload(
                            server,
                            request.params.unwrap_or_default(),
                        ),
                    )
                    .await
                }
                "provider.configure" => {
                    dispatch_to_client(
                        server,
                        request_id,
                        runtime_pack::handle_provider_configure(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "provider.test_connection" => {
                    let params = request.params.unwrap_or_default();
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        runtime_pack::provider_test_connection_payload(server, &params).await,
                    )
                    .await
                }
                "provider.test_completion" => {
                    let params = request.params.unwrap_or_default();
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        runtime_pack::provider_test_completion_payload(server, &params),
                    )
                    .await
                }
                "provider.copilot_device_code" => {
                    dispatch_to_client(
                        server,
                        request_id,
                        runtime_pack::handle_copilot_device_code_request(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "provider.copilot_device_code_poll" => {
                    dispatch_to_client(
                        server,
                        request_id,
                        runtime_pack::handle_copilot_device_code_poll(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "provider.capabilities" => {
                    let params = request.params.unwrap_or_default();
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        runtime_pack::provider_capabilities_payload(server, &params),
                    )
                    .await
                }
                "provider.catalog" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        runtime_pack::provider_catalog_payload(server),
                    )
                    .await
                }
                "provider.list_models" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        runtime_pack::provider_list_models_payload(
                            server,
                            request.params.unwrap_or_default(),
                        )
                        .await,
                    )
                    .await
                }
                "tool.approve" => {
                    let params = request.params.unwrap_or_default();
                    let tool_name = params
                        .get("tool_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if tool_name.is_empty() {
                        dispatch_to_client(
                            server,
                            request_id,
                            Err(anyhow::anyhow!(
                                "tool.approve: missing 'tool_name' parameter"
                            )),
                        )
                        .await
                    } else {
                        if let Some(ref harness_bus) = server.governance_deps.harness_bus {
                            harness_bus.evaluator.approve_tool(tool_name);
                            tracing::info!("tool.approve: user approved tool '{}'", tool_name);
                        }
                        crate::acp::r#impl::io::respond(
                            server,
                            request_id,
                            Ok(serde_json::json!({
                                "approved": true,
                                "tool_name": tool_name,
                            })),
                        )
                        .await
                    }
                }
                "runtime.restart" => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        lifecycle_handlers::runtime_restart_payload(server),
                    )
                    .await
                }
                _ => {
                    let localized = tf(
                        "error.request.unknown_method",
                        &[("method", &request.method)],
                    );
                    let descriptive = format!("unknown method: {}", request.method);
                    dispatch_to_client(
                        server,
                        request_id,
                        Ok(DispatchOutput::error(
                            AcpErrorCode::MethodNotFound as i32,
                            if localized.contains("unknown method")
                                || localized.contains("method not found")
                            {
                                localized
                            } else {
                                format!("{} ({})", descriptive, localized)
                            },
                        )),
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
        with_error_contract_data, LockHealthSummary,
    };
    #[cfg(not(feature = "backend-postgres"))]
    use crate::vector::VectorStore;
    #[cfg(not(feature = "backend-postgres"))]
    use std::sync::Arc;

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
        // Tool methods (registered in MethodRouter and ACP_METHODS list)
        assert!(is_acp_request("tools/list"));
        assert!(is_acp_request("tools/call"));
        // Terminal + approval methods live in the sorted ACP_METHODS list;
        // binary_search depends on the list staying alphabetically sorted, so
        // these were previously unreachable in ACP mode (see log 20260806-7).
        assert!(is_acp_request("terminal/create"));
        assert!(is_acp_request("terminal/kill"));
        assert!(is_acp_request("terminal/output"));
        assert!(is_acp_request("terminal/wait_for_exit"));
        assert!(is_acp_request("tool.approve"));
        // Unknown methods return false
        assert!(!is_acp_request("unknown.method"));
        assert!(!is_acp_request(""));
    }

    #[test]
    fn acp_methods_list_is_sorted_for_binary_search() {
        // The production `is_acp_request` uses `binary_search`, which silently
        // misses entries when the list is not alphabetically sorted (this made
        // `tool.approve` / `terminal/kill` unreachable in ACP mode — see
        // log 20260806-7 round-2 regression verification). Assert the invariant
        // against the real list to prevent silent regressions.
        let list = super::protocol::ACP_METHODS;
        let mut prev: Option<&str> = None;
        for entry in list {
            if let Some(p) = prev {
                assert!(
                    p < entry,
                    "ACP_METHODS out of order: {p:?} must sort before {entry:?}"
                );
            }
            prev = Some(entry);
        }
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
    #[tokio::test]
    async fn collect_vector_context_snippets_searches_execution_and_semantic_phase() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("request-vector-dual-phase.sqlite3");
        let store =
            Arc::new(VectorStore::new(&db_path, 64, 256).expect("vector store should initialize"));

        Arc::clone(&store)
            .upsert(
                "coding",
                "fix retrieval alignment",
                "semantic-phase knowledge",
            )
            .await
            .expect("semantic phase upsert should succeed");

        // No entries under execution phase key; this verifies we still retrieve
        // by semantic phase fallback and avoid false miss caused by key mismatch.
        let phases = vec!["phase-1".to_string(), "coding".to_string()];
        let snippets = collect_vector_context_snippets(
            Arc::clone(&store),
            &phases,
            "fix retrieval alignment",
            3,
        )
        .await;

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
            -32603_i32,
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
            LockHealthSummary {
                status: "warn",
                poisoned_total: 1,
                recovered_total: 1,
                slow_wait_total: 0,
                max_wait_ms: 1.2,
                components_tracked: 1,
            },
            LockHealthSummary {
                status: "healthy",
                poisoned_total: 0,
                recovered_total: 0,
                slow_wait_total: 0,
                max_wait_ms: 0.5,
                components_tracked: 1,
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
        let summary = summarize_lock_health(&[LockHealthSummary {
            status: "healthy",
            poisoned_total: 0,
            recovered_total: 0,
            slow_wait_total: 0,
            max_wait_ms: 0.5,
            components_tracked: 1,
        }]);
        assert_eq!(summary.status, "healthy");
        assert_eq!(summary.poisoned_total, 0);
    }
}
