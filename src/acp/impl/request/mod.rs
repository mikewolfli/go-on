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

use crate::acp::background::run_maintenance_cycle;

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
mod dispatch_content;
mod dispatch_learning;
mod dispatch_runtime;
mod dispatch_session;
mod dispatch_state;
mod dispatch_workflow;
pub(crate) mod exec_pack;
mod governance_handlers;
use self::governance_handlers::governance_pack;
mod hardness_pack;
mod health_pack;
mod learning_pack;
mod lifecycle_handlers;
mod lifecycle_pack;
mod mcp_client_pack;
mod metrics_pack;
pub mod prompts_pack;
pub(crate) mod protocol;
pub(crate) mod protocol_pack;
pub(crate) use self::trace_pack::{mark_error_response, tool_budget_trackers};
mod pua_pack;
mod repro_handlers;
pub(crate) use dispatch::{dispatch_to_client, DispatchOutput};
mod repro_pack;
mod runtime_pack;
mod status_pack;
pub(crate) mod tools_pack;
// `pub(crate)`: the trace sink is called directly by the chat session handler
// and the workflow pack (the former `request::record_trace_event` pass-through
// wrapper was removed as pure indirection).
pub(crate) mod trace_pack;
mod util;
pub(crate) mod workflow_pack;
use self::chat_pack::{parse_messages, send_error};
pub(crate) use self::checkpoint_pack::create_checkpoint_record;
pub(crate) use self::checkpoint_pack::persist_checkpoint_metacognitive_loop;
use self::checkpoint_pack::*;
use self::diagnostic_pack::*;
use self::exec_pack::*; // glob needed: tests.rs uses exec_pack items bare (e.g. infer_workflow_parallelism); dispatch_workflow.rs imports them directly
pub use self::governance_pack::build_knowledge_refinement_profile;
pub use self::governance_pack::build_learning_profile;
pub(crate) use self::governance_pack::inject_platform_profiles_if_absent;
use self::governance_pack::*;
use self::hardness_pack::*;
use self::learning_pack::*;
pub(crate) use self::protocol::is_acp_request;
use self::protocol::*;
pub use self::protocol_pack::record_tool_call_audit_with_protocol;
use self::protocol_pack::*;
use self::pua_pack::*;
use self::runtime_pack::*;
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

    // The `authenticate` method IS the credential handshake: it must reach
    // its handler so credentials can be presented. Gating it here would make
    // authentication impossible whenever user_auth_enabled is set (every
    // request — including authenticate itself — would be rejected for
    // missing credentials). The handler performs the real validation.
    if request.method == "authenticate" {
        return Ok(None);
    }

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
                AcpErrorCode::RateLimited as i32,
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
            //
            // Normalize bare MCP method names (`ping` -> `mcp.ping`,
            // `tools/list` -> `mcp.tools.list`, `notifications/initialized` ->
            // `mcp.notifications_initialized`) so standard MCP clients are
            // routed to the `mcp.*` handlers instead of falling into the
            // `_ =>` MethodNotFound branch. `initialize` is deliberately left
            // unnormalized: in Auto mode it keeps ACP semantics (the ACP
            // handshake), so an ACP client's `initialize` is not hijacked by
            // the MCP bridge. See `normalize_mcp_method` and the dual-stack
            // note in `src/protocol/access_mode.rs`.
            if is_mcp_request(method.as_ref()) && method.as_ref() != "initialize" {
                method = Cow::Owned(normalize_mcp_method(method.as_ref()));
            }
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
    // NOTE: task_context (task type / file count / risk score inference) is only
    // consumed by the PUA violation error paths and the stage-evidence branch
    // below, so it is built lazily inside those branches instead of on every
    // passing request (hot-path allocation + keyword scans).
    let build_task_context = || {
        let task_type = infer_task_type(method.as_ref(), &request.params);
        TaskContext {
            task_type: task_type.clone(),
            file_count: infer_file_count(&request.params),
            risk_score: infer_risk_score(method.as_ref(), &task_type),
        }
    };

    // NOTE: The former `pua_engine.check_red_lines(method)` guard was removed
    // — it was a dead check that could never fire. `check_red_lines` compares
    // the PUA plan's natural-language red lines ("Close the loop with
    // executable proof…") with `eq_ignore_ascii_case` against the ACP method
    // name ("task.execute"), which never matches, so it always passed while
    // giving the appearance of enforcement. The real PUA red-line enforcement
    // happens in the tool execution path: `execute_tool_call`
    // (src/acp/impl/request/tools_pack.rs:554) calls
    // `harness_bus.validate_action` → `PolicyEvaluator::check_tool_call`, which
    // substring-matches the configured `GovernancePolicy.red_lines` against the
    // serialized tool arguments.

    if let Some(stage) = infer_pua_stage(method.as_ref()) {
        let task_context = build_task_context();
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
        // Server is draining — reject new requests immediately. The
        // active-request counter was incremented above, so it must be
        // decremented here: this early return skips the accounting tail
        // (request.rs request-complete path).
        server.observability.metrics.dec_active_requests();
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

    self::trace_pack::record_trace_event(
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
    // Key for the $/cancel_request registry: same `value_to_id` mapping the
    // cancellation handler uses, so marks match the in-flight request id.
    let current_request_key = request_id
        .as_ref()
        .map(crate::rpc_protocol::value_to_id)
        .unwrap_or_else(|| "null".to_string());

    // Single dispatch table (the match below). The former registration-based
    // MethodRouter (B51-28) was merged back into this match: every handler it
    // registered was a thin forward to the same protocol_pack payload functions
    // used here, and its early `return` skipped the request-complete metrics /
    // trace tail, leaking active_requests. Merging restores a single dispatch
    // path with correct accounting.

    // Use the potentially normalized method for dispatch.
    let result = protocol_pack::ACP_CURRENT_REQUEST_ID
        .scope(
            Some(current_request_key.clone()),
            DISPATCH_REQUEST_METHOD.scope(method.to_string(), async {
                match method.as_ref() {
                    // ACP session lifecycle, MCP bridge, and terminal methods
                    "initialize"
                    | "session/new"
                    | "session/load"
                    | "session/prompt"
                    | "session/cancel"
                    | "session/list"
                    | "session/set_mode"
                    | "session/set_config_option"
                    | "session/resume"
                    | "session/close"
                    | "session/request_permission"
                    | "session/delete"
                    | "session/config/set"
                    | "session/config/get"
                    | "session/config/favorite/toggle"
                    | "authenticate"
                    | "logout"
                    | "$/cancel_request"
                    | "mcp.client.connect"
                    | "mcp.client.list"
                    | "mcp.client.call"
                    | "mcp.initialize"
                    | "mcp.notifications_initialized"
                    | "mcp.notifications_cancelled"
                    | "mcp.ping"
                    | "mcp.tools.list"
                    | "mcp.tools.call"
                    | "mcp.resources.list"
                    | "mcp.resources.read"
                    | "mcp.resources.subscribe"
                    | "mcp.logging.setLevel"
                    | "mcp.completion.complete"
                    | "mcp.sampling.createMessage"
                    | "mcp.prompts.list"
                    | "mcp.prompts.get"
                    | "terminal/create"
                    | "terminal/output"
                    | "terminal/release"
                    | "terminal/kill"
                    | "terminal/wait_for_exit" => {
                        dispatch_session::dispatch_session(
                            server,
                            request,
                            request_id,
                            http_headers,
                            &trace,
                            method.as_ref(),
                        )
                        .await
                    }
                    // Prompt, skill, and ACP tool methods
                    "prompts.list"
                    | "prompts.search"
                    | "prompts.get"
                    | "prompts.create"
                    | "prompts.update"
                    | "prompts.delete"
                    | "skill.import"
                    | "skill.enable"
                    | "skill.disable"
                    | "skill.list_imported"
                    | "skill.list"
                    | "skill.create"
                    | "skill.update"
                    | "skill.version.list"
                    | "skill.version.rollback"
                    | "skill.remove"
                    | "tools/list"
                    | "tools/call" => {
                        dispatch_content::dispatch_content(
                            server,
                            request,
                            request_id,
                            http_headers,
                            &trace,
                            method.as_ref(),
                        )
                        .await
                    }
                    // Chat/phase, metrics, health/lifecycle, observability,
                    // harness/breaker/cache maintenance, capabilities, models
                    "chat"
                    | "phase"
                    | "phase.status"
                    | "metrics.get"
                    | "metrics"
                    | "metrics.prometheus"
                    | "metrics.window.query"
                    | "metrics.errors.summary"
                    | "metrics.reset"
                    | "debug_panel.get"
                    | "debug.panel.get"
                    | "trace.get"
                    | "trace.metrics"
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
                    | "action.check"
                    | "approval.list"
                    | "capabilities.list"
                    | "models.list"
                    | "models/list"
                    | "runtime.restart" => {
                        dispatch_runtime::dispatch_runtime(
                            server,
                            request,
                            request_id,
                            http_headers,
                            &trace,
                            method.as_ref(),
                        )
                        .await
                    }
                    // Conversation checkpoints, config, reproducibility/autotune,
                    // and provider configuration
                    "conversation.checkpoint.create"
                    | "conversation.checkpoint.list"
                    | "checkpoint.list"
                    | "conversation.rollback"
                    | "conversation.checkpoint.prune"
                    | "config.reload"
                    | "config.baseline"
                    | "build.repro"
                    | "optimization.peak"
                    | "error.contract"
                    | "autotune.get"
                    | "autotune.status"
                    | "autotune.reset"
                    | "selector.status"
                    | "hardness.status"
                    | "cost.status"
                    | "provider.configure"
                    | "provider.test_connection"
                    | "provider.test_completion"
                    | "provider.copilot_device_code"
                    | "provider.copilot_device_code_poll"
                    | "provider.capabilities"
                    | "provider.catalog"
                    | "provider.list_models"
                    | "tool.approve" => {
                        dispatch_state::dispatch_state(
                            server,
                            request,
                            request_id,
                            http_headers,
                            &trace,
                            method.as_ref(),
                        )
                        .await
                    }
                    // Workflow lifecycle, task planning/execution, memory ingest
                    "workflow.confirm"
                    | "workflow.clarify"
                    | "workflow.research"
                    | "workflow.consult"
                    | "workflow.ask"
                    | "workflow.generate_from_chat"
                    | "workflow.generate"
                    | "workflow.execute"
                    | "workflow.run.list"
                    | "workflow.run.get"
                    | "workflow.run.cancel"
                    | "workflow.run.pause"
                    | "workflow.run.resume"
                    | "task.plan"
                    | "memory.ingest"
                    | "task.execute" => {
                        dispatch_workflow::dispatch_workflow(
                            server,
                            request,
                            request_id,
                            http_headers,
                            &trace,
                            method.as_ref(),
                        )
                        .await
                    }
                    // Learning/knowledge, governance, and health checks
                    "learning.summary"
                    | "learning.replay"
                    | "learning.guardrail"
                    | "knowledge.distill"
                    | "rl.alignment.offline_eval"
                    | "governance.status"
                    | "governance.plan.get"
                    | "governance.plan.update"
                    | "governance.audit.recent"
                    | "governance.audit.verify"
                    | "phase.policy.replay"
                    | "primary_secondary.summary"
                    | "summary/primary_secondary"
                    | "health.check"
                    | "governance.remediate"
                    | "governance.config.save" => {
                        dispatch_learning::dispatch_learning(
                            server,
                            request,
                            request_id,
                            http_headers,
                            &trace,
                            method.as_ref(),
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
            }),
        )
        .await
        .map_err(|error| attach_request_dispatch_context(error, method.as_ref()));

    // $/cancel_request lifecycle: the mark for this request id is consumed
    // once the request completes, so a later request reusing the same id is
    // not spuriously cancelled. (The registry is additionally bounded by
    // oldest-entry eviction in mark_acp_request_cancelled.)
    protocol_pack::clear_acp_request_cancelled(&current_request_key);

    let duration_ms = started.elapsed().as_millis() as u64;
    let success = result.is_ok() && !take_error_response_mark(&trace.request_id);
    let status = if success { "success" } else { "error" };
    server
        .observability
        .metrics
        .record_request_outcome(success, duration_ms as f64);
    server.observability.metrics.dec_active_requests();

    self::trace_pack::record_trace_event(
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
mod tests;
