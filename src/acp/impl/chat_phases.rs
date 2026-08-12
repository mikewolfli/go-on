//! Cognitive-loop phases for process_chat_request (BLUE62 ARCH-1)
//!
//! This module breaks the monolithic `process_chat_request` function into
//! four cognitive-loop phases, each <300 lines of orchestration logic. Internal
//! helpers within each phase handle deeper decomposition.
//!
//! Phase breakdown:
//!   1. `observe_phase` — Observe current state: input validation, multimodal detection,
//!      prompt injection check, context gathering, memory recall,
//!      capability sensing
//!   2. `think_phase`   — Think about the situation: model resolution, agent selection,
//!      routing, planning, capability analysis, risk assessment,
//!      metacognitive evaluation
//!   3. `act_phase`     — Execute actions: LLM calls, tool execution, autonomy loop,
//!      fallback, vote, cache operations, scheduler
//!   4. `reflect_phase` — Reflect on outcomes: response assembly, error handling,
//!      knowledge persistence, metacognitive updates, threshold learning,
//!      capability bus feedback, BrainLoop reflection

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Instant;

// ── observe_phase constants ─────────────────────────────────────────────
/// Timeout for the pre-fetch GET request (3s).
const PREFETCH_GET_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
/// Timeout for reading the pre-fetch response body (2s).
const PREFETCH_BODY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
/// Timeout for the SPA API probe POST (10s).
const SPA_API_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// Timeout for reading the SPA API probe response body (5s).
const SPA_API_BODY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// Pre-fetch body truncation limit (8192 bytes).
const PREFETCH_BODY_LIMIT: usize = 8192;
/// SPA API probe body truncation limit (4096 bytes).
const SPA_API_BODY_LIMIT: usize = 4096;

use anyhow::Result;
use futures_util::future::join_all;
use opentelemetry::Context as OtelContext;
use serde_json::{json, Value};
use tracing::{debug, info};

use crate::acp::helpers::autonomy_metrics::{
    record_cache_bypass_for_execution, record_cache_shortcircuit_refused,
};
use crate::acp::helpers::cache_strategy::{
    should_bypass_for_execution, CacheDecision, CacheStrategy,
};
use crate::acp::helpers::context::request_timeout;
use crate::acp::helpers::response_assembler::CapabilityRoutingInfo;
use crate::acp::helpers::review_gate::run_review_gate;
use crate::acp::helpers::vote_executor::{execute_high_risk_vote, HighRiskVoteExecutionResult};
use crate::acp::r#impl::chat::{
    agent_switch_state, apply_review_gate_assemble, auto_create_skills_from_conversation,
    auto_generate_workflow_from_conversation, emit_status_event, emit_stream_chunk,
    emit_stream_done, emit_stream_token_economy, estimate_token_economy,
    evaluate_pre_route_policies, execute_autonomy_round, execute_fallback_agents,
    extract_task_description, persist_chat_knowledge, persist_session_distillation,
    persist_vector_memory, resolve_request_phase, routing_handles, select_and_score_agents,
    AutonomyOutcome, ChatParams, ChatRequestContext, FallbackExecutionResult, RiskAssessment,
    RiskVotePolicy, StreamEventMeta, StreamObserver, VectorContext,
};

/// Classify whether the user's last message is a "simple chat" (e.g. "你好", "hello").
/// Simple messages skip expensive pre-processing steps (URL prefetch, multimodal,
/// memory recall, capability bus) and go directly to the LLM — same philosophy
/// as Codex's lean pre-processing pipeline.
///
/// A message is classified as "simple" when ALL conditions are met:
/// - The last user message is short (< 200 chars)
/// - Contains no HTTP/HTTPS URLs
/// - Contains no file:// or data: URIs
/// - Mode is "ask" or "chat" (not "edit"/"full_auto"/"safeguard")
fn is_simple_chat(params: &ChatParams) -> bool {
    // Mode check: only simple modes qualify
    let mode = params.mode.to_ascii_lowercase();
    if mode != "ask" && mode != "chat" && !mode.is_empty() {
        return false;
    }

    // Find the last user message
    let last_user = params.messages.iter().rev().find(|m| m.role == "user");
    let Some(last) = last_user else {
        return false;
    };

    let content = last.content.trim();

    // Short message only
    if content.len() > 200 {
        return false;
    }

    // No URLs
    if content.contains("http://") || content.contains("https://") {
        return false;
    }

    // No file:// or data: URIs
    if content.contains("file://") || content.contains("data:") {
        return false;
    }

    true
}
use crate::acp::r#impl::request;
use crate::acp::server::{AcpServer, OutcomeEvent};
use crate::agent::Message;
use crate::evaluation::TraceEvent;
use crate::i18n::runtime::tf;
use crate::intelligence::token_cache::{
    estimate_messages_token_count, messages_to_text, ContextLengthClass,
};

use crate::orchestration::flow::ResolvedPhase;
use crate::orchestration::mode::{resolve_mode_runtime, ModeKind};
use crate::orchestration::multi_agent_pipeline::MultiAgentPipeline;
use crate::rpc_protocol::{child_trace_context, RequestTraceContext};

// ═════════════════════════════════════════════════════════════════════
// Phase Result Types
// ═════════════════════════════════════════════════════════════════════

/// Collected output from the observe phase.
pub(crate) struct ObserveOutput {
    pub tenant_id: String,
    pub user_id: Option<String>,
    pub phase: ResolvedPhase,
    pub phase_name: String,
    pub phase_origin: String,
    pub resolved: crate::orchestration::flow::ResolvedRouting,
    pub schema_warnings: Vec<String>,
    pub schema_error: Option<String>,
    pub routing_provenance: Vec<String>,
    pub reputation_scores: HashMap<String, f64>,
    pub multimodal_context: Option<String>,
}

/// Collected output from the think phase.
pub(crate) struct ThinkOutput {
    pub capability_selected_agent: Option<String>,
    pub capability_recommended_mode: Option<String>,
    pub capability_candidate_count: Option<u64>,
    pub capability_decision_confidence: Option<f64>,
    pub capability_selection_reason: Option<String>,
    pub capability_optimization_hint: Option<Value>,
    pub configured_primary_agent: Option<String>,
    pub conversation_id: String,
    pub branch_id: String,
    pub agent_messages: Vec<Message>,
    pub layered_prompt_segments: usize,
    pub base_agent_options: HashMap<String, Value>,
    pub risk_policy: RiskVotePolicy,
    pub risk_assessment: RiskAssessment,
    pub enable_high_risk_multi_agent_vote: bool,
    pub min_vote_agents: usize,
    pub max_vote_agents: usize,
    pub escalation_enabled: bool,
    pub escalation_models_per_agent: usize,
    pub escalation_max_agents: usize,
    pub unhealthy_fallback_agent: Option<String>,
    pub fallback_reason: Option<String>,
    pub council_decision: Option<Value>,
    pub candidate_agents: Vec<String>,
    pub vector_context: VectorContext,
}

/// Collected output from the act phase.
pub(crate) struct ActOutput {
    pub selected_agent: String,
    pub response_text: String,
    pub reasoning_text: String,
    pub selected_model_name: Option<String>,
    pub last_err: Option<anyhow::Error>,
    pub cache_hit: bool,
    pub cache_bypassed_for_execution: bool,
    pub agent_attempts: Vec<Value>,
    pub quota_failed_agents: Vec<String>,
    pub vote_winner: Option<String>,
    pub vote_report: Option<Value>,
    pub used_multi_model_vote: bool,
    pub used_multi_agent_vote: bool,
    pub review_required: bool,
    pub review_blocked: bool,
    pub checkpoint: crate::acp::ConversationCheckpoint,
    pub knowledge: Value,
    pub metacognitive_loop: Value,
    pub distillation: Value,
    /// True when tools were requested but ALL of them failed.
    pub all_tools_failed: bool,
}

// ═════════════════════════════════════════════════════════════════════
// Phase 1: Observe
// ═════════════════════════════════════════════════════════════════════

/// Phase 1: Observe current state: input validation, multimodal detection,
/// prompt injection check, context gathering, memory recall, capability sensing.
pub(crate) async fn observe_phase(
    server: &AcpServer,
    params: &mut ChatParams,
    ctx: ChatRequestContext,
    stream_observer: Option<&StreamObserver>,
) -> Result<ObserveOutput> {
    let (flow, registry) = routing_handles(server)?;
    let tenant_id = ctx.tenant_id.clone();
    let user_id = ctx.user_id.clone();

    evaluate_pre_route_policies(server, params, &tenant_id).await?;

    // ── Sub-step 1: Security check — prompt injection detection ────
    emit_status_event(
        stream_observer,
        "Checking for prompt injection, safety violations...",
        "analyzing",
    )
    .await?;

    // ── Prompt injection detection & enforcement ──────────────────
    //
    // Security: Injection is detected per-message with escalating action:
    //   1. Block (HIGH/CRITICAL across ANY message) — reject the request
    //   2. Sanitize (MEDIUM/LOW) — replace injection spans with inert markers
    //   3. Log only — contaminations are recorded for audit
    //
    // This is the pipeline's per-message sanitizing pass, distinct from the
    // pre-pipeline escalation gate in `evaluate_pre_chat_gates` (chat.rs),
    // which scans the raw full history for ANY violation to escalate approval.
    // The inputs differ (pre-trim full history vs this compressed/trimmed
    // working set) and the severity policies differ (any-violation vs High+
    // block) — deliberate defense-in-depth, not a duplicate scan
    // (debt #12 verdict: keep).
    if let Some(ref detector) = server.governance_deps.injection_detector {
        use crate::security::severity::DetectionSeverity as InjectionSeverity;

        // Detect and classify severity across ALL messages first.
        // detect_and_sanitize runs ONE detect() pass per message and returns
        // the sanitized form; re-detecting in the enforcement loop below
        // would scan every message twice.
        let mut has_high_or_critical = false;
        let mut all_violations: Vec<crate::security::prompt_injection::SafetyViolation> =
            Vec::new();
        let mut max_contamination = 0.0_f64;
        let mut sanitized_messages: Vec<(bool, String)> = Vec::with_capacity(params.messages.len());

        for msg in &params.messages {
            let (sanitized, result) = detector.detect_and_sanitize(&msg.content);
            sanitized_messages.push((result.detected, sanitized));
            if result.detected {
                all_violations.extend(result.violations.clone());
                max_contamination = max_contamination.max(result.contamination_score);
                if detector.should_block(&result, InjectionSeverity::High) {
                    has_high_or_critical = true;
                }
            }
        }

        if !all_violations.is_empty() {
            info!(
                injection_warnings = ?all_violations,
                contamination_score = max_contamination,
                "prompt injection detected — taking action"
            );

            // Always record to the canonical audit sink (chained by the sink's
            // writer thread — tamper-evident by construction).
            crate::governance::audit::global_audit_log().record(
                crate::governance::audit::AuditLogEntry {
                    timestamp: crate::governance::audit::chrono_now(),
                    task_id: ctx.tenant_id.clone(),
                    phase: "security".to_string(),
                    agent: None,
                    tool: None,
                    decision: "prompt_injection_enforced".to_string(),
                    inputs: json!({
                        "warnings": &all_violations,
                        "contamination_score": max_contamination,
                    }),
                    outputs: None,
                    error: None,
                    confidence: None,
                    data_classification: None,
                    compliance_tags: vec![],
                    retention_policy: None,
                    correlation_id: None,
                },
            );

            // ── HIGH/CRITICAL: block the request ───────────────────────
            if has_high_or_critical {
                let critical: Vec<String> = all_violations
                    .iter()
                    .filter(|v| v.base.severity >= InjectionSeverity::High)
                    .map(|v| format!("{:?}: {}", v.category, v.base.description))
                    .collect();
                anyhow::bail!(
                    "Request blocked: prompt injection detected. Violations: {}",
                    critical.join("; ")
                );
            }

            // ── MEDIUM/LOW: sanitize each message (apply the sanitized form
            // computed during the single detect pass above) ────────────
            for (msg, (was_detected, sanitized)) in
                params.messages.iter_mut().zip(sanitized_messages.iter())
            {
                if *was_detected {
                    msg.content = sanitized.clone();
                }
            }
        }
    }

    // ── Sub-steps 2+3: Multimodal input detection ∥ URL pre-fetching ──
    // Codex-style: skip for simple chat — no files/URIs to process. The two
    // expensive sub-steps run concurrently (multimodal only reads the
    // messages; URL processing collects inserts applied after the join).
    let is_simple = is_simple_chat(params);

    // Cache-gated skip: when the semantic cache is guaranteed to serve this
    // request (probe on the last user message; the gate conditions are
    // deterministic and identical to act_phase's — see
    // `semantic_prefetch_should_skip`), the fetched URL content / multimodal
    // context would never be consumed, so both expensive sub-steps are pure
    // waste. Skip them.
    let skip_expensive = semantic_prefetch_should_skip(server, params);
    if skip_expensive {
        tracing::debug!(
            target = "chat_pipeline",
            "observe_phase: semantic cache hit — skipping multimodal + URL prefetch"
        );
    }

    // URL scan first (pure read of the LAST user message — avoids re-fetching
    // URLs from conversation history on every turn) so both branches below can
    // run concurrently.
    let url_entries: Vec<(usize, String)> = if skip_expensive {
        Vec::new()
    } else {
        params
            .messages
            .iter()
            .enumerate()
            .rev()
            .take(1)
            .filter(|(_, msg)| msg.role == "user")
            .filter_map(|(i, msg)| {
                crate::orchestration::tool_extended::http::extract_url(&msg.content)
                    .map(|u| (i, u.to_string()))
            })
            .collect()
    };

    // Status events are emitted before the join so `?` error propagation is
    // preserved; the heavy work below then runs concurrently.
    if !is_simple && !skip_expensive {
        emit_status_event(
            stream_observer,
            "Processing multimodal input (images, files, audio)...",
            "analyzing",
        )
        .await?;
    }
    if !skip_expensive && !url_entries.is_empty() {
        emit_status_event(stream_observer, "Pre-fetching URLs...", "analyzing").await?;
    }

    let (multimodal_context, url_inserts) = tokio::join!(
        async {
            if is_simple || skip_expensive {
                None
            } else {
                detect_and_process_multimodal(server, params).await
            }
        },
        async {
            if skip_expensive || url_entries.is_empty() {
                Vec::new()
            } else {
                // Phase 1: Fetch all URLs in parallel
                let fetch_futures: Vec<_> = url_entries
                    .iter()
                    .map(|(msg_idx, url)| {
                        let host_owned = url::Url::parse(url)
                            .ok()
                            .and_then(|u| u.host_str().map(str::to_string));
                        let url_owned = url.clone();
                        let msg_idx = *msg_idx;
                        async move {
                            // Skip pre-fetch for local/private URLs. Uses the same
                            // private-host definition as the http_request tool
                            // (is_private_host covers loopback, private ranges,
                            // link-local, multicast, unspecified, and IPv6); the
                            // hostname DNS check runs on the blocking pool with a
                            // bounded timeout (is_private_host_async) so a slow
                            // resolver cannot stall this worker.
                            let is_private = match &host_owned {
                                Some(host) => crate::orchestration::tool_extended::http::
                                    is_private_host_async(host)
                                    .await,
                                None => false,
                            };
                            if is_private {
                                tracing::info!(
                                    "observe_phase: skipping pre-fetch for local/private URL: {}",
                                    url_owned
                                );
                                return None;
                            }

                            let fetch_url = url_owned
                                .split('#')
                                .next()
                                .unwrap_or(&url_owned)
                                .to_string();
                            tracing::info!(
                                "observe_phase: auto-detected URL, pre-fetching: {}",
                                url_owned
                            );

                            let result = match tokio::time::timeout(
                                PREFETCH_GET_TIMEOUT,
                                crate::shared::http_client::http_client()
                                    .expect("shared HTTP client must build")
                                    .get(&fetch_url)
                                    .send(),
                            )
                            .await
                            {
                                Ok(Ok(resp)) => {
                                    let status = resp.status().to_string();
                                    // Bound the body read like http_request:
                                    // stream at most PREFETCH_BODY_LIMIT (+one
                                    // chunk) so a huge response is never fully
                                    // buffered — the consumer truncates to
                                    // PREFETCH_BODY_LIMIT anyway. Previously
                                    // `resp.text()` buffered the whole body
                                    // before truncating.
                                    let body_bytes =
                                        tokio::time::timeout(PREFETCH_BODY_TIMEOUT, async {
                                            use futures_util::StreamExt;
                                            let mut body =
                                                Vec::with_capacity(PREFETCH_BODY_LIMIT.min(8192));
                                            let mut stream = resp.bytes_stream();
                                            while body.len() <= PREFETCH_BODY_LIMIT {
                                                match stream.next().await {
                                                    Some(Ok(chunk)) => {
                                                        body.extend_from_slice(&chunk);
                                                    }
                                                    Some(Err(e)) => {
                                                        tracing::debug!(
                                                            "observe_phase: pre-fetch body read error for {}: {}",
                                                            url_owned,
                                                            e
                                                        );
                                                        return None;
                                                    }
                                                    None => break,
                                                }
                                            }
                                            Some(body)
                                        })
                                        .await
                                        .ok()
                                        .flatten();
                                    body_bytes.map(|bytes| {
                                        let body = String::from_utf8_lossy(&bytes).into_owned();
                                        (status, body)
                                    })
                                }
                                _ => None,
                            };
                            Some((msg_idx, url_owned, fetch_url, result))
                        }
                    })
                    .collect();

                let fetch_results = join_all(fetch_futures).await;

                // Phase 2: Process each result sequentially (SPA detection, API probing,
                // message building). Inserts are collected and applied by the caller
                // (after the join) so this block can run concurrently with multimodal
                // detection.
                let mut url_inserts: Vec<(usize, Message)> = Vec::new();
                for item in fetch_results.into_iter().flatten() {
                    let (msg_idx, url, fetch_url, fetch_result) = item;
                    if let Some((status, body)) = fetch_result {
                        let truncated = if body.len() > PREFETCH_BODY_LIMIT {
                            format!(
                                "{}...\n[Response truncated at {} bytes]",
                                crate::shared::truncate::truncate_chars(
                                    &body,
                                    PREFETCH_BODY_LIMIT,
                                    ""
                                ),
                                PREFETCH_BODY_LIMIT
                            )
                        } else {
                            body.clone()
                        };
                        let mut context_msg = format!(
                            "[Auto-fetched content from {}]\nHTTP Status: {}\n\n{}",
                            url, status, truncated
                        );

                        // Phase 2: Detect SPA and probe API endpoints
                        let is_spa = body.contains("<div id=\"root\"")
                            || body.contains("<div id=\"app\"")
                            || (body.contains("<script")
                                && body.chars().filter(|c| *c == '<').count() > 20
                                && body.len() > 200
                                && body.len() < 5000);
                        if is_spa {
                            tracing::info!(
                                "observe_phase: detected SPA page, probing API: {}",
                                url
                            );

                            // Extract fragment params
                            let fragment_params: Vec<(String, String)> = url
                                .split('#')
                                .nth(1)
                                .map(|f| {
                                    url::form_urlencoded::parse(f.as_bytes())
                                        .map(|(k, v)| (k.to_string(), v.to_string()))
                                        .collect()
                                })
                                .unwrap_or_default();

                            let path_segments: Vec<&str> = url
                                .split('#')
                                .next()
                                .unwrap_or(&url)
                                .split('/')
                                .filter(|s| !s.is_empty())
                                .collect();

                            let mut spa_info = format!(
                                "\n\n[SPA Page Analysis]\n\
                         The URL returned a JavaScript SPA shell.\n\
                         Path segments: {}\nFragment params: {:?}",
                                path_segments.join(" / "),
                                fragment_params,
                            );

                            // Try common API pattern: POST /api/v1/agent-binding/invitations/{id}/agent-task
                            if let Some(invitation_id) = path_segments.last().filter(|s| {
                                s.starts_with("invite_") || s.starts_with("invitation_")
                            }) {
                                let token = fragment_params
                                    .iter()
                                    .find(|(k, _)| k == "task_access_token")
                                    .map(|(_, v)| v.clone());

                                if let Some(token_val) = token {
                                    let host = url.split('/').nth(2).unwrap_or("");
                                    let scheme = if fetch_url.starts_with("https") {
                                        "https"
                                    } else {
                                        "http"
                                    };
                                    let api_url = format!(
                                        "{}://{}/api/v1/agent-binding/invitations/{}/agent-task",
                                        scheme, host, invitation_id,
                                    );
                                    let web_origin = format!("{}://{}", scheme, host);
                                    let api_body = serde_json::json!({
                                        "task_access_token": token_val,
                                        "web_origin": web_origin,
                                    });

                                    match tokio::time::timeout(
                                        SPA_API_PROBE_TIMEOUT,
                                        crate::shared::http_client::http_client()
                                            .expect("shared HTTP client must build")
                                            .post(&api_url)
                                            .header("Content-Type", "application/json")
                                            .json(&api_body)
                                            .send(),
                                    )
                                    .await
                                    {
                                        Ok(Ok(api_resp)) => {
                                            let api_status = api_resp.status();
                                            match tokio::time::timeout(
                                                SPA_API_BODY_TIMEOUT,
                                                api_resp.text(),
                                            )
                                            .await
                                            {
                                                Ok(Ok(api_body_text)) => {
                                                    let t = if api_body_text.len()
                                                        > SPA_API_BODY_LIMIT
                                                    {
                                                        format!(
                                                            "{}...\n[truncated]",
                                                            crate::shared::truncate::truncate_chars(
                                                                &api_body_text,
                                                                SPA_API_BODY_LIMIT,
                                                                ""
                                                            )
                                                        )
                                                    } else {
                                                        api_body_text.clone()
                                                    };
                                                    spa_info.push_str(&format!(
                                                "\n\n[API: POST {}]\nStatus: {}\nRequest: {}\nResponse:\n{}",
                                                api_url, api_status, api_body, t,
                                            ));

                                                    // ── Phase 3: Present raw task data to AI for planning ──────
                                                    // The AI receives the full task package and plans the workflow
                                                    // itself using general PUA principles (FETCH, ANALYZE, EXTRACT,
                                                    // CHAIN, RES, ERR). No task-specific instructions here.
                                                    match serde_json::from_str::<Value>(&api_body_text) {
                                                Ok(task_json) => {
                                                    let ok_val = task_json
                                                        .get("ok")
                                                        .and_then(|v| v.as_bool())
                                                        .unwrap_or(false);
                                                    if ok_val {
                                                        if let Some(data) = task_json
                                                            .get("data")
                                                            .and_then(|v| v.as_object())
                                                        {
                                                            let data_json = serde_json::to_string_pretty(data)
                                                                .unwrap_or_default();
                                                            spa_info.push_str(&format!(
                                                                "\n\n[Agent World Task Package - pre-fetched by system]\n{}",
                                                                data_json,
                                                            ));
                                                        } else {
                                                            spa_info.push_str(
                                                                "\n\n[Agent World] No `data` object in task response",
                                                            );
                                                        }
                                                    } else {
                                                        spa_info.push_str(&format!(
                                                            "\n\n[Agent World] Task package returned ok=false: {}",
                                                            task_json,
                                                        ));
                                                    }
                                                }
                                                Err(e) => spa_info.push_str(&format!(
                                                    "\n\n[Agent World] Failed to parse task package JSON: {}",
                                                    e,
                                                )),
                                            }
                                                }
                                                _ => spa_info.push_str(&format!(
                                                    "\n\n[API: POST {}] - read failed",
                                                    api_url
                                                )),
                                            }
                                        }
                                        Ok(Err(e)) => spa_info.push_str(&format!(
                                            "\n\n[API: POST {}] - failed: {}",
                                            api_url, e
                                        )),
                                        Err(_) => spa_info.push_str(&format!(
                                            "\n\n[API: POST {}] - timeout",
                                            api_url
                                        )),
                                    }
                                }
                            }
                            context_msg.push_str(&spa_info);
                        }

                        url_inserts.push((
                            msg_idx,
                            Message {
                                role: "system".to_string(),
                                content: context_msg,
                            },
                        ));
                    } else {
                        tracing::warn!("observe_phase: failed to fetch URL: {}", url);
                    }
                }

                url_inserts
            }
        },
    );

    // Apply the fetched-content system messages (insertion order preserved —
    // identical to the previous inline insertion loop).
    for (msg_idx, msg) in url_inserts {
        params.messages.insert(msg_idx, msg);
    }

    // ── HarnessBus during-execute checkpoint ───────────────────────
    if let Some(ref harness) = server.governance_deps.harness_bus {
        let verdict = harness
            .validate_action("chat.execute", &serde_json::Value::Null)
            .await;
        if !verdict.is_allowed() {
            anyhow::bail!(
                "harness_bus during-execute denied: sandbox={} budget={} permitted={}",
                verdict.allowed,
                verdict.budget_ok,
                verdict.permitted
            );
        }
    }

    // ── Phase resolution ──────────────────────────────────────────
    let phase_res = resolve_request_phase(server, params, &flow, registry.as_ref()).await?;

    // flow and registry are kept alive via server reference
    drop(flow);
    drop(registry);

    Ok(ObserveOutput {
        tenant_id,
        user_id,
        phase: phase_res.phase,
        phase_name: phase_res.phase_name,
        phase_origin: phase_res.phase_origin,
        resolved: phase_res.resolved,
        schema_warnings: phase_res.schema_warnings,
        schema_error: phase_res.schema_error,
        routing_provenance: phase_res.routing_provenance,
        reputation_scores: phase_res.reputation_scores,
        multimodal_context,
    })
}

/// Whether the expensive observe sub-steps (multimodal + URL prefetch) can be
/// safely skipped because the semantic cache will serve this request.
///
/// Mirrors act_phase's semantic-cache gate exactly, so the decision is
/// deterministic across both points:
/// - the semantic key is the LAST user message — unchanged by observe/think,
///   so the probe at observe time sees the same key act_phase will use;
/// - the execution-like bypass scan runs on USER messages only (see
///   [`should_bypass_for_execution`] — system/metadata injections are context,
///   not intent), so `params.messages` and the think-phase `agent_messages`
///   yield the same decision;
/// - the duplicate check is user-message based
///   (`last_user_message_is_duplicate`).
///
/// The only divergence from act_phase is a rare race: a background purge may
/// remove the entry between the probe and act's lookup, in which case act
/// falls through to a fresh agent run WITHOUT the prefetched URL context — a
/// graceful, bounded degradation (the URL text is still visible to the model).
fn semantic_prefetch_should_skip(server: &AcpServer, params: &ChatParams) -> bool {
    let semantic_hit = match params
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.as_str())
    {
        Some(key) => try_semantic_cache(server, key).is_some(),
        None => false,
    };
    semantic_prefetch_should_skip_condition(&params.mode, &params.messages, semantic_hit)
}

/// Pure gate for [`semantic_prefetch_should_skip`] (testable without a server):
/// skip only when the semantic cache hit is guaranteed AND neither the
/// execution-like bypass nor the duplicate-user bypass applies.
fn semantic_prefetch_should_skip_condition(
    mode: &str,
    messages: &[Message],
    semantic_hit: bool,
) -> bool {
    // A user message must exist (the semantic key is the last user message).
    messages.iter().any(|m| m.role == "user")
        && semantic_hit
        && !should_bypass_for_execution(mode, messages)
        && !crate::intelligence::token_cache::last_user_message_is_duplicate(messages)
}

/// Detect and process multimodal input (repo queries, data: URIs, file:// refs).
async fn detect_and_process_multimodal(server: &AcpServer, params: &ChatParams) -> Option<String> {
    let mp = server.multimodal_processor.as_ref()?;
    use crate::multimodal::MultimodalInput;
    let mut contexts: Vec<String> = Vec::new();
    for msg in &params.messages {
        if msg.role != "user" {
            continue;
        }
        let content = msg.content.trim();
        if mp.repo_analyzer.is_some() && content.starts_with(crate::multimodal::REPO_PREFIX) {
            let processed = mp
                .process_input(&MultimodalInput::Text(content.to_owned()))
                .await;
            if !processed.is_empty() && processed.text != content {
                contexts.push(format!("[Repository analysis result]:\n{}", processed.text));
            }
            continue;
        }
        // Extract every inline `data:` URI — the GUI appends one URI per
        // attachment (F-GAP-66). The scanner also preserves the original
        // single-URI semantics (a message that is exactly one data URI).
        extract_data_uris(mp, content, &mut contexts).await;
        if content.starts_with("file://") {
            if let Some(c) = process_file_uri(mp, content).await {
                contexts.push(c);
            }
            continue;
        }
    }
    if contexts.is_empty() {
        None
    } else {
        Some(contexts.join("\n---\n"))
    }
}

/// Extract and process every inline `data:<mime>;base64,<payload>` URI in a
/// user message. A base64 payload runs until the first character outside the
/// base64 alphabet (whitespace, `)`, newline, etc.), so multiple URIs in one
/// message are handled independently.
async fn extract_data_uris(
    mp: &crate::multimodal::MultimodalProcessor,
    content: &str,
    contexts: &mut Vec<String>,
) {
    use crate::multimodal::MultimodalInput;
    // First pass: locate and decode every inline `data:` URI. The base64
    // payload runs until the first character outside the base64 alphabet
    // (whitespace, `)`, newline, etc.), so multiple URIs in one message are
    // handled independently.
    let mut inputs: Vec<MultimodalInput> = Vec::new();
    let mut rest = content;
    while let Some(start) = rest.find("data:") {
        let after = &rest[start + 5..];
        let Some((mime, payload_tail)) = after.split_once(";base64,") else {
            break;
        };
        let payload_end = payload_tail
            .find(|c: char| !matches!(c, 'A'..='Z' | 'a'..='z' | '0'..='9' | '+' | '/' | '='))
            .unwrap_or(payload_tail.len());
        let b64 = &payload_tail[..payload_end];
        // Advance past this URI before processing (process_input may await).
        rest = &payload_tail[payload_end..];
        let Ok(bytes) = crate::multimodal::base64_decode(b64) else {
            tracing::debug!(len = b64.len(), "skipping undecodable inline data URI");
            continue;
        };
        if bytes.is_empty() {
            continue;
        }
        let ml = mime.to_lowercase();
        // Enforce the declared per-modality size limits (previously the
        // MAX_IMAGE_SIZE / MAX_AUDIO_SIZE constants were never applied, so a
        // hostile message could push an arbitrarily large payload into the
        // vision/ASR pipeline).
        if (ml.contains("image") && bytes.len() > crate::multimodal::MAX_IMAGE_SIZE)
            || (ml.contains("audio") && bytes.len() > crate::multimodal::MAX_AUDIO_SIZE)
        {
            tracing::warn!(
                mime = %ml,
                len = bytes.len(),
                "skipping inline data URI exceeding the declared modality size limit"
            );
            continue;
        }
        let input = if ml.contains("image") {
            MultimodalInput::Image(bytes)
        } else if ml.contains("audio") {
            MultimodalInput::Audio(bytes)
        } else if ml.contains("video") {
            MultimodalInput::Video(bytes)
        } else {
            MultimodalInput::Document(bytes, crate::multimodal::mime_to_extension(&ml))
        };
        inputs.push(input);
    }

    // Second pass: process all URIs concurrently (image/audio/document
    // decoding is independent per URI). join_all preserves input order, so the
    // resulting contexts keep the original document order.
    let processed_all = join_all(
        inputs
            .into_iter()
            .map(|input| async move { mp.process_input(&input).await }),
    )
    .await;

    for processed in processed_all {
        if !processed.is_empty() {
            let mut e = String::from("[Processed content]:");
            if !processed.text.is_empty() {
                e.push('\n');
                e.push_str(&processed.text);
            }
            for (i, img) in processed.images.iter().enumerate() {
                e.push_str(&format!(
                    "\n![extracted-image-{}](data:image/unknown;base64,{})",
                    i, img
                ));
            }
            if !processed.audio_transcriptions.is_empty() {
                e.push_str(&format!(
                    "\n[Audio transcription]:\\n{}",
                    processed.joined_audio()
                ));
            }
            contexts.push(e);
        }
    }
}

async fn process_file_uri(
    mp: &crate::multimodal::MultimodalProcessor,
    content: &str,
) -> Option<String> {
    use crate::multimodal::MultimodalInput;
    let file_path = content.strip_prefix("file://")?;
    let path = std::path::Path::new(file_path);
    let ext = path.extension().and_then(|e| e.to_str())?;
    let bytes = tokio::fs::read(path).await.ok()?;
    let ext_lower = ext.to_lowercase();
    // Enforce the declared per-modality size limits (same guard as the
    // inline `data:` URI path) so a model-picked huge file cannot push an
    // unbounded payload into the vision/ASR pipeline.
    let is_image = matches!(
        ext_lower.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
    );
    let is_audio = matches!(ext_lower.as_str(), "mp3" | "wav" | "flac" | "ogg" | "m4a");
    if (is_image && bytes.len() > crate::multimodal::MAX_IMAGE_SIZE)
        || (is_audio && bytes.len() > crate::multimodal::MAX_AUDIO_SIZE)
    {
        tracing::warn!(
            file = %file_path,
            len = bytes.len(),
            "skipping file:// URI exceeding the declared modality size limit"
        );
        return None;
    }
    let input = match ext_lower.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => MultimodalInput::Image(bytes),
        "mp3" | "wav" | "flac" | "ogg" | "m4a" => MultimodalInput::Audio(bytes),
        "mp4" | "avi" | "mkv" | "mov" | "webm" => MultimodalInput::Video(bytes),
        _ => MultimodalInput::Document(bytes, ext_lower),
    };
    let processed = mp.process_input(&input).await;
    if !processed.is_empty() {
        let mut e = format!("[Processed file content]: file={}", file_path);
        if !processed.text.is_empty() {
            e.push('\n');
            e.push_str(&processed.text);
        }
        for (i, img) in processed.images.iter().enumerate() {
            e.push_str(&format!(
                "\n![extracted-image-{}](data:image/unknown;base64,{})",
                i, img
            ));
        }
        if !processed.audio_transcriptions.is_empty() {
            e.push_str(&format!(
                "\n[Audio transcription]:\\n{}",
                processed.joined_audio()
            ));
        }
        return Some(e);
    }
    None
}

// ═════════════════════════════════════════════════════════════════════
// Phase 2: Think
// ═════════════════════════════════════════════════════════════════════

/// Phase 2: Think about the situation: model resolution, agent selection,
/// routing, planning, capability analysis, risk assessment, metacognitive evaluation.
pub(crate) async fn think_phase(
    server: &AcpServer,
    params: &ChatParams,
    resolve_out: &mut ObserveOutput,
    trace: &RequestTraceContext,
) -> Result<ThinkOutput> {
    let agent_sel = select_and_score_agents(
        server,
        params,
        &mut resolve_out.resolved,
        &resolve_out.phase,
        &resolve_out.phase_name,
        &resolve_out.tenant_id,
        trace,
        &mut resolve_out.routing_provenance,
        &resolve_out.reputation_scores,
    )
    .await?;

    let mut agent_messages = agent_sel.agent_messages;

    // Inject multimodal context
    if let Some(ctx_text) = &resolve_out.multimodal_context {
        agent_messages.insert(
            0,
            Message {
                role: "system".to_string(),
                content: ctx_text.clone(),
            },
        );
    }

    // AgentMemoryBus — inject relevant memories into context
    // Codex-style: skip for simple chat — no task context to recall
    let is_simple = is_simple_chat(params);
    if !is_simple {
        inject_agent_memory_bus(
            server,
            resolve_out.user_id.as_deref(),
            &resolve_out.phase_name,
            agent_sel.capability_selected_agent.as_deref(),
            &params.messages,
            &mut agent_messages,
        )
        .await;
    }

    Ok(ThinkOutput {
        capability_selected_agent: agent_sel.capability_selected_agent,
        capability_recommended_mode: agent_sel.capability_recommended_mode,
        capability_candidate_count: agent_sel.capability_candidate_count,
        capability_decision_confidence: agent_sel.capability_decision_confidence,
        capability_selection_reason: agent_sel.capability_selection_reason,
        capability_optimization_hint: agent_sel.capability_optimization_hint,
        configured_primary_agent: agent_sel.configured_primary_agent,
        conversation_id: agent_sel.conversation_id,
        branch_id: agent_sel.branch_id,
        agent_messages,
        layered_prompt_segments: agent_sel.layered_prompt_segments,
        base_agent_options: agent_sel.base_agent_options,
        risk_policy: agent_sel.risk_policy,
        risk_assessment: agent_sel.risk_assessment,
        enable_high_risk_multi_agent_vote: agent_sel.enable_high_risk_multi_agent_vote,
        min_vote_agents: agent_sel.min_vote_agents,
        max_vote_agents: agent_sel.max_vote_agents,
        escalation_enabled: agent_sel.escalation_enabled,
        escalation_models_per_agent: agent_sel.escalation_models_per_agent,
        escalation_max_agents: agent_sel.escalation_max_agents,
        unhealthy_fallback_agent: agent_sel.unhealthy_fallback_agent,
        fallback_reason: agent_sel.fallback_reason,
        council_decision: agent_sel.council_decision,
        candidate_agents: agent_sel.candidate_agents,
        vector_context: agent_sel.vector_context,
    })
}

async fn inject_agent_memory_bus(
    _server: &AcpServer,
    user_id: Option<&str>,
    phase_name: &str,
    agent_name: Option<&str>,
    messages: &[Message],
    agent_messages: &mut Vec<Message>,
) {
    use crate::memory::agent_memory_bus::{AgentMemoryBus, AGENT_MEMORY_BUS};
    if let Some(memory_ctx) = AGENT_MEMORY_BUS
        .get_or_init(AgentMemoryBus::new_default)
        .retrieve_context_for_agent(
            agent_name.unwrap_or("unknown"),
            phase_name,
            &extract_task_description(messages),
            5,
            user_id,
        )
        .await
    {
        agent_messages.insert(
            0,
            Message {
                role: "system".to_string(),
                content: format!("[AgentMemoryBus context]\n{}", memory_ctx),
            },
        );
    }
}

// ═════════════════════════════════════════════════════════════════════
// Phase 3: Act
// ═════════════════════════════════════════════════════════════════════

/// Phase 3: Execute actions: LLM calls, tool execution, autonomy loop,
/// fallback, vote, cache operations, scheduler.
pub(crate) async fn act_phase(
    server: &AcpServer,
    params: &ChatParams,
    trace: &RequestTraceContext,
    stream_observer: Option<StreamObserver>,
    started: Instant,
    resolve_out: &mut ObserveOutput,
    routing_out: &ThinkOutput,
) -> Result<ActOutput> {
    let mut selected_agent = String::new();
    let mut response_text = String::new();
    let mut reasoning_text = String::new();
    let mut selected_model_name: Option<String> = None;
    let mut last_err: Option<anyhow::Error> = None;
    let mut quota_failed_agents: Vec<String> = Vec::new();
    let mut agent_attempts: Vec<Value> = Vec::with_capacity(resolve_out.resolved.agents.len() + 2);
    let mut cache_hit = false;
    let cache_bypassed_for_execution =
        should_bypass_for_execution(&params.mode, &routing_out.agent_messages);

    // Token & semantic caches (run concurrently for lower latency)
    let input_text = messages_to_text(&routing_out.agent_messages);
    let estimated_tokens = estimate_messages_token_count(&routing_out.agent_messages);
    let context_class = ContextLengthClass::from_token_count(estimated_tokens);

    // Semantic-cache key: the LAST user message (the current intent), not the
    // full conversation history. History grows append-only, so two consecutive
    // turns share a long prefix; keying on history would make every turn after
    // the first hash-collide with the first (the bucket hash truncates to
    // max_request_hash_len) and both the exact and similarity branches would
    // return turn-1's answer for turn-N's question.
    let semantic_key = routing_out
        .agent_messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .unwrap_or(&input_text);

    // Duplicate-user detection must run BEFORE the canonical lookup: if the
    // last user message repeats an earlier one, serving the cached answer would
    // silently return a stale response (previously the CachedAgentWrapper's
    // duplicate check never ran when act_phase's lookup hit first).
    let is_duplicate_user = crate::intelligence::token_cache::last_user_message_is_duplicate(
        &routing_out.agent_messages,
    );

    #[derive(Default)]
    struct TokenOutcome {
        response_text: String,
        selected_agent: String,
        agent_entry: Option<Value>,
    }

    #[derive(Default)]
    struct SemanticOutcome {
        response_text: String,
        selected_agent: String,
    }

    let (token_outcome, semantic_outcome) = tokio::join!(
        // ── Token cache lookup ────────────────────────────────────────
        async {
            if is_duplicate_user {
                // Repeated user message → bypass cache so the agent produces
                // a fresh response (same intent as CachedAgentWrapper).
                tracing::debug!(
                    target = "token_cache",
                    "act_phase: last user message is a duplicate — bypassing cache"
                );
                TokenOutcome {
                    agent_entry: Some(
                        json!({"agent": "cache", "ok": false, "duplicate_user": true}),
                    ),
                    ..Default::default()
                }
            } else if let Some((level, entry, confidence)) = server
                .cache_deps
                .cache
                .token_cache
                .lookup(&input_text, context_class)
                .await
            {
                let decision = CacheStrategy::decide_from_entry(
                    &format!("{level}"),
                    &entry,
                    confidence,
                    cache_bypassed_for_execution,
                );
                match decision {
                    CacheDecision::Hit { response } => {
                        let agent = resolve_out
                            .resolved
                            .agents
                            .first()
                            .map(|(n, _)| n.clone())
                            .unwrap_or_else(|| "cached".to_string());
                        TokenOutcome {
                            response_text: response.clone(),
                            selected_agent: agent,
                            agent_entry: Some(
                                json!({"agent": "cache", "ok": true, "level": format!("{level}")}),
                            ),
                        }
                    }
                    CacheDecision::Refused { level, reason } => {
                        record_cache_shortcircuit_refused(&reason);
                        record_cache_bypass_for_execution();
                        TokenOutcome {
                            agent_entry: Some(
                                json!({"agent": "cache", "ok": false, "refused": true, "level": format!("{level}")}),
                            ),
                            ..Default::default()
                        }
                    }
                    CacheDecision::Miss => TokenOutcome::default(),
                }
            } else if cache_bypassed_for_execution {
                record_cache_bypass_for_execution();
                TokenOutcome {
                    agent_entry: Some(json!({"agent": "cache", "ok": false})),
                    ..Default::default()
                }
            } else {
                TokenOutcome::default()
            }
        },
        // ── Semantic cache lookup ─────────────────────────────────────
        async {
            if !cache_bypassed_for_execution && !is_duplicate_user {
                if let Some(text) = try_semantic_cache(server, semantic_key) {
                    let agent = resolve_out
                        .resolved
                        .agents
                        .first()
                        .map(|(n, _)| n.clone())
                        .unwrap_or_else(|| "cached".to_string());
                    return SemanticOutcome {
                        response_text: text,
                        selected_agent: agent,
                    };
                }
            }
            SemanticOutcome::default()
        },
    );

    // Merge: token cache has priority
    let token_hit = !token_outcome.response_text.is_empty();
    let semantic_hit = !semantic_outcome.response_text.is_empty();

    // Apply non-hit agent-entries immediately (Refused / bypass -- no streaming needed)
    if !token_hit {
        if let Some(ref entry) = token_outcome.agent_entry {
            agent_attempts.push(entry.clone());
        }
    }

    if token_hit {
        cache_hit = true;
        response_text.clone_from(&token_outcome.response_text);
        selected_agent.clone_from(&token_outcome.selected_agent);
        stream_cache_response(
            server,
            stream_observer.as_ref(),
            &selected_agent,
            &resolve_out.phase_name,
            &trace.trace_id,
            &response_text,
            &None,
        )
        .await?;
        // Push hit entry only after successful stream (preserves original ordering)
        if let Some(entry) = token_outcome.agent_entry {
            agent_attempts.push(entry);
        }
    } else if semantic_hit {
        cache_hit = true;
        response_text.clone_from(&semantic_outcome.response_text);
        selected_agent.clone_from(&semantic_outcome.selected_agent);
        stream_cache_response(
            server,
            stream_observer.as_ref(),
            &selected_agent,
            &resolve_out.phase_name,
            &trace.trace_id,
            &response_text,
            &None,
        )
        .await?;
    }

    // ── Pre-execution review gate (SafeGuard mode) ────────────────────
    let mut review_blocked = false;
    let review_passed = match ModeKind::from(params.mode.as_str()) {
        ModeKind::SafeGuard => {
            let outcome = run_review_gate(
                server,
                &params.messages,
                resolve_out.phase.options.as_ref(),
                None,
                trace,
            )
            .await;
            if !outcome.passed {
                let reason = if outcome.comments.is_empty() {
                    "SafeGuard review blocked the requested operation due to risk detection."
                        .to_string()
                } else {
                    format!(
                        "SafeGuard review blocked the requested operation: {}",
                        outcome.comments.join("; ")
                    )
                };
                tracing::info!(
                    "safeguard review gate blocked execution (verdict={:?}): {reason}",
                    outcome.verdict
                );
                emit_status_event(
                    stream_observer.as_ref(),
                    &format!("{} (verdict: {:?})", reason, outcome.verdict),
                    "warning",
                )
                .await?;
                response_text = reason;
                review_blocked = true;
            }
            outcome.passed
        }
        _ => true,
    };

    // Autonomy round
    let progress_sse_tx = stream_observer.as_ref().and_then(|o| o.sse_sender());
    let autonomy_outcome = if review_passed {
        execute_autonomy_round(
            server,
            params,
            &resolve_out.phase,
            &resolve_out.phase_name,
            &resolve_out.resolved,
            &routing_out.agent_messages,
            &routing_out.base_agent_options,
            cache_hit,
            progress_sse_tx,
        )
        .await
    } else {
        AutonomyOutcome {
            autonomy_loop_executed: false,
            selected_agent: String::new(),
            response_text: String::new(),
            agent_attempts: Vec::new(),
            all_tools_failed: false,
        }
    };
    if autonomy_outcome.autonomy_loop_executed {
        selected_agent = autonomy_outcome.selected_agent;
        response_text = autonomy_outcome.response_text;
    }
    let all_tools_failed = autonomy_outcome.all_tools_failed;
    agent_attempts.extend(autonomy_outcome.agent_attempts);
    let autonomy_loop_executed = autonomy_outcome.autonomy_loop_executed;

    // BLUE56-GAP-C04: Record autonomy round execution in hyper-resilience engine
    if autonomy_loop_executed {
        let success = !response_text.is_empty() && last_err.is_none();
        server
            .resilience
            .hyper_resilience
            .record_execution(&selected_agent, success)
            .await;
    }

    // Fallback + vote
    let mut checkpoint = cognitive_empty_checkpoint();
    let mut knowledge = Value::Null;
    let mut metacognitive_loop = Value::Null;
    let mut distillation = Value::Null;
    // Hoisted out of the fallback block so the post-block stream completion
    // logic can tell whether the high-risk vote path already emitted done.
    let mut emit_final_vote = false;
    // Hoisted vote metadata so ActOutput carries the real values to reflect_phase
    // (risk_decision / routing_diagnostics consumers), instead of None/false.
    let mut used_multi_model_vote = false;
    let mut used_multi_agent_vote = false;
    let mut review_required = false;
    let mut vote_winner: Option<String> = None;
    let mut vote_report: Option<Value> = None;
    if !(cache_hit || autonomy_loop_executed && !response_text.trim().is_empty()) {
        let (fallback_result, vote_result, vote_flag) = execute_fallback_with_vote(
            server,
            params,
            &mut *resolve_out,
            routing_out,
            trace,
            stream_observer.clone(),
            agent_attempts,
        )
        .await?;
        emit_final_vote = vote_flag;

        selected_agent = fallback_result.selected_agent;
        response_text = fallback_result.response_text;
        reasoning_text = fallback_result.reasoning_text;
        selected_model_name = fallback_result.selected_model_name;
        last_err = fallback_result.last_err;
        quota_failed_agents = fallback_result.quota_failed_agents;
        agent_attempts = fallback_result.agent_attempts;

        let (
            used_multi_model_vote_val,
            used_multi_agent_vote_val,
            review_required_val,
            vote_winner_val,
            vote_report_val,
        ) = if emit_final_vote {
            response_text = vote_result.response_text;
            reasoning_text = vote_result.reasoning_text;
            selected_agent = vote_result.selected_agent;
            last_err = vote_result.last_err;
            agent_attempts.extend(vote_result.agent_attempts);
            if let Some(ref observer) = stream_observer {
                let meta = StreamEventMeta {
                    agent_name: &selected_agent,
                    phase_name: &resolve_out.phase_name,
                    trace_id: &trace.trace_id,
                    mode: Some(&params.mode),
                    risk_score: None,
                    degrade_policy: None,
                };
                let total_chars = response_text.chars().count();
                emit_stream_chunk(server, Some(observer), meta, &response_text, 1, total_chars)
                    .await?;
                emit_stream_done(
                    server,
                    Some(observer),
                    meta,
                    1,
                    total_chars,
                    0u64,
                    selected_model_name.clone(),
                    Some(&response_text),
                )
                .await?;
            }
            (
                vote_result.used_multi_model_vote,
                vote_result.used_multi_agent_vote,
                vote_result.review_required,
                vote_result.vote_winner,
                vote_result.vote_report,
            )
        } else {
            (false, false, false, None, None)
        };
        used_multi_model_vote = used_multi_model_vote_val;
        used_multi_agent_vote = used_multi_agent_vote_val;
        review_required = review_required_val;
        vote_winner = vote_winner_val;
        vote_report = vote_report_val;

        // Error handling
        if let Some(early_value) = handle_execution_errors(
            server,
            params,
            &resolve_out.phase_name,
            &resolve_out.phase_origin,
            &resolve_out.tenant_id,
            &response_text,
            &last_err,
            &agent_attempts,
            &quota_failed_agents,
            &routing_out.candidate_agents,
            &routing_out.risk_policy,
            &routing_out.risk_assessment,
            used_multi_model_vote,
            used_multi_agent_vote,
            review_required,
            &vote_report,
            started,
        )
        .await?
        {
            // Use the error prompt as the response text so the user sees
            // actionable guidance (e.g., quota exhaustion, switch agent, retry).
            if let Some(prompt) = early_value.get("prompt").and_then(|v| v.as_str()) {
                if response_text.trim().is_empty() {
                    response_text = prompt.to_string();
                }
            }
        }
        if let Some(err) = last_err.take() {
            return Err(err);
        }

        // Post-success cleanup
        if let Some(ref primary) = routing_out.configured_primary_agent {
            if selected_agent == *primary {
                let _ = agent_switch_state()
                    .write()
                    .map(|mut s| s.forced_agent_by_phase.remove(&resolve_out.phase_name));
            }
        }
        let _ = server
            .resilience
            .outcome_tx
            .send(OutcomeEvent::PhaseOutcome {
                phase_name: resolve_out.phase_name.to_string(),
                success: true,
                duration_ms: started.elapsed().as_millis() as u64,
            });
    }

    // Semantic + token cache populate — runs for ALL successful execution
    // paths (autonomy loop and fallback). Previously this block lived inside
    // the fallback-only branch, so autonomy-produced responses never filled
    // the caches they read on the next request.
    if !cache_hit && !response_text.is_empty() && !cache_bypassed_for_execution {
        // Clone BEFORE acquiring write lock to minimize critical section.
        let cached_response = Value::String(response_text.clone());
        server
            .cache_deps
            .cache
            .semantic_cache
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .put(semantic_key, cached_response);

        // Token cache populate (multi-level: L1 exact + L2 semantic + L3
        // durable) so the primary execution path fills the cache it reads
        // on the next request.
        let token_cache = server.cache_deps.cache.token_cache.clone();
        let model_name = selected_model_name.clone();
        let agent_for_cache = if selected_agent.is_empty() {
            None
        } else {
            Some(selected_agent.clone())
        };
        crate::acp::helpers::cache_strategy::store_async(
            token_cache,
            input_text.clone(),
            response_text.clone(),
            crate::intelligence::token_cache::estimate_token_count(&response_text),
            agent_for_cache,
            model_name,
        );
    }

    // Persistence + post-execute verification — run for ALL successful execution
    // paths (autonomy loop, fallback, or cache hit). Previously this was only
    // reachable via the fallback branch, so autonomy-loop chats never persisted
    // knowledge / checkpoints / distillation / vector memory.
    if !selected_agent.is_empty() && !response_text.is_empty() && last_err.is_none() {
        let mut msgs = params.messages.clone();
        msgs.push(Message {
            role: "assistant".to_string(),
            content: response_text.clone(),
        });
        let (kn, (new_checkpoint_from_join, ml), dst, _vec) = tokio::join!(
            persist_chat_knowledge(
                server,
                &routing_out.conversation_id,
                &routing_out.branch_id,
                &resolve_out.phase_name,
                &selected_agent,
                params,
                &response_text
            ),
            async {
                let cp = request::create_checkpoint_record(
                    server,
                    &routing_out.conversation_id,
                    &routing_out.branch_id,
                    msgs,
                    None,
                    None,
                )
                .await;
                let ml = request::persist_checkpoint_metacognitive_loop(
                    server,
                    &routing_out.conversation_id,
                    &routing_out.branch_id,
                    &cp.checkpoint_id,
                    json!({
                        "active": true, "schema_version": "blue25-metacognitive-loop-v1", "cycle_count": 1,
                        "checkpoint_id": cp.checkpoint_id, "last_reflection": format!("{}:{}", resolve_out.phase_name, selected_agent),
                        "trigger": "response_completed", "last_selected_agent": selected_agent, "response_chars": response_text.chars().count(),
                    })
                ).await;
                (cp, ml)
            },
            persist_session_distillation(
                server,
                &routing_out.conversation_id,
                &routing_out.branch_id,
                &resolve_out.phase_name,
                params,
                &selected_agent,
                &routing_out.candidate_agents,
                &agent_attempts,
                &response_text
            ),
            persist_vector_memory(
                server,
                &resolve_out.phase_name,
                resolve_out.phase.options.as_ref(),
                params,
                &response_text,
                &selected_agent,
            ),
        );
        checkpoint = crate::acp::ConversationCheckpoint {
            metacognitive_loop: Some(ml.clone()),
            ..new_checkpoint_from_join
        };
        knowledge = kn;
        metacognitive_loop = ml;
        distillation = dst;

        // HarnessBus post-execute — this runs in the PUA verification stage,
        // so pass the real stage so the PUA evidence chain is evaluated
        // against the actual stage requirements.
        if let Some(ref harness) = server.governance_deps.harness_bus {
            let output_v = harness.verify_output(
                &json!({"agent": &selected_agent, "response": &response_text, "reasoning": &reasoning_text, "phase": &resolve_out.phase_name}),
                "verification",
            );
            if !output_v.quality {
                tracing::warn!(target: "harness_bus", risk_score = output_v.risk_score, "post-execute: verification flagged quality issue");
            }
        }
    }

    // Stream completion events (telemetry + done) — emitted for ALL successful
    // execution paths (autonomy loop, fallback, or cache hit). Previously these
    // only fired inside the fallback branch, so autonomy-loop chats never sent
    // telemetry/done. The high-risk vote path already emits chunk+done in-block.
    if !selected_agent.is_empty() && !response_text.is_empty() && last_err.is_none() {
        if let Some(ref observer) = stream_observer {
            let meta = StreamEventMeta {
                agent_name: &selected_agent,
                phase_name: &resolve_out.phase_name,
                trace_id: &trace.trace_id,
                mode: Some(&params.mode),
                risk_score: None,
                degrade_policy: None,
            };
            emit_stream_token_economy(
                server,
                Some(observer),
                meta,
                &estimate_token_economy(&params.messages, &response_text),
            )
            .await?;
            if !emit_final_vote {
                let total_chars = response_text.chars().count();
                emit_stream_done(
                    server,
                    Some(observer),
                    meta,
                    1,
                    total_chars,
                    started.elapsed().as_millis() as u64,
                    selected_model_name.clone(),
                    Some(&response_text),
                )
                .await?;
            }
        }
    }

    // Trace event — recorded for ALL successful execution paths (autonomy loop,
    // fallback, or cache hit). Previously this only fired inside the fallback
    // branch, so autonomy-loop chats had no phase.agent telemetry.
    if !selected_agent.is_empty() && !response_text.is_empty() && last_err.is_none() {
        request::append_trace_event(TraceEvent {
            timestamp: crate::shared::timestamps::now_ts().to_string(),
            event_type: "phase.agent".into(),
            task_id: "chat".into(),
            phase: resolve_out.phase_name.clone(),
            agent: Some(selected_agent.clone()),
            tool: None,
            status: "ok".into(),
            inputs: json!({"attributes": {"agent": selected_agent.clone()}}),
            outputs: None,
            duration_ms: 0,
            error: None,
            pua_stage: None,
        });
    }

    // Global performance accounting for chat happens at the transport
    // boundary: the HTTP route (`route_http_post`) and the stdio dispatch
    // loop (`runtime.rs`) each record one op per request. Recording here as
    // well would double-count HTTP chat requests (route + act_phase) in the
    // same global store. Cache-hit and fallback-early chats are covered by
    // the transport-level records.
    Ok(ActOutput {
        selected_agent,
        response_text,
        reasoning_text,
        selected_model_name,
        last_err,
        cache_hit,
        cache_bypassed_for_execution,
        agent_attempts,
        quota_failed_agents,
        vote_winner,
        vote_report,
        used_multi_model_vote,
        used_multi_agent_vote,
        review_required,
        review_blocked,
        all_tools_failed,
        checkpoint,
        knowledge,
        metacognitive_loop,
        distillation,
    })
}

fn cognitive_empty_checkpoint() -> crate::acp::ConversationCheckpoint {
    crate::acp::ConversationCheckpoint {
        checkpoint_id: String::new(),
        conversation_id: String::new(),
        branch_id: String::new(),
        parent_checkpoint_id: None,
        created_at: 0,
        note: None,
        metacognitive_loop: None,
        messages: Vec::new(),
    }
}

// ── Execution internal helpers ──────────────────────────────────────────

fn try_semantic_cache(server: &AcpServer, cache_key: &str) -> Option<String> {
    server
        .cache_deps
        .cache
        .semantic_cache
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .get(cache_key)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
}

async fn stream_cache_response(
    server: &AcpServer,
    observer: Option<&StreamObserver>,
    agent: &str,
    phase: &str,
    tid: &str,
    text: &str,
    model: &Option<String>,
) -> Result<()> {
    if let Some(o) = observer {
        let meta = StreamEventMeta {
            agent_name: agent,
            phase_name: phase,
            trace_id: tid,
            mode: None,
            risk_score: None,
            degrade_policy: None,
        };
        let total = text.chars().count();
        emit_stream_chunk(server, Some(o), meta, text, 1, total).await?;
        emit_stream_done(
            server,
            Some(o),
            meta,
            1,
            total,
            0u64,
            model.clone(),
            Some(text),
        )
        .await?;
    }
    Ok(())
}

async fn execute_fallback_with_vote(
    server: &AcpServer,
    params: &ChatParams,
    resolve_out: &mut ObserveOutput,
    routing_out: &ThinkOutput,
    trace: &RequestTraceContext,
    stream_observer: Option<StreamObserver>,
    _agent_attempts: Vec<Value>,
) -> Result<(FallbackExecutionResult, HighRiskVoteExecutionResult, bool)> {
    // Derive the effective operation mode the same way execute_autonomy_round
    // does, so fallback tool execution carries the real mode (approval events
    // report safeguard/edit/full_auto instead of a hard-coded "edit").
    let effective_mode = if params.mode.is_empty() {
        "edit"
    } else {
        params.mode.as_str()
    };
    let is_safeguard = effective_mode == "safeguard";
    let fallback_result = execute_fallback_agents(
        server,
        resolve_out.resolved.agents.clone(),
        &resolve_out.phase,
        &resolve_out.phase_name,
        routing_out.agent_messages.clone(),
        routing_out.base_agent_options.clone(),
        stream_observer,
        trace,
        routing_out.unhealthy_fallback_agent.clone(),
        routing_out.enable_high_risk_multi_agent_vote,
        routing_out.max_vote_agents,
        &resolve_out.tenant_id,
        effective_mode,
        is_safeguard,
    )
    .await;

    let vote_result = execute_high_risk_vote(
        server,
        &resolve_out.phase_name,
        &trace.trace_id,
        fallback_result.high_risk_vote_jobs.clone(),
        &routing_out.agent_messages,
        resolve_out.phase.principles.clone(),
        request_timeout(resolve_out.phase.options.as_ref()),
        false,
        routing_out.enable_high_risk_multi_agent_vote,
        routing_out.min_vote_agents,
        routing_out.max_vote_agents,
        routing_out.escalation_enabled,
        routing_out.escalation_models_per_agent,
        routing_out.escalation_max_agents,
        &resolve_out.reputation_scores,
        &mut resolve_out.routing_provenance,
    )
    .await;

    let emit_final_vote = vote_result.emit_final_vote_response;
    Ok((fallback_result, vote_result, emit_final_vote))
}

/// Handle errors from execution. Returns `Some(value)` for early return or `None` to continue.
#[allow(clippy::too_many_arguments)]
async fn handle_execution_errors(
    server: &AcpServer,
    params: &ChatParams,
    phase_name: &str,
    phase_origin: &str,
    _tenant_id: &str,
    response_text: &str,
    last_err: &Option<anyhow::Error>,
    agent_attempts: &[Value],
    quota_failed_agents: &[String],
    candidate_agents: &[String],
    risk_policy: &RiskVotePolicy,
    risk_assessment: &RiskAssessment,
    used_multi_model_vote: bool,
    used_multi_agent_vote: bool,
    review_required: bool,
    vote_report: &Option<Value>,
    started: Instant,
) -> Result<Option<Value>> {
    if response_text.is_empty() && last_err.is_none() {
        let all_empty = !agent_attempts.is_empty()
            && agent_attempts.iter().all(|a| {
                a.get("ok")
                    .and_then(|v| v.as_bool())
                    .map(|ok| !ok)
                    .unwrap_or(false)
                    && a.get("error")
                        .and_then(|v| v.as_str())
                        .map(|e| e == "empty_response")
                        .unwrap_or(false)
            });
        if all_empty {
            return Ok(Some(json!({
                "done": false, "mode": params.mode, "phase": phase_name, "phase_origin": phase_origin,
                "requires_user_action": true, "action": "retry",
                "prompt": tf("error.chat.all_agents_empty", &[("phase", phase_name)]),
                "agent_attempts": agent_attempts,
            })));
        }
        return Ok(Some(json!({
            "done": false, "mode": params.mode, "phase": phase_name, "phase_origin": phase_origin,
            "requires_user_action": true, "action": "retry",
            "prompt": tf("error.chat.no_healthy_agent", &[("phase", phase_name)]),
            "agent_attempts": agent_attempts,
        })));
    }
    if let Some(_err) = last_err {
        let all_quota = !agent_attempts.is_empty()
            && agent_attempts.iter().all(|a| {
                a.get("ok")
                    .and_then(|v| v.as_bool())
                    .map(|ok| !ok)
                    .unwrap_or(false)
                    && a.get("quota_limited")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
            });
        let _ = server
            .resilience
            .outcome_tx
            .send(OutcomeEvent::PhaseOutcome {
                phase_name: phase_name.to_string(),
                success: false,
                duration_ms: started.elapsed().as_millis() as u64,
            });
        if all_quota {
            return Ok(Some(json!({
                "done": false, "mode": params.mode, "phase": phase_name, "phase_origin": phase_origin,
                "requires_user_action": true, "action": "switch_agent",
                "prompt": tf("error.chat.all_agents_quota_limited", &[("phase", phase_name)]),
                "available_agents": candidate_agents, "quota_failed_agents": quota_failed_agents,
                "agent_attempts": agent_attempts,
                "risk_decision": json!({"policy_enabled": risk_policy.enabled, "score": risk_assessment.score,
                    "is_high_risk": risk_assessment.is_high_risk, "reasons": risk_assessment.reasons,
                    "multi_model_vote_enabled": used_multi_model_vote, "multi_agent_vote_enabled": used_multi_agent_vote,
                    "review_required": review_required, "vote_report": vote_report}),
                "hint": {"options_field": "options.extra.preferred_agent",
                    "example": {"preferred_agent": candidate_agents.first().cloned().unwrap_or_else(|| "primary".into())}},
            })));
        }
    }
    Ok(None)
}

// ═════════════════════════════════════════════════════════════════════
// Phase 4: Reflect
// ═════════════════════════════════════════════════════════════════════

/// Phase 4: Reflect on outcomes: response assembly, error handling,
/// knowledge persistence, metacognitive updates, threshold learning,
/// capability bus feedback, BrainLoop reflection.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn reflect_phase(
    server: &AcpServer,
    params: &ChatParams,
    trace: &RequestTraceContext,
    _span: Option<&OtelContext>,
    started: Instant,
    _stream_observer: Option<StreamObserver>,
    resolve_out: &ObserveOutput,
    routing_out: &ThinkOutput,
    exec_out: &mut ActOutput,
) -> Result<serde_json::Value> {
    // NOTE: the cache-hit early return is handled by `ChatPipeline::run`
    // before this phase — the identical branch inside `reflect_phase` was
    // unreachable (this function's only caller guards it).

    // ModeRuntime + MultiAgent — skip for ask mode since response is already handled
    let mode_kind = ModeKind::from(params.mode.as_str());
    if mode_kind != ModeKind::Ask {
        run_mode_runtime_and_multi_agent(
            server,
            params,
            trace,
            &resolve_out.phase_name,
            &resolve_out.phase,
            &resolve_out.resolved,
            exec_out,
        )
        .await;
    }

    // Extract inline tool calls from the model's natural-language response
    // for observability and audit. This catches tool calls that appear as
    // JSON blocks or inline markers (e.g. ```json {"tool_call": "read_file"} ```)
    // rather than through the structured streaming protocol.
    let inline_tool_calls =
        crate::acp::r#impl::chat::tool_extraction::extract_tool_calls_from_response(
            &exec_out.response_text,
            16,
        );
    if !inline_tool_calls.is_empty() {
        tracing::info!(
            target: "chat_phases",
            tool_calls = ?inline_tool_calls,
            "reflect_phase: detected {} inline tool call(s) in response",
            inline_tool_calls.len(),
        );
    }

    let risk_decision = json!({
        "policy_enabled": routing_out.risk_policy.enabled,
        "score": routing_out.risk_assessment.score,
        "is_high_risk": routing_out.risk_assessment.is_high_risk,
        "reasons": routing_out.risk_assessment.reasons,
        "multi_model_vote_enabled": routing_out.enable_high_risk_multi_agent_vote,
        "multi_model_vote_used": exec_out.used_multi_model_vote,
        "multi_agent_vote_enabled": routing_out.enable_high_risk_multi_agent_vote,
        "multi_agent_vote_used": exec_out.used_multi_agent_vote,
        "escalation_enabled": routing_out.escalation_enabled,
        "review_required": exec_out.review_required,
        "vote_report": exec_out.vote_report,
    });

    // Background skill/workflow generation runs concurrently with the
    // review-gate assembly — both are independent side-effects on the same
    // response text (the generators cap at one 2s timeout each).
    let response_text_for_skills = exec_out.response_text.clone();
    let empty_tool_results = Vec::<Value>::new();
    let (assemble_result, ()) = tokio::join!(
        apply_review_gate_assemble(
            server,
            params,
            trace,
            &resolve_out.phase_name,
            &resolve_out.phase_origin,
            &exec_out.selected_agent,
            &exec_out.selected_model_name,
            &exec_out.response_text,
            &exec_out.reasoning_text,
            &resolve_out.tenant_id,
            started,
            &routing_out.conversation_id,
            &routing_out.branch_id,
            resolve_out.schema_warnings.clone(),
            resolve_out.schema_error.clone(),
            routing_out.layered_prompt_segments,
            &empty_tool_results,
            &routing_out.candidate_agents,
            &resolve_out.routing_provenance,
            &resolve_out.reputation_scores,
            resolve_out
                .reputation_scores
                .get(&exec_out.selected_agent)
                .copied(),
            &routing_out.council_decision,
            &exec_out.vote_winner,
            &routing_out.fallback_reason,
            exec_out.cache_hit,
            exec_out.cache_bypassed_for_execution,
            CapabilityRoutingInfo {
                selected_agent: routing_out.capability_selected_agent.clone(),
                recommended_mode: routing_out.capability_recommended_mode.clone(),
                candidate_count: routing_out.capability_candidate_count,
                decision_confidence: routing_out.capability_decision_confidence,
                selection_reason: routing_out.capability_selection_reason.clone(),
                optimization_hint: routing_out.capability_optimization_hint.clone(),
            },
            Vec::new(),
            std::mem::take(&mut exec_out.agent_attempts),
            risk_decision,
            exec_out.quota_failed_agents.clone(),
            routing_out.vector_context.clone(),
            std::mem::take(&mut exec_out.knowledge),
            std::mem::take(&mut exec_out.distillation),
            std::mem::take(&mut exec_out.checkpoint),
            std::mem::take(&mut exec_out.metacognitive_loop),
        ),
        async {
            // Codex-style: skip for simple chat — no meaningful patterns to extract.
            if !is_simple_chat(params) {
                let (skills_res, workflow_res) = tokio::join!(
                    tokio::time::timeout(
                        std::time::Duration::from_secs(2),
                        auto_create_skills_from_conversation(
                            server,
                            params,
                            &response_text_for_skills
                        ),
                    ),
                    tokio::time::timeout(
                        std::time::Duration::from_secs(2),
                        auto_generate_workflow_from_conversation(
                            server,
                            params,
                            &response_text_for_skills
                        ),
                    ),
                );
                let _ = (skills_res, workflow_res);
            }
        },
    );
    let result = assemble_result?;

    // ── Independent post-execution side-effects ─────────────────────────
    // Rationalization is spawned fire-and-forget (see below); capability
    // feedback, memory-bus completion, provenance recording, and memory-bridge
    // store are mutually independent and hold only shared references —
    // serializing them added their latencies to the response. Run them
    // concurrently instead.
    let task_desc = extract_task_description(&params.messages);
    let confidence = if exec_out.response_text.is_empty() {
        0.3
    } else {
        0.8
    };
    // Rationalization (Delphi debate) runs off the response path: its verdict
    // only feeds a debug log, but awaiting it would block the response up to
    // the voter network timeouts (10s × voters). The audit records and
    // counters inside `rationalize_decision` still run — just asynchronously.
    {
        let agent = exec_out.selected_agent.clone();
        // `task_desc` is also borrowed by the provenance join-block below, so
        // move a clone into the fire-and-forget spawn instead of the original.
        let task_desc = task_desc.clone();
        tokio::spawn(async move {
            let (justified, reason) =
                crate::intelligence::hub::rationalize_decision(&agent, &task_desc, confidence)
                    .await;
            if !justified {
                debug!("rationalize: blocked agent={} reason={}", agent, reason);
            }
        });
    }
    let (_, _, _, _) = tokio::join!(
        capability_bus_feedback(
            server,
            trace,
            &resolve_out.phase_name,
            &exec_out.selected_agent,
            &exec_out.response_text,
            &exec_out.last_err,
            params,
        ),
        store_agent_memory_bus_completion(
            &exec_out.selected_agent,
            resolve_out.user_id.as_deref(),
            &resolve_out.phase_name,
            params,
            &exec_out.response_text,
            &exec_out.last_err,
        ),
        async {
            if let Some(ref ledger) = server.governance_deps.provenance_ledger {
                let _ = ledger
                    .record_provenance(
                        &trace.trace_id,
                        &task_desc,
                        &exec_out.selected_agent,
                        exec_out.last_err.is_none(),
                        started.elapsed().as_millis() as u64,
                    )
                    .await;
            }
        },
        async {
            // Memory bridge: persist reflection outcome (GAP-B54-011).
            // The entry carries the conversation id so session/load and
            // session/resume can restore it (D2: previously session_id was
            // always None, so the warm-tier session lookup found nothing).
            if let Some(mp) = server.get_or_init_memory_persistence() {
                use crate::memory::memory::{MemoryClass, MemoryEntry};
                let entry = MemoryEntry {
                    id: format!("reflect-{}", trace.request_id),
                    class: MemoryClass::Episodic,
                    content: if exec_out.response_text.is_empty() {
                        "empty_response".to_string()
                    } else {
                        exec_out.response_text.clone()
                    },
                    timestamp: crate::shared::timestamps::now_ts_ms().to_string(),
                    // Neutral usefulness for auto-recorded episodic memory
                    // (not user-rated).
                    usefulness: 0.5,
                    staleness: 0,
                    user_id: None,
                    session_id: Some(routing_out.conversation_id.clone()),
                };
                let _ = crate::memory::memory_bridge::bridge_store(
                    &server.persistence.memory_store,
                    mp.as_ref(),
                    entry,
                )
                .await;
            }
        },
    );

    // Metacognitive observation and fusion cycle — gated by
    // `enable_metacognitive_feedback` (the flag was previously written into
    // agent options but never read anywhere, so toggling it had no effect).
    //
    // NOTE: the per-request fire-and-forget persistence save was removed — the
    // periodic background save (every `maintenance_interval_seconds`, plus on
    // graceful shutdown) already persists the same snapshot, so the extra
    // per-request `create_dir_all` + rebuild + write (and its fixed tmp-file
    // races with the background writer) were redundant.
    if server.runtime_config.enable_metacognitive_feedback {
        if let Some(ref cb) = server.governance_deps.capability_bus {
            let success = !exec_out.response_text.is_empty() && exec_out.last_err.is_none();
            let task_desc = task_desc.clone();
            let now_ms = crate::shared::timestamps::now_ts_ms_u64();
            if let Ok(obs_id) = cb.metacognitive.record_observation(
                &format!("chat-{}", now_ms),
                &exec_out.selected_agent,
                if success {
                    "execution_success"
                } else {
                    "execution_failure"
                },
                if success { "info" } else { "error" },
                &format!(
                    "Chat execution for '{}': {}",
                    task_desc,
                    if success { "success" } else { "failed" }
                ),
            ) {
                if !success {
                    cb.metacognitive.autoreflect();
                    let _ = cb.metacognitive.resolve_observation(&obs_id);
                }
            }
        }

        // ── TripleFusion fusion cycle (fire-and-forget, non-blocking) ────────
        if let Some(ref cb) = server.governance_deps.capability_bus {
            let meta = cb.metacognitive.clone();
            let cs = cb.consciousness.clone();
            tokio::spawn(async move {
                let fusion_bridge =
                    crate::intelligence::triple_fusion::global_triple_fusion_bridge();
                let triggers = fusion_bridge.lock().await.run_fusion_cycle(&meta, &cs);
                crate::intelligence::fusion_evolution_bridge::send_triggers_to_evolution(triggers);
            });
        }
    }

    // Include all_tools_failed flag when all tools failed
    if exec_out.all_tools_failed {
        if let Some(obj) = result.as_object() {
            let mut enriched = obj.clone();
            enriched.insert(
                "all_tools_failed".to_string(),
                serde_json::Value::Bool(true),
            );
            enriched.insert(
                "error".to_string(),
                serde_json::Value::String(
                    "All tools failed to execute. The task could not be completed.".to_string(),
                ),
            );
            return Ok(serde_json::Value::Object(enriched));
        }
    }

    // Include review_blocked flag in the response when SafeGuard blocked execution
    if exec_out.review_blocked {
        if let Some(obj) = result.as_object() {
            let mut enriched = obj.clone();
            enriched.insert("review_blocked".to_string(), serde_json::Value::Bool(true));
            return Ok(serde_json::Value::Object(enriched));
        }
    }

    Ok(result)
}

// ── Response assembly helpers ───────────────────────────────────────

async fn run_mode_runtime_and_multi_agent(
    server: &AcpServer,
    params: &ChatParams,
    trace: &RequestTraceContext,
    phase_name: &str,
    _phase: &ResolvedPhase,
    resolved: &crate::orchestration::flow::ResolvedRouting,
    exec_out: &mut ActOutput,
) {
    let mode_runtime = match resolve_mode_runtime(
        &params.mode,
        server.agent_registry(),
        Some(exec_out.selected_agent.clone()),
    ) {
        Ok(runtime) => runtime,
        Err(e) => {
            tracing::warn!("failed to resolve mode runtime: {}", e);
            return;
        }
    };

    // Determine whether to run the mode runtime:
    //
    // ┌────────────┬─────────────────────────┬──────────────────────────────┐
    // │ All modes  │ Only as safety net      │ Captured (emergency         │
    // │            │ when act_phase          │ fallback) when act_phase    │
    // │            │ output is empty         │ output is empty             │
    // └────────────┴─────────────────────────┴──────────────────────────────┘
    //
    // The act_phase autonomy loop is the primary execution engine for ALL
    // modes (including FullAuto and SafeGuard). It runs the multi-round
    // think→act→observe cycle with tool execution, properly passing the
    // user's selected model via base_agent_options.
    //
    // Now all modes skip the mode runtime when act_phase produced output,
    // making FullAuto / SafeGuard fully autonomous via the autonomy loop.
    // The mode runtime is only kept as an emergency safety net: when act_phase
    // left both response AND agent empty, it can attempt recovery.
    let act_phase_produced_output =
        !exec_out.response_text.trim().is_empty() || !exec_out.selected_agent.trim().is_empty();
    let should_run = !exec_out.cache_hit && !act_phase_produced_output;

    if should_run {
        let envelope = crate::agent::AgentTaskEnvelope {
            task_id: format!("chat-{}-{}", phase_name, trace.request_id),
            phase: phase_name.to_string(),
            role: exec_out.selected_agent.clone(),
            objective: extract_task_description(&params.messages),
            constraints: None,
            evidence: Some(
                params
                    .messages
                    .iter()
                    .map(|m| format!("{}: {}", m.role, m.content))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            input: json!({"response_text": exec_out.response_text, "reasoning_text": exec_out.reasoning_text}),
        };
        if let Ok(result) = mode_runtime.run(envelope).await {
            // Capture the mode runtime's output back into exec_out so the
            // final "result" SSE event carries the actual response.
            // (should_run implies !act_phase_produced_output, so capture is
            // unconditional here — the old `should_capture` flag was always true.)
            if let Some(ref output) = result.output {
                let answer = output.get("answer").and_then(|v| v.as_str());
                let agent_from_output = output.get("agent").and_then(|v| v.as_str());
                if let Some(text) = answer {
                    if !text.trim().is_empty() {
                        exec_out.response_text = text.to_string();
                    }
                }
                if let Some(name) = agent_from_output {
                    if !name.trim().is_empty() {
                        exec_out.selected_agent = name.to_string();
                    }
                }
            }
        }
    }

    // ── Multi-agent pipeline (Edit + multiple agents) ──────────────────
    // Runs only as a safety net when the act-phase autonomy loop produced no
    // output — the same guard applied to the mode runtime above. Without it,
    // every Edit-mode request with >1 agents triggered a second full agentic
    // execution (LLM decomposition + per-subtask LLM calls) whose result
    // silently OVERWROTE the act-phase response.
    if matches!(mode_runtime.kind(), ModeKind::Edit)
        && resolved.agents.len() > 1
        && !exec_out.cache_hit
        && !act_phase_produced_output
    {
        run_multi_agent_pipeline(server, params, resolved, exec_out, phase_name).await;
    }
}

async fn run_multi_agent_pipeline(
    server: &AcpServer,
    params: &ChatParams,
    resolved: &crate::orchestration::flow::ResolvedRouting,
    exec_out: &mut ActOutput,
    _phase_name: &str,
) {
    let desc = extract_task_description(&params.messages);

    // Single authoritative task analysis — reuse TaskRouter::analyze_task
    // instead of maintaining a second keyword/classification implementation
    // here (previously the two drifted on thresholds and type mapping).
    let task_chars = crate::orchestration::task_router::TaskRouter::analyze_task(&desc);
    let registry = server
        .agent_registry()
        .unwrap_or_else(|| Arc::new(crate::agent::AgentRegistry::new()));
    let pipeline_result = MultiAgentPipeline::new(registry)
        .execute(
            &desc,
            &task_chars,
            resolved.agents.first().map(|(_, a)| a.clone()),
        )
        .await;

    if pipeline_result.succeeded_count > 0 {
        exec_out.response_text = pipeline_result
            .merged_output
            .get("subtask_outputs")
            .and_then(|o| serde_json::to_string(o).ok())
            .unwrap_or_default();
        exec_out.selected_agent = format!(
            "multi-agent ({} succeeded)",
            pipeline_result.succeeded_count
        );
    }
}

#[allow(clippy::too_many_arguments)]
async fn capability_bus_feedback(
    server: &AcpServer,
    trace: &RequestTraceContext,
    phase_name: &str,
    selected_agent: &str,
    response_text: &str,
    last_err: &Option<anyhow::Error>,
    params: &ChatParams,
) {
    if let Some(ref cb) = server.governance_deps.capability_bus {
        let success = !response_text.is_empty() && last_err.is_none();
        let economy = estimate_token_economy(&params.messages, response_text);
        let token_cost_est = economy["total_tokens"].as_u64().unwrap_or(0);
        // NOTE: the per-request cb.feedback call was removed — the single
        // feedback point is finalize_chat_response (weight 1.0, stable
        // conversation_id). This function now only drives the THROTTLED
        // evolve() (every evolve_interval requests) so learning happens
        // without a per-request full pipeline spawn.

        // The evolve cycle is gated by `enable_capability_bus` (previously the
        // flag was never read — the 12-subsystem evolve ran regardless).
        if !cb.config.enable_capability_bus {
            return;
        }

        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if count > 0 && count.is_multiple_of(cb.config.evolve_interval) {
            let cb = cb.clone();
            let agent_owned = selected_agent.to_string();
            let phase_owned = phase_name.to_string();
            // Pre-allocate the second copy to avoid a clone inside the tuple constructor.
            let agent_owned2 = agent_owned.clone();
            let phase_complete = format!("{}_complete", &phase_owned);
            let _child = child_trace_context(trace, "evolve");
            tokio::spawn(async move {
                cb.evolve(
                    &(agent_owned, phase_owned),
                    "chat_complete",
                    &(agent_owned2, phase_complete),
                    token_cost_est,
                    success,
                    if success { 0.8 } else { 0.2 },
                )
                .await;
            });
        }
    }
}

async fn store_agent_memory_bus_completion(
    selected_agent: &str,
    user_id: Option<&str>,
    phase_name: &str,
    params: &ChatParams,
    response_text: &str,
    last_err: &Option<anyhow::Error>,
) {
    use crate::memory::agent_memory_bus::AGENT_MEMORY_BUS;
    if let Some(bus) = AGENT_MEMORY_BUS.get() {
        let success = !response_text.is_empty() && last_err.is_none();
        bus.store_agent_completion(
            selected_agent,
            phase_name,
            &extract_task_description(&params.messages),
            response_text,
            success,
            user_id,
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multimodal::MultimodalProcessor;

    /// `process_image` needs no sub-processor, so a bare `MultimodalProcessor::new()`
    /// is enough to verify inline `data:` URI extraction (F-GAP-66).
    #[tokio::test]
    async fn extract_data_uris_handles_multiple_uris_in_one_message() {
        let mp = MultimodalProcessor::new();
        let mut contexts = Vec::new();
        let content = "see data:image/png;base64,QUJD and data:image/png;base64,REVG";

        extract_data_uris(&mp, content, &mut contexts).await;

        assert_eq!(contexts.len(), 2, "both inline data URIs must be processed");
        assert!(contexts[0].contains("QUJD"));
        assert!(contexts[1].contains("REVG"));
    }

    #[tokio::test]
    async fn extract_data_uris_stops_at_markdown_bracket() {
        let mp = MultimodalProcessor::new();
        let mut contexts = Vec::new();
        let content = "![x](data:image/png;base64,QUJDREVG)";

        extract_data_uris(&mp, content, &mut contexts).await;

        assert_eq!(contexts.len(), 1);
        assert!(contexts[0].contains("QUJDREVG"));
    }

    #[tokio::test]
    async fn extract_data_uris_ignores_empty_payload() {
        let mp = MultimodalProcessor::new();
        let mut contexts = Vec::new();

        extract_data_uris(&mp, "data:image/png;base64,", &mut contexts).await;

        assert!(contexts.is_empty());
    }

    #[test]
    fn semantic_skip_requires_hit_and_no_bypass() {
        let plain = vec![Message {
            role: "user".to_string(),
            content: "what is rust?".to_string(),
        }];
        // Hit + plain question → skip the expensive observe sub-steps.
        assert!(semantic_prefetch_should_skip_condition(
            "chat", &plain, true
        ));
        // Miss → never skip.
        assert!(!semantic_prefetch_should_skip_condition(
            "chat", &plain, false
        ));
        // Execution mode → bypass → never skip.
        assert!(!semantic_prefetch_should_skip_condition(
            "edit", &plain, true
        ));
        // Execution hint in the user text → bypass → never skip.
        let exec = vec![Message {
            role: "user".to_string(),
            content: "implement a login form".to_string(),
        }];
        assert!(!semantic_prefetch_should_skip_condition(
            "chat", &exec, true
        ));
        // Repeated last user message → bypass → never skip.
        let dup = vec![
            Message {
                role: "user".to_string(),
                content: "what is rust?".to_string(),
            },
            Message {
                role: "user".to_string(),
                content: "what is rust?".to_string(),
            },
        ];
        assert!(!semantic_prefetch_should_skip_condition("chat", &dup, true));
        // No user message at all → never skip.
        let no_user = vec![Message {
            role: "system".to_string(),
            content: "context only".to_string(),
        }];
        assert!(!semantic_prefetch_should_skip_condition(
            "chat", &no_user, true
        ));
    }
}
