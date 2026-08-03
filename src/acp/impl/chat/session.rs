//! Session-level chat handling
//!
//! Contains the top-level `handle_chat` function along with its private
//! helper functions (`infer_optimal_mode`, `send_error`, `send_result`,
//! `record_trace_event`).  Extracted from the parent `chat.rs` to reduce
//! the monolithic file size.

use std::time::Instant;

use anyhow::Result;
use opentelemetry::{Context as OtelContext, KeyValue};
use serde_json::{json, Value};
use tracing::{debug, info, warn};

use crate::acp::server::AcpServer;
use crate::agent::Message;
use crate::i18n::runtime::{t, tf};
use crate::orchestration::session_compressor::SessionCompressor;
use crate::orchestration::session_context::SessionContextManager;
use crate::rpc_protocol::{chat_trace_context, child_trace_context, RequestTraceContext};

use super::params::ChatParams;
use super::streaming::StreamObserver;
use super::{check_server_shutdown, evaluate_pre_chat_gates, process_chat_request};

// ---------------------------------------------------------------------------
// handle_chat
// ---------------------------------------------------------------------------

/// Handle chat request
///
/// This function replaces the `AcpServer::handle_chat` method.
pub(crate) async fn handle_chat(
    server: &AcpServer,
    id: Option<Value>,
    params: Option<Value>,
    request_span: Option<OtelContext>,
    parent_trace: Option<RequestTraceContext>,
    stream_observer: Option<StreamObserver>,
) -> Result<()> {
    let started = Instant::now();
    let pipeline_trace = parent_trace
        .map(|trace| child_trace_context(&trace, "chat.pipeline"))
        .unwrap_or_else(|| chat_trace_context(&id, "chat.pipeline"));

    info!(
        trace_id = %pipeline_trace.trace_id,
        "pipeline entry: chat request received"
    );

    let chat_span = request_span.as_ref().and_then(|parent| {
        server
            .observability
            .telemetry_runtime
            .lock()
            .unwrap_or_else(|poisoned| {
                warn!("handle_chat: telemetry_runtime poisoned, recovering");
                poisoned.into_inner()
            })
            .start_child_span(
                parent,
                "acp.chat",
                vec![KeyValue::new("phase.entry", "chat")],
            )
    });

    let result = async {
        // Shared lifecycle gate — reject requests while the server is
        // shutting down (same gate is applied by the session/prompt entry).
        if let Some(snapshot) = check_server_shutdown(server).await? {
            send_error(
                server,
                id,
                -32031,
                t("error.chat.server_shutting_down"),
                Some(snapshot),
            )
            .await?;
            return Ok(());
        }

        let params_value = params.unwrap_or_else(|| json!({}));
        let mut chat_params: ChatParams = match serde_json::from_value(params_value) {
            Ok(value) => value,
            Err(err) => {
                send_error(
                    server,
                    id,
                    -32602,
                    tf("error.invalid_chat_params", &[("error", &format!("{err}"))]),
                    None,
                )
                .await?;
                return Ok(());
            }
        };

        // BLUE48-AUTO: Intelligent mode selection when mode is absent, empty, or "auto".
        // - Client passes a specific mode ("ask"/"edit"/"agent"/"safeguard"/"full_auto"): use it as-is
        // - Client passes nothing or "auto": analyze input to select optimal mode
        if chat_params.mode.trim().is_empty()
            || chat_params.mode.trim().eq_ignore_ascii_case("auto")
        {
            let auto_mode = infer_optimal_mode(&chat_params.messages, server);
            chat_params.mode = auto_mode;
            info!(
                "mode not specified by client, auto-selected mode='{}'",
                chat_params.mode
            );
        }

        // Validate the resolved mode is a recognized value before dispatching.
        // (Shared with the session/prompt entry via evaluate_pre_chat_gates.)
        if let crate::acp::r#impl::chat::PreChatGate::EscalationRequired { mode } =
            evaluate_pre_chat_gates(server, &mut chat_params).await?
        {
            info!(
                trace_id = %pipeline_trace.trace_id,
                "approval strategy escalated due to policy — rejecting request"
            );
            send_error(
                server,
                id,
                -32040,
                t("error.chat.escalation_required"),
                Some(serde_json::json!({
                    "reason": "Request requires human approval per governance policy",
                    "mode": mode,
                })),
            )
            .await?;
            return Ok(());
        }

        // Determine which observer to use — external (SSE) or internal (JSON-RPC).

        // GAP-46-12: Track session context across requests.
        // Concept extraction only benefits long conversations — for ordinary
        // conversations (<50 messages) the extracted data was discarded after
        // a debug log. The manager is therefore built lazily inside the trim
        // branch below.
        let conversation_id = chat_params
            .conversation_id
            .clone()
            .unwrap_or_else(|| "default".to_string());
        let msg_count = chat_params.messages.len();
        // If the conversation is long, extract key concepts and apply trim budget.
        if msg_count > 50 {
            let mut session_mgr = SessionContextManager::default();
            debug!(
                "SessionContextManager: tracking conversation '{}' with {} messages",
                conversation_id, msg_count
            );
            // Record each message for context extraction.
            for msg in &chat_params.messages {
                session_mgr.record_message(&msg.content);
            }
            let concept_count = session_mgr.concept_count();
            let decision_count = session_mgr.decision_count();
            if concept_count > 0 || decision_count > 0 {
                debug!(
                    "SessionContextManager: {} concepts, {} decisions extracted",
                    concept_count, decision_count
                );
            }
            let effective = session_mgr.budget.effective_retain();
            debug!(
                "SessionContextManager: effective retain budget for {} messages = {}",
                msg_count, effective
            );

            // Convert messages to the tuple format expected by select_retained_messages.
            let msg_tuples: Vec<(String, String)> = chat_params
                .messages
                .iter()
                .map(|m| (m.role.clone(), m.content.clone()))
                .collect();

            let retained_indices = session_mgr.select_retained_messages(&msg_tuples, effective);

            // If messages heavily exceed budget (50%+ over), try semantic
            // compression as an alternative to simple trimming.
            let compression_applied = if msg_count > effective * 3 / 2 {
                let compressor = SessionCompressor::default();
                let compressed = session_mgr.compress_messages(&msg_tuples, &compressor);
                if !compressed.summary.is_empty() {
                    let original_count = compressed.original_count;
                    let compressed_count = compressed.compressed_count;
                    let compression_ratio = compressed.compression_ratio;
                    let kept_count = compressed.kept_messages.len();
                    let summary_text = compressed.summary.clone();

                    warn!(
                        "SessionContextManager: compression reduced {}→{} messages (ratio: {:.2})",
                        original_count, compressed_count, compression_ratio,
                    );

                    // Convert compressor messages back to agent messages.
                    let mut compressed_msgs: Vec<Message> = compressed
                        .kept_messages
                        .into_iter()
                        .map(|m| Message {
                            role: m.role,
                            content: m.content,
                        })
                        .collect();
                    // Prepend the summary as a system message.
                    compressed_msgs.insert(
                        0,
                        Message {
                            role: "system".to_string(),
                            content: format!(
                                "[Session compressed: {} messages summarized]\n{}",
                                original_count - kept_count,
                                summary_text,
                            ),
                        },
                    );
                    chat_params.messages = compressed_msgs;
                    true
                } else {
                    false
                }
            } else {
                false
            };

            if !compression_applied && retained_indices.len() < msg_count {
                let trimmed_count = msg_count - retained_indices.len();
                warn!(
                    "SessionContextManager: trimming {} of {} messages (retaining {})",
                    trimmed_count,
                    msg_count,
                    retained_indices.len(),
                );

                // Generate continuity marker for the trimmed messages.
                let trimmed_indices: Vec<usize> = (0..msg_count)
                    .filter(|i| !retained_indices.contains(i))
                    .collect();
                let marker = session_mgr.generate_continuity_marker(&trimmed_indices);

                // Build a concise continuity marker text for the LLM.
                let marker_text = format!(
                    "[Continuity: {} messages trimmed to fit context window]\n\
                     Key concepts: {}\n\
                     Files referenced: {}\n\
                     Decisions made: {}",
                    marker.messages_trimmed,
                    if marker.key_concepts.is_empty() {
                        "(none)".to_string()
                    } else {
                        marker.key_concepts.join(", ")
                    },
                    if marker.files_referenced.is_empty() {
                        "(none)".to_string()
                    } else {
                        marker.files_referenced.join(", ")
                    },
                    if marker.decisions_made.is_empty() {
                        "(none)".to_string()
                    } else {
                        marker.decisions_made.join(", ")
                    },
                );

                // Rebuild the message list from retained indices only.
                chat_params.messages = retained_indices
                    .iter()
                    .map(|&i| chat_params.messages[i].clone())
                    .collect();

                // Prepend the continuity marker as a system message so the LLM
                // knows what context was trimmed and can reference it if needed.
                chat_params.messages.insert(
                    0,
                    Message {
                        role: "system".to_string(),
                        content: marker_text,
                    },
                );
            }
        }

        // Check if should escalate approval strategy — shared gate with the
        // session/prompt entry (evaluate_pre_chat_gates above).
        //
        // Determine which observer to use — external (SSE) or internal (JSON-RPC).
        let observer = stream_observer.unwrap_or_else(|| StreamObserver::jsonrpc(id.clone()));

        // Process chat request
        let result = process_chat_request(
            server,
            &mut chat_params,
            Some(observer.clone()),
            &pipeline_trace,
            chat_span.as_ref(),
            None,
        )
        .await?;

        // `process_chat_request` already emits the final "result" stream frame
        // through the SSE observer channel (payload: {response, agent, done}),
        // which `dispatch_to_client` forwards as the JSON-RPC response. Sending
        // a second "result" frame here produced a duplicate JSON-RPC response
        // with the same id. Only the non-SSE (jsonrpc-observer) case needs an
        // explicit result here.
        if observer.sse_sender().is_none() {
            send_result(server, id, json!(result)).await?;
        }

        Ok(())
    }
    .await;

    // Record trace event
    let duration_ms = started.elapsed().as_millis() as u64;
    let status = if result.is_ok() { "success" } else { "error" };

    server
        .observability
        .metrics
        .record_chat_latency(duration_ms as f64);

    // ── AlertManager evaluation ─────────────────────────────────────────
    // NOTE: chat_latency_ms evaluate was removed — no alert rule matches the
    // "chat" keyword prefix, so it always returned empty (dead per-request
    // lock + scan). Only the cache-hit-ratio rule is relevant here.
    {
        let mut alert_mgr = server
            .observability
            .alert_manager
            .lock()
            .unwrap_or_else(|poisoned| {
                warn!("handle_chat: alert_manager poisoned, recovering");
                poisoned.into_inner()
            });
        // Also evaluate cache hit ratio if applicable
        if let Ok(stats) = server.cache_deps.cache.semantic_cache.read() {
            let s = stats.stats();
            if s.total_hits + s.total_misses > 0 {
                let ratio = s.hit_ratio * 100.0;
                let cache_fired = alert_mgr.evaluate("cache_hit_ratio_pct", ratio);
                for alert in &cache_fired {
                    tracing::warn!(
                        target = "alert_manager",
                        rule = %alert.rule,
                        severity = %alert.severity,
                        value = %alert.value,
                        threshold = %alert.threshold,
                        "AlertManager: {}", alert.message
                    );
                }
            }
        }
    }

    record_trace_event(
        server,
        &pipeline_trace,
        "chat.complete",
        status,
        "pipeline",
        json!({}),
        None,
        duration_ms,
    );

    result
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Infer the optimal chat mode from the message content.
///
/// Returns one of: `"ask"`, `"plan"`, `"edit"`, `"agent"`, `"safeguard"`, `"full_auto"`.
fn infer_optimal_mode(messages: &[Message], _server: &AcpServer) -> String {
    // Collect all user message text
    let corpus: String = messages
        .iter()
        .filter(|m| m.role.eq_ignore_ascii_case("user"))
        .map(|m| &m.content[..])
        .collect::<Vec<_>>()
        .join("\n");
    let lower = corpus.to_lowercase();
    let trimmed = lower.trim();
    let word_count = trimmed.split_whitespace().count();
    let char_count = trimmed.chars().count();

    // ----------------------------------------------------------------
    // Edge cases
    // ----------------------------------------------------------------

    // Empty or whitespace-only → ask
    if trimmed.is_empty() || word_count == 0 {
        return "ask".to_string();
    }

    // Pure symbols / no alphabetic characters → ask
    if !trimmed.chars().any(|c| c.is_alphabetic()) {
        return "ask".to_string();
    }

    // Very short messages (1-2 words, ≤12 chars) → ask
    if word_count <= 2 && char_count <= 12 {
        return "ask".to_string();
    }

    // ----------------------------------------------------------------
    // Single-turn / question detection
    // ----------------------------------------------------------------

    let last_user = messages
        .iter()
        .rfind(|m| m.role.eq_ignore_ascii_case("user"));
    let last_user_text = last_user.map(|m| m.content.as_str()).unwrap_or("");
    let ends_with_question = last_user_text.trim().ends_with('?');

    // Question / conversational words
    let question_words = [
        "what", "why", "how", "when", "where", "who", "which", "is", "are", "can", "could",
        "would", "should", "do", "does", "did",
    ];
    let question_word_count = question_words.iter().filter(|w| lower.contains(*w)).count();

    // Analytical / explanatory keywords
    let analytical_indicators = [
        "analyze",
        "explain",
        "debug",
        "investigate",
        "review",
        "check",
        "verify",
        "test",
        "describe",
        "define",
        "meaning",
        "difference",
        "tell me",
        "clarify",
        "summarize",
        "understand",
    ];
    let analytical_count = analytical_indicators
        .iter()
        .filter(|t| lower.contains(*t))
        .count();

    let is_single_turn = messages.len() <= 2;

    // Ends with ? and is short → ask
    if ends_with_question && word_count <= 15 {
        return "ask".to_string();
    }

    // Multiple analytical + question words, short message → ask
    if analytical_count >= 2 && question_word_count >= 2 && word_count <= 50 {
        return "ask".to_string();
    }

    // Single turn, analytical, short → ask
    if is_single_turn && analytical_count >= 1 && word_count <= 30 {
        return "ask".to_string();
    }

    // Single turn with mostly question words, even if longer → ask
    if is_single_turn && question_word_count >= 3 && analytical_count == 0 && word_count <= 40 {
        return "ask".to_string();
    }

    // ----------------------------------------------------------------
    // SafeGuard detection — dangerous operations needing human review
    // ----------------------------------------------------------------

    let has_destructive = [
        "delete prod",
        "drop table",
        "rm -rf",
        "shutdown",
        "destroy",
        "terminate instance",
        "remove all",
        "delete all",
        "truncate",
        "format disk",
        "wipe",
    ]
    .iter()
    .any(|t| lower.contains(t));

    let has_safety_marker = [
        "careful",
        "safely",
        "backup",
        "back up",
        "caution",
        "confirm first",
        "ask before",
        "approve",
        "review before",
        "safe",
        "safeguard",
    ]
    .iter()
    .any(|t| lower.contains(t));

    if has_destructive || has_safety_marker {
        return "safeguard".to_string();
    }

    // ----------------------------------------------------------------
    // FullAuto detection — multi-step automation
    // ----------------------------------------------------------------

    let auto_triggers = [
        "automate",
        "automated",
        "automation",
        "pipeline",
        "batch",
        "multiple steps",
        "full auto",
        "fully automatic",
        "run all",
        "ci/cd",
        "deploy",
        "release process",
        "unattended",
    ];
    let auto_count = auto_triggers.iter().filter(|t| lower.contains(*t)).count();
    let has_multi_step =
        lower.contains("first") && lower.contains("then") && lower.contains("finally");

    if auto_count >= 2 || (auto_count >= 1 && word_count > 30) || has_multi_step {
        return "full_auto".to_string();
    }

    // ----------------------------------------------------------------
    // Density-based routing (imperative / planning / analytical)
    // ----------------------------------------------------------------

    let imperative_triggers = [
        "create",
        "build",
        "make",
        "write",
        "implement",
        "add",
        "update",
        "fix",
        "refactor",
        "change",
        "modify",
        "remove",
        "delete",
        "replace",
        "rename",
    ];
    let planning_triggers = [
        "plan",
        "design",
        "architecture",
        "decide",
        "strategy",
        "approach",
        "consider",
        "evaluate",
        "compare",
        "propose",
        "outline",
        "structure",
    ];

    let imperative_count = imperative_triggers
        .iter()
        .filter(|t| lower.contains(*t))
        .count();
    let planning_count = planning_triggers
        .iter()
        .filter(|t| lower.contains(*t))
        .count();

    // Combined trigger pool for the entry threshold check
    let total_triggers = imperative_count + planning_count + analytical_count;

    // Weighted heuristic (imperative > planning > analytical)
    let imperative_score = (imperative_count as f64) * 2.0;
    let planning_score = (planning_count as f64) * 1.5;
    let analytical_score = analytical_count as f64;

    // Adjust for conversation length
    let length_factor = (word_count as f64 / 100.0).min(3.0);
    let imperative_score = imperative_score * length_factor;
    let planning_score = planning_score * length_factor;
    let analytical_score = analytical_score * length_factor;

    let has_multiple_turns = messages.len() > 4;
    let has_strong_imperative = imperative_triggers
        .iter()
        .any(|t| lower.starts_with(t) || lower.contains(&format!(" {} ", t)));

    // No trigger matches at all — fall back on heuristics
    if total_triggers == 0 {
        if is_single_turn && word_count <= 10 {
            return "ask".to_string();
        }
        if has_multiple_turns || word_count > 30 {
            return "edit".to_string();
        }
        return "ask".to_string();
    }

    // ----------------------------------------------------------------
    // Multi-turn conversations with planning → edit
    // ----------------------------------------------------------------
    if has_multiple_turns && (planning_score > 0.0 || imperative_score > 0.0) {
        return "edit".to_string();
    }

    // ----------------------------------------------------------------
    // Route by highest score
    // ----------------------------------------------------------------

    // Plan mode: planning clearly dominates, no strong imperative
    if planning_score > imperative_score
        && planning_score > analytical_score
        && !has_strong_imperative
    {
        return "plan".to_string();
    }

    // Edit or agent: strong imperative present
    if has_strong_imperative {
        return "edit".to_string();
    }

    // Agent mode: imperative score highest
    if imperative_score >= planning_score && imperative_score >= analytical_score {
        return "agent".to_string();
    }

    // ----------------------------------------------------------------
    // Final fallback (rarely reached)
    // ----------------------------------------------------------------
    if analytical_score > 0.0 {
        "ask"
    } else if word_count > 10 {
        "edit"
    } else {
        "ask"
    }
    .to_string()
}

/// Send error response
async fn send_error(
    server: &AcpServer,
    id: Option<Value>,
    code: i32,
    message: String,
    data: Option<Value>,
) -> Result<()> {
    crate::acp::r#impl::io::send_error(server, id, code, message, data).await
}

/// Send result response
async fn send_result(server: &AcpServer, id: Option<Value>, result: Value) -> Result<()> {
    crate::acp::r#impl::io::send_result(server, id, result).await
}

/// Record a trace event with metrics counter.
///
/// Delegates to the real trace sink in `trace_pack.rs` so chat lifecycle
/// events (e.g. `chat.complete`) appear in `trace.get` alongside the events
/// recorded by other request handlers.
#[allow(clippy::too_many_arguments)]
fn record_trace_event(
    server: &AcpServer,
    trace: &RequestTraceContext,
    event_type: &str,
    status: &str,
    stage: &str,
    inputs: Value,
    outputs: Option<Value>,
    duration_ms: u64,
) {
    crate::acp::r#impl::request::record_trace_event(
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
