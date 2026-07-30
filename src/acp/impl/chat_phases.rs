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
use std::sync::{Mutex as StdMutex, OnceLock};
use std::time::Instant;

use anyhow::Result;
use opentelemetry::Context as OtelContext;
use serde_json::{json, Value};
use tracing::{debug, info};
use futures_util::future::join_all;

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
    auto_generate_workflow_from_conversation, clear_task_description_cache, emit_status_event,
    emit_stream_chunk, emit_stream_done, emit_stream_token_economy, estimate_token_economy,
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
use crate::observability::performance::record_global_operation;
use crate::orchestration::core_dag::TaskContext;
use crate::orchestration::flow::ResolvedPhase;
use crate::orchestration::mode::{resolve_mode_runtime, ModeKind};
use crate::orchestration::multi_agent_pipeline::MultiAgentPipeline;
use crate::orchestration::task_router::{TaskCharacteristics, TaskType};
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

// ── ThresholdLearner (INT-2) ──────────────────────────────────────────
static CHAT_THRESHOLD_LEARNER: OnceLock<
    StdMutex<crate::orchestration::threshold_learner::ThresholdLearner>,
> = OnceLock::new();

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
    clear_task_description_cache();
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
    if let Some(ref detector) = server.governance_deps.injection_detector {
        use crate::security::severity::DetectionSeverity as InjectionSeverity;

        // Detect and classify severity across ALL messages first.
        let mut has_high_or_critical = false;
        let mut all_violations: Vec<crate::security::prompt_injection::SafetyViolation> =
            Vec::new();
        let mut max_contamination = 0.0_f64;

        for msg in &params.messages {
            let result = detector.detect(&msg.content);
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

            // Always record to audit trail.
            if let Some(ref auditor) = server.governance_deps.hash_chain_auditor {
                if let Ok(mut auditor) = auditor.lock() {
                    let _ = auditor.append(
                        json!({"event": "prompt_injection", "action": "enforced", "warnings": &all_violations, "contamination_score": max_contamination}),
                    );
                }
            }

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

            // ── MEDIUM/LOW: sanitize each message individually ────────
            for msg in &mut params.messages {
                let (sanitized, _) = detector.detect_and_sanitize(&msg.content);
                msg.content = sanitized;
            }
        }
    }

    // ── Sub-step 2: Multimodal input detection ───────────────────
    // Codex-style: skip for simple chat — no files/URIs to process
    let is_simple = is_simple_chat(params);
    let multimodal_context = if is_simple {
        None
    } else {
        emit_status_event(
            stream_observer,
            "Processing multimodal input (images, files, audio)...",
            "analyzing",
        )
        .await?;
        detect_and_process_multimodal(server, params).await
    };

    // ── Sub-step 3: URL auto-detection & pre-fetching ─────────────
    // NOTE: Only scans the LAST user message for URLs to avoid re-fetching
    // URLs from conversation history on every turn. Timeouts are aggressive
    // (3s) so a slow/unreachable URL does not block the chat pipeline.
    {
        let url_entries: Vec<(usize, String)> = params
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
            .collect();

        if url_entries.is_empty() {
            // Fast path: no URLs to pre-fetch, skip immediately
        } else {
            emit_status_event(stream_observer, "Pre-fetching URLs...", "analyzing").await?;
        }

        // Phase 1: Fetch all URLs in parallel
        let fetch_futures: Vec<_> = url_entries.iter().filter_map(|(msg_idx, url)| {
            let url_lower = url.to_lowercase();
            if url_lower.starts_with("http://localhost")
                || url_lower.starts_with("http://127.0.0.1")
                || url_lower.starts_with("https://localhost")
                || url_lower.starts_with("https://127.0.0.1")
                || url_lower.starts_with("http://10.")
                || url_lower.starts_with("http://192.168.")
                || url_lower.starts_with("https://10.")
                || url_lower.starts_with("https://192.168.")
            {
                tracing::info!(
                    "observe_phase: skipping pre-fetch for local/private URL: {}",
                    url
                );
                return None;
            }

            let fetch_url = url.split('#').next().unwrap_or(url).to_string();
            let url_owned = url.clone();
            let msg_idx = *msg_idx;
            Some(async move {
                tracing::info!("observe_phase: auto-detected URL, pre-fetching: {}", url_owned);

                let result = match tokio::time::timeout(
                    std::time::Duration::from_secs(3),
                    reqwest::get(&fetch_url),
                )
                .await
                {
                    Ok(Ok(resp)) => {
                        let status = resp.status().to_string();
                        match tokio::time::timeout(std::time::Duration::from_secs(2), resp.text()).await
                        {
                            Ok(Ok(body)) => Some((status, body)),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                (msg_idx, url_owned, fetch_url, result)
            })
        }).collect();

        let fetch_results = join_all(fetch_futures).await;

        // Phase 2: Process each result sequentially (SPA detection, API probing, message insertion)
        for (msg_idx, url, fetch_url, fetch_result) in fetch_results {
            if let Some((status, body)) = fetch_result {
                let truncated = if body.len() > 8192 {
                    format!("{}...\n[Response truncated at 8192 bytes]", &body[..8192])
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
                    tracing::info!("observe_phase: detected SPA page, probing API: {}", url);

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
                    if let Some(invitation_id) = path_segments
                        .last()
                        .filter(|s| s.starts_with("invite_") || s.starts_with("invitation_"))
                    {
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
                                std::time::Duration::from_secs(10),
                                reqwest::Client::new()
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
                                        std::time::Duration::from_secs(5),
                                        api_resp.text(),
                                    )
                                    .await
                                    {
                                        Ok(Ok(api_body_text)) => {
                                            let t = if api_body_text.len() > 4096 {
                                                format!(
                                                    "{}...\n[truncated]",
                                                    &api_body_text[..4096]
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
                                Err(_) => spa_info
                                    .push_str(&format!("\n\n[API: POST {}] - timeout", api_url)),
                            }
                        }
                    }
                    context_msg.push_str(&spa_info);
                }

                params.messages.insert(
                    msg_idx,
                    Message {
                        role: "system".to_string(),
                        content: context_msg,
                    },
                );
            } else {
                tracing::warn!("observe_phase: failed to fetch URL: {}", url);
            }
        }
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
        if content.starts_with("data:") {
            process_data_uri(mp, content, &mut contexts).await;
            continue;
        }
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

async fn process_data_uri(
    mp: &crate::multimodal::MultimodalProcessor,
    content: &str,
    contexts: &mut Vec<String>,
) {
    use crate::multimodal::MultimodalInput;
    if let Some(rest) = content.strip_prefix("data:") {
        if let Some((mime, b64)) = rest.split_once(";base64,") {
            if let Ok(bytes) = crate::multimodal::base64_decode(b64) {
                let ml = mime.to_lowercase();
                let input = if ml.contains("image") {
                    MultimodalInput::Image(bytes)
                } else if ml.contains("audio") {
                    MultimodalInput::Audio(bytes)
                } else if ml.contains("video") {
                    MultimodalInput::Video(bytes)
                } else {
                    MultimodalInput::Document(bytes, crate::multimodal::mime_to_extension(&ml))
                };
                let processed = mp.process_input(&input).await;
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
    resolve_out: &ObserveOutput,
    routing_out: &ThinkOutput,
) -> Result<ActOutput> {
    let act_started = Instant::now();
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
    let sched_task_id = trace.request_id.clone();

    // Create intermediate file directory for this task
    let _ = crate::orchestration::intermediate::create_task_intermediate_dir(&trace.request_id);

    // Scheduler
    observe_submit_to_scheduler(server, &resolve_out.resolved, &sched_task_id).await;

    // Token & semantic caches (run concurrently for lower latency)
    let input_text = messages_to_text(&routing_out.agent_messages);
    let estimated_tokens = estimate_messages_token_count(&routing_out.agent_messages);
    let context_class = ContextLengthClass::from_token_count(estimated_tokens);

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
            if let Some((level, entry)) = server
                .cache_deps
                .cache
                .token_cache
                .lookup(&input_text, context_class)
                .await
            {
                let decision = CacheStrategy::decide_from_entry(
                    &format!("{level}"),
                    &entry,
                    &input_text,
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
            if !cache_bypassed_for_execution {
                if let Some(text) = try_semantic_cache(server, &input_text) {
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
                tracing::info!("safeguard review gate blocked execution: {reason}");
                emit_status_event(stream_observer.as_ref(), &reason, "warning").await?;
                response_text = reason;
                review_blocked = true;
            }
            outcome.passed
        }
        _ => true,
    };

    // Autonomy round
    let mut task_contexts: Vec<TaskContext> = Vec::new();
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

    // TaskContext propagation
    if autonomy_loop_executed && !response_text.is_empty() {
        let mut ctx = TaskContext::new(format!(
            "acp-{}-{}",
            resolve_out.phase_name, trace.request_id
        ));
        ctx.reasoning_trace.push(format!(
            "Autonomy round for phase '{}' using agent '{}'",
            resolve_out.phase_name, selected_agent
        ));
        ctx.intermediate_findings.insert(
            "response_length".into(),
            Value::Number(serde_json::Number::from(response_text.len() as u64)),
        );
        ctx.intermediate_findings
            .insert("mode".into(), Value::String(params.mode.clone()));
        ctx.intermediate_findings
            .insert("agent".into(), Value::String(selected_agent.clone()));
        task_contexts.push(ctx);
    }

    // Fallback + vote
    let mut checkpoint = cognitive_empty_checkpoint();
    let mut knowledge = Value::Null;
    let mut metacognitive_loop = Value::Null;
    let mut distillation = Value::Null;
    if !(cache_hit || autonomy_loop_executed && !response_text.trim().is_empty()) {
        let (fallback_result, vote_result, emit_final_vote) = execute_fallback_with_vote(
            server,
            params,
            resolve_out,
            routing_out,
            trace,
            stream_observer.clone(),
            agent_attempts,
        )
        .await?;

        selected_agent = fallback_result.selected_agent;
        response_text = fallback_result.response_text;
        reasoning_text = fallback_result.reasoning_text;
        selected_model_name = fallback_result.selected_model_name;
        last_err = fallback_result.last_err;
        quota_failed_agents = fallback_result.quota_failed_agents;
        agent_attempts = fallback_result.agent_attempts;

        let (
            used_multi_model_vote,
            used_multi_agent_vote,
            review_required,
            _vote_winner,
            vote_report,
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

        // Semantic cache populate
        if !cache_hit && !response_text.is_empty() && !cache_bypassed_for_execution {
            // Clone BEFORE acquiring write lock to minimize critical section.
            let cached_response = Value::String(response_text.clone());
            server
                .cache_deps
                .cache
                .semantic_cache
                .write()
                .unwrap_or_else(|p| p.into_inner())
                .put(&input_text, cached_response);
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

        // Persistence (BLUE69: all 4 ops run concurrently via tokio::join!)
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
                        "reflection_trigger": "response_completed", "last_selected_agent": selected_agent, "response_chars": response_text.chars().count(),
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

        if stream_observer.is_some() {
            emit_stream_token_economy(
                server,
                stream_observer.as_ref(),
                StreamEventMeta {
                    agent_name: &selected_agent,
                    phase_name: &resolve_out.phase_name,
                    trace_id: &trace.trace_id,
                    mode: Some(&params.mode),
                    risk_score: None,
                    degrade_policy: None,
                },
                &estimate_token_economy(&params.messages, &response_text),
            )
            .await?;
        }

        // Trace event
        request::append_trace_event(TraceEvent {
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
                .to_string(),
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

        // HarnessBus post-execute
        if let Some(ref harness) = server.governance_deps.harness_bus {
            let output_v = harness.verify_output(&json!({"agent": &selected_agent, "response": &response_text, "reasoning": &reasoning_text, "phase": &resolve_out.phase_name}));
            if !output_v.quality {
                tracing::warn!(target: "harness_bus", risk_score = output_v.risk_score, "post-execute: verification flagged quality issue");
            }
        }
    }

    // O-FIX4: Record global performance metric (cache-hit or fallback-early path)
    let success = !response_text.is_empty() && last_err.is_none();
    let elapsed_ms = act_started.elapsed().as_secs_f64() * 1000.0;
    record_global_operation(success, elapsed_ms);

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
        vote_winner: None,
        vote_report: None,
        used_multi_model_vote: false,
        used_multi_agent_vote: false,
        review_required: false,
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

async fn observe_submit_to_scheduler(
    server: &AcpServer,
    resolved: &crate::orchestration::flow::ResolvedRouting,
    sched_task_id: &str,
) {
    if let Some(ref sched) = server.orchestration_deps.scheduler {
        let submitted_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let role = resolved
            .agents
            .first()
            .map(|(n, _)| n.clone())
            .unwrap_or_else(|| "general".to_string());
        let task = crate::orchestration::scheduler::ScheduledTask {
            task_id: sched_task_id.to_string(),
            role,
            priority: crate::orchestration::scheduler::Priority(100),
            base_score: 1.0,
            urgency: 0.5,
            cost_efficiency: 0.8,
            deadline_pressure: 0.0,
            aging_bonus: 0.0,
            submitted_at,
            retries: 0,
            max_retries: 3,
            provider: None,
        };
        if let Err(e) = sched.level1.submit(task) {
            let s = format!("{}", e);
            if s.contains("backpressure") {
                tracing::warn!("scheduler backpressure: {}", s);
            } else {
                tracing::warn!("scheduler submit failed: {}", s);
            }
        }
    }
}

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
    _params: &ChatParams,
    resolve_out: &ObserveOutput,
    routing_out: &ThinkOutput,
    trace: &RequestTraceContext,
    stream_observer: Option<StreamObserver>,
    _agent_attempts: Vec<Value>,
) -> Result<(FallbackExecutionResult, HighRiskVoteExecutionResult, bool)> {
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
        &mut resolve_out.routing_provenance.clone(),
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
        #[cfg(feature = "multi-users-server")]
        {
            let _ = server
                .rate_limiting
                .tenant_budget
                .lock()
                .map(|mut b| b.record_usage(_tenant_id, 0, 0));
        }
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
            #[cfg(feature = "multi-users-server")]
            {
                let _ = server
                    .rate_limiting
                    .tenant_budget
                    .lock()
                    .map(|mut b| b.record_usage(_tenant_id, 0, 0));
            }
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
        #[cfg(feature = "multi-users-server")]
        {
            let _ = server
                .rate_limiting
                .tenant_budget
                .lock()
                .map(|mut b| b.record_usage(_tenant_id, 0, 0));
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
    let sched_task_id = trace.request_id.clone();

    // Early return for cache-hit / no-execution-needed paths
    if exec_out.cache_hit && !exec_out.response_text.is_empty() {
        return Ok(json!({
            "done": true, "mode": params.mode, "phase": resolve_out.phase_name,
            "phase_origin": resolve_out.phase_origin, "cached": exec_out.cache_hit,
            "agent": exec_out.selected_agent, "response": exec_out.response_text,
        }));
    }

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
        "multi_model_vote_enabled": exec_out.used_multi_model_vote,
        "multi_model_vote_used": exec_out.used_multi_model_vote,
        "multi_agent_vote_enabled": exec_out.used_multi_agent_vote,
        "multi_agent_vote_used": exec_out.used_multi_agent_vote,
        "escalation_enabled": exec_out.review_required,
        "review_required": exec_out.review_required,
        "vote_report": exec_out.vote_report,
    });

    let result = apply_review_gate_assemble(
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
        &Vec::<Value>::new(),
        &sched_task_id,
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
    )
    .await?;

    // Background skill/workflow generation
    // Codex-style: skip for simple chat — no meaningful patterns to extract
    if !is_simple_chat(params) {
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            auto_create_skills_from_conversation(server, params, &exec_out.response_text),
        )
        .await;
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            auto_generate_workflow_from_conversation(server, params, &exec_out.response_text),
        )
        .await;
    }

    // Rationalization
    let (justified, reason) = crate::intelligence::hub::rationalize_decision(
        &exec_out.selected_agent,
        &extract_task_description(&params.messages),
        if exec_out.response_text.is_empty() {
            0.3
        } else {
            0.8
        },
    )
    .await;
    if !justified {
        debug!(
            "rationalize: blocked agent={} reason={}",
            exec_out.selected_agent, reason
        );
    }

    // CapabilityBus feedback
    capability_bus_feedback(
        server,
        trace,
        &resolve_out.phase_name,
        &exec_out.selected_agent,
        &exec_out.response_text,
        &exec_out.last_err,
        params,
        started,
    )
    .await;

    // AgentMemoryBus completion
    store_agent_memory_bus_completion(
        &exec_out.selected_agent,
        resolve_out.user_id.as_deref(),
        &resolve_out.phase_name,
        params,
        &exec_out.response_text,
        &exec_out.last_err,
    )
    .await;

    // BrainLoop post-execution reflection
    if let Some(ref harness) = server.governance_deps.harness_bus {
        let bl = harness.brain_loop.clone();
        let task_type = extract_task_description(&params.messages);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        tokio::spawn(async move {
            let _ = bl
                .run_async(
                    "post-chat-reflection",
                    vec![crate::orchestration::brain_loop::BrainLoopStep {
                        id: format!("post-chat-{}", now_ms),
                        phase: crate::orchestration::brain_loop::BrainLoopPhase::Executing,
                        description: format!("Post-chat reflection: {}", task_type),
                        input: task_type,
                        output: String::new(),
                        started_ms: now_ms,
                        completed_ms: 0,
                        duration_ms: 0,
                        status: crate::orchestration::brain_loop::StepStatus::Done,
                        context: None,
                        depends_on: vec![],
                        mode: "auto".to_string(),
                        agent: None,
                        timeout_seconds: 60,
                        parallel_group: None,
                    }],
                )
                .await;
        });
    }

    // Metacognitive observation
    if let Some(ref cb) = server.governance_deps.capability_bus {
        let success = !exec_out.response_text.is_empty() && exec_out.last_err.is_none();
        let task_desc = extract_task_description(&params.messages);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
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

    // ThresholdLearner
    {
        let success = !exec_out.response_text.is_empty() && exec_out.last_err.is_none();
        if let Ok(mut learner) = CHAT_THRESHOLD_LEARNER
            .get_or_init(|| {
                StdMutex::new(
                    crate::orchestration::threshold_learner::ThresholdLearner::default_learner(),
                )
            })
            .lock()
        {
            learner.record_trial("chat_execution", 0.5, success, !success, false);
        }
    }

    // ── Provenance recording ────────────────────────────────────────
    // Record a high-level provenance entry for this chat execution.
    if let Some(ref ledger) = server.governance_deps.provenance_ledger {
        let _ = ledger
            .record_provenance(
                &trace.trace_id,
                &extract_task_description(&params.messages),
                &exec_out.selected_agent,
                exec_out.last_err.is_none(),
                started.elapsed().as_millis() as u64,
            )
            .await;
    }

    // ── Metacognitive persistence save (fire-and-forget) ──────────────────
    if let Some(ref cb) = server.governance_deps.capability_bus {
        let meta = cb.metacognitive.clone();
        tokio::spawn(async move {
            use crate::intelligence::metacognitive_persistence::MetacognitivePersistence;
            let storage_dir = std::path::PathBuf::from(".goon/metacognitive");
            if let Ok(persistence) = MetacognitivePersistence::new(storage_dir) {
                let _ = persistence.save(&meta);
            }
        });
    }

    // ── TripleFusion fusion cycle (fire-and-forget, non-blocking) ────────
    if let Some(ref cb) = server.governance_deps.capability_bus {
        let meta = cb.metacognitive.clone();
        let cs = cb.consciousness.clone();
        tokio::spawn(async move {
            let fusion_bridge = crate::intelligence::triple_fusion::global_triple_fusion_bridge();
            let triggers = fusion_bridge.lock().await.run_fusion_cycle(&meta, &cs);
            crate::intelligence::fusion_evolution_bridge::send_triggers_to_evolution(triggers);
        });
    }

    // ── Memory bridge: persist reflection outcome (GAP-B54-011) ────────
    // Uses lazy initialization (S1 startup optimization).
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
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .to_string(),
            usefulness: 0.5,
            staleness: 0,
            user_id: None,
        };
        let _ = crate::memory::memory_bridge::bridge_store(
            &server.persistence.memory_store,
            mp.as_ref(),
            entry,
        )
        .await;
    }

    // ── MemoryRetrievalEngine: index session memory (GAP-B52-13) ───────
    // Uses lazy initialization (S1 startup optimization).
    if let Some(engine) = server.get_or_init_memory_retrieval_engine() {
        let _ = engine.index_session_memory(&routing_out.conversation_id, &trace.request_id);
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
    let should_capture = !act_phase_produced_output;

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
            if should_capture {
                // Capture the mode runtime's output back into exec_out so the
                // final "result" SSE event carries the actual response.
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
            if let Some(ref cb) = server.governance_deps.capability_bus {
                let _ = cb.continuous_learning.lock().map(|cl| {
                    cl.schedule_review(&crate::intelligence::continuous_learning::ConsolidatedMemory {
                        id: format!("chat-{}-{}", phase_name, trace.request_id),
                        pattern_key: format!("chat:{}:{}", params.mode,
                            extract_task_description(&params.messages).chars().take(50).collect::<String>()),
                        data: json!({"task": extract_task_description(&params.messages), "agent": &exec_out.selected_agent,
                            "response_length": exec_out.response_text.len(), "mode": &params.mode}).to_string(),
                        importance: 0.5,
                        consolidated_ms: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0),
                        last_accessed_ms: 0, access_count: 1,
                    });
                });
            }
        }
    }

    if matches!(mode_runtime.kind(), ModeKind::Edit)
        && resolved.agents.len() > 1
        && !exec_out.cache_hit
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
    let desc_lower = desc.to_lowercase();

    // Basic task analysis based on description keywords
    let task_type = if desc_lower.contains("test")
        || desc_lower.contains("verify")
        || desc_lower.contains("validate")
    {
        TaskType::TestImplementation
    } else if desc_lower.contains("bug")
        || desc_lower.contains("fix")
        || desc_lower.contains("error")
    {
        TaskType::BugFix
    } else if desc_lower.contains("refactor")
        || desc_lower.contains("clean")
        || desc_lower.contains("optimize")
    {
        TaskType::Refactoring
    } else if desc_lower.contains("doc")
        || desc_lower.contains("readme")
        || desc_lower.contains("comment")
    {
        TaskType::Documentation
    } else if desc_lower.contains("design")
        || desc_lower.contains("architect")
        || desc_lower.contains("plan")
    {
        TaskType::ArchitectureDesign
    } else {
        TaskType::FeatureImplementation
    };

    let complexity = if desc.len() > 500 {
        5
    } else if desc.len() > 200 {
        3
    } else {
        1
    };

    let mut required_capabilities = vec!["coding".to_string()];
    if desc_lower.contains("security")
        || desc_lower.contains("vulnerability")
        || desc_lower.contains("audit")
    {
        required_capabilities.push("security".to_string());
    }
    if desc_lower.contains("data") || desc_lower.contains("database") || desc_lower.contains("sql")
    {
        required_capabilities.push("data".to_string());
    }
    if desc_lower.contains("ui")
        || desc_lower.contains("frontend")
        || desc_lower.contains("gui")
        || desc_lower.contains("web")
    {
        required_capabilities.push("frontend".to_string());
    }

    let involves_multiple_modules = desc_lower.contains("module")
        || desc_lower.contains("component")
        || desc_lower.contains("integration");
    let is_time_critical = desc_lower.contains("urgent")
        || desc_lower.contains("asap")
        || desc_lower.contains("quick");
    let needs_verification = desc_lower.contains("verify")
        || desc_lower.contains("test")
        || desc_lower.contains("validate")
        || desc_lower.contains("ensure");
    let has_safety_concerns = desc_lower.contains("security")
        || desc_lower.contains("sensitive")
        || desc_lower.contains("dangerous");

    let task_chars = TaskCharacteristics {
        description: desc.clone(),
        task_type,
        complexity,
        required_capabilities,
        involves_multiple_modules,
        is_time_critical,
        needs_verification,
        has_safety_concerns,
    };
    let registry = server
        .agent_registry()
        .unwrap_or_else(|| Arc::new(crate::agent::AgentRegistry::new()));
    let pipeline_result = MultiAgentPipeline::new(registry)
        .execute(
            &extract_task_description(&params.messages),
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
        exec_out.cache_hit = true;
        #[cfg(feature = "sub-bus-voter-future")]
        run_multi_model_voter(resolved, &extract_task_description(&params.messages)).await;
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
    started: Instant,
) {
    if let Some(ref cb) = server.governance_deps.capability_bus {
        let success = !response_text.is_empty() && last_err.is_none();
        let duration_ms = started.elapsed().as_millis() as u64;
        let economy = estimate_token_economy(&params.messages, response_text);
        let token_cost_est = economy["total_tokens"].as_u64().unwrap_or(0);
        cb.feedback(
            selected_agent,
            phase_name,
            &trace.request_id,
            success,
            duration_ms,
            token_cost_est,
            if success { 0.8 } else { 0.2 },
        )
        .await;

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

#[cfg(feature = "sub-bus-voter-future")]
async fn run_multi_model_voter(
    resolved: &crate::orchestration::flow::ResolvedRouting,
    task_description: &str,
) {
    use crate::intelligence::multi_model_voter::MultiModelVoter;
    let agents: Vec<Arc<dyn crate::agent::Agent>> =
        resolved.agents.iter().map(|(_, a)| a.clone()).collect();
    if agents.len() > 1 {
        if let Ok(outcome) = MultiModelVoter::new().vote(task_description, &agents).await {
            if outcome.consensus_level < 0.5 {
                tracing::warn!("low-consensus multi-agent vote");
            }
        }
    }
}
