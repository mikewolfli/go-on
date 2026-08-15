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
use serde_json::{json, Value};
use tracing::info;

use crate::acp::helpers::cache_strategy::should_bypass_for_execution;
use crate::acp::r#impl::chat::{
    emit_status_event, evaluate_pre_route_policies, resolve_request_phase, routing_handles,
    ChatParams, ChatRequestContext, StreamObserver,
};
use crate::acp::server::AcpServer;
use crate::agents::agent::Message;

use super::types::ObserveOutput;

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
pub(crate) fn is_simple_chat(params: &ChatParams) -> bool {
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
use super::act::try_semantic_cache_probe;
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
    // waste. Skip them. The probe itself is also disabled when the phase
    // config turns the cache off (`cache_enabled=false`), so the observe
    // path agrees with act_phase's lookup gate.
    let observe_cache_enabled = flow
        .config()
        .phases
        .get(
            params
                .phase
                .as_deref()
                .unwrap_or_else(|| flow.default_phase()),
        )
        .and_then(|p| p.options.as_ref())
        .and_then(|opts| opts.cache_enabled)
        .unwrap_or(true);
    let (skip_expensive, semantic_embedding) = if observe_cache_enabled {
        semantic_prefetch_probe(server, params)
    } else {
        (false, None)
    };
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
        )
        .await?;
    }
    if !skip_expensive && !url_entries.is_empty() {
        emit_status_event(stream_observer, "Pre-fetching URLs...").await?;
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
        semantic_embedding,
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
/// Probe the semantic cache for the last user message and report whether the
/// expensive observe sub-steps (multimodal + URL prefetch) can be skipped
/// because the answer is already cached.
///
/// Returns `(skip, embedding)`: `skip` is the decision (same semantics as the
/// former `semantic_prefetch_should_skip`); `embedding` is the minhash query
/// embedding the probe computed for the similarity scan, if any — handed to
/// the act-phase lookup (via [`ObserveOutput::semantic_embedding`]) so the
/// expensive embedding is computed at most once per request.
fn semantic_prefetch_probe(server: &AcpServer, params: &ChatParams) -> (bool, Option<Vec<f32>>) {
    let probe = match params
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.as_str())
    {
        Some(key) => try_semantic_cache_probe(server, key),
        None => (None, None),
    };
    let semantic_hit = probe.0.is_some();
    let skip =
        semantic_prefetch_should_skip_condition(&params.mode, &params.messages, semantic_hit);
    (skip, probe.1)
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
        // A `data:` prefix without `;base64,` (e.g. `data:text/plain,hello`)
        // must not abort the scan — advance past it and keep looking for
        // valid base64 URIs (previously `break` dropped every later URI).
        let Some((mime, payload_tail)) = after.split_once(";base64,") else {
            rest = &after[after
                .find(|c: char| c.is_whitespace() || c == ')' || c == '>' || c == ']')
                .unwrap_or(after.len())..];
            continue;
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
        // vision/ASR pipeline). Video and document modalities get the same
        // treatment so no modality bypasses the caps.
        let cap = if ml.contains("image") {
            crate::multimodal::MAX_IMAGE_SIZE
        } else if ml.contains("audio") {
            crate::multimodal::MAX_AUDIO_SIZE
        } else if ml.contains("video") {
            crate::multimodal::MAX_VIDEO_SIZE
        } else {
            crate::multimodal::MAX_DOCUMENT_SIZE
        };
        if bytes.len() > cap {
            tracing::warn!(
                mime = %ml,
                len = bytes.len(),
                cap = cap,
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
                    "\n[Audio transcription]:\n{}",
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
    use crate::multimodal::{
        MultimodalInput, MAX_AUDIO_SIZE, MAX_DOCUMENT_SIZE, MAX_IMAGE_SIZE, MAX_VIDEO_SIZE,
    };
    let file_path = content.strip_prefix("file://")?;
    let path = std::path::Path::new(file_path);
    let ext = path.extension().and_then(|e| e.to_str())?;
    let ext_lower = ext.to_lowercase();
    // Enforce the declared per-modality size limits against the file's
    // metadata BEFORE reading, so a model-picked huge file cannot push an
    // unbounded payload into the pipeline (previously the whole file was
    // read first, and video/document modalities had no cap at all).
    let is_image = matches!(
        ext_lower.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
    );
    let is_audio = matches!(ext_lower.as_str(), "mp3" | "wav" | "flac" | "ogg" | "m4a");
    let is_video = matches!(ext_lower.as_str(), "mp4" | "avi" | "mkv" | "mov" | "webm");
    let cap = if is_image {
        MAX_IMAGE_SIZE
    } else if is_audio {
        MAX_AUDIO_SIZE
    } else if is_video {
        MAX_VIDEO_SIZE
    } else {
        MAX_DOCUMENT_SIZE
    };
    match tokio::fs::metadata(path).await {
        Ok(meta) if meta.len() > cap as u64 => {
            tracing::warn!(
                file = %file_path,
                len = meta.len(),
                cap = cap,
                "skipping file:// URI exceeding the declared modality size limit"
            );
            return None;
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(
                file = %file_path,
                error = %e,
                "skipping file:// URI whose metadata could not be read"
            );
            return None;
        }
    }
    let bytes = tokio::fs::read(path).await.ok()?;
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
                "\n[Audio transcription]:\n{}",
                processed.joined_audio()
            ));
        }
        return Some(e);
    }
    None
}

// ═════════════════════════════════════════════════════════════════════

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
