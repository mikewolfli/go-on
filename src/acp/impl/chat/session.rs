//! Session-level chat handling
//!
//! Contains the top-level `handle_chat` function along with its private
//! helper functions (`infer_optimal_mode`, `send_error`, `send_result`,
//! `record_trace_event`).  Extracted from the parent `chat.rs` to reduce
//! the monolithic file size.

use std::sync::atomic::{AtomicU64, Ordering};
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
use super::{process_chat_request, should_escalate_approval_strategy};

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
        let lifecycle_snapshot = {
            let lifecycle_guard =
                server
                    .resilience
                    .lifecycle_state
                    .read()
                    .unwrap_or_else(|poisoned| {
                        warn!("handle_chat: lifecycle_state poisoned, recovering");
                        poisoned.into_inner()
                    });
            if lifecycle_guard.shutdown_requested() {
                Some(serde_json::to_value(lifecycle_guard.snapshot())?)
            } else {
                None
            }
        };
        if let Some(snapshot) = lifecycle_snapshot {
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
        let mode_lower = chat_params.mode.trim().to_ascii_lowercase();
        match mode_lower.as_str() {
            "ask" | "plan" | "edit" | "agent" | "full_auto" | "safeguard" | "safe_guard"
            | "fullauto" => {
                info!("chat mode validated: '{}'", chat_params.mode);
            }
            other => {
                warn!(
                    "unrecognized mode '{}' from client, defaulting to 'ask'",
                    other
                );
                chat_params.mode = "edit".to_string();
            }
        }

        // GAP-46-12: Track session context across requests.
        // Use SessionContextManager to extract key concepts from the conversation
        // and maintain continuity markers for long-running sessions.
        let mut session_mgr = SessionContextManager::default();
        let conversation_id = chat_params
            .conversation_id
            .clone()
            .unwrap_or_else(|| "default".to_string());
        let msg_count = chat_params.messages.len();
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
        // If the conversation is long, compute trim budget and apply it.
        if msg_count > 50 {
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
                    warn!(
                        "SessionContextManager: compression reduced {}→{} messages (ratio: {:.2})",
                        compressed.original_count,
                        compressed.compressed_count,
                        compressed.compression_ratio,
                    );
                    let kept_count = compressed.kept_messages.len();
                    let orig_count = compressed.original_count;
                    let summary_text = compressed.summary.clone();
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
                                orig_count - kept_count,
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

        // Check if should escalate approval strategy
        let should_escalate = should_escalate_approval_strategy(
            server,
            &chat_params.mode,
            &chat_params.messages,
            chat_params.conversation_id.as_deref(),
            chat_params.phase.as_deref(),
            chat_params.options.as_ref(),
        )
        .await?;

        if should_escalate {
            info!(
                trace_id = %pipeline_trace.trace_id,
                "approval strategy escalated due to policy"
            );
            // Handle escalation logic here
            // This will be implemented when we migrate the escalation logic
        }

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

        // If using an external SSE observer, send the final result as a stream frame
        // so dispatch_to_client can forward it. Otherwise send as a JSON-RPC result.
        if let Some(sender) = observer.sse_sender() {
            use crate::acp::r#impl::chat::streaming::StreamFrame;
            let _ = sender.send(StreamFrame {
                event: "result",
                payload: json!(result),
                status: None,
            });
        } else {
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
    {
        let mut alert_mgr = server
            .observability
            .alert_manager
            .lock()
            .unwrap_or_else(|poisoned| {
                warn!("handle_chat: alert_manager poisoned, recovering");
                poisoned.into_inner()
            });
        let fired = alert_mgr.evaluate("chat_latency_ms", duration_ms as f64);
        for alert in &fired {
            tracing::warn!(
                target = "alert_manager",
                rule = %alert.rule,
                severity = %alert.severity,
                value = %alert.value,
                threshold = %alert.threshold,
                "AlertManager: {}", alert.message
            );
        }
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
fn infer_optimal_mode(messages: &[Message], _server: &AcpServer) -> String {
    // Collect all user message text
    let corpus: String = messages
        .iter()
        .filter(|m| m.role.eq_ignore_ascii_case("user"))
        .map(|m| &m.content[..])
        .collect::<Vec<_>>()
        .join("\n");
    let lower = corpus.to_lowercase();
    let word_count = lower.split_whitespace().count().max(1);

    // Quick mode steering based on conversation structure
    let has_multiple_turns = messages.len() > 4;
    let last_user = messages
        .iter()
        .rfind(|m| m.role.eq_ignore_ascii_case("user"));
    let last_user_len = last_user.map(|m| m.content.len()).unwrap_or(0);

    // ── Vote early: short / ambiguous user inputs → "edit" ──────────────
    if last_user_len > 0 && last_user_len < 15 {
        return "edit".to_string();
    }

    // ── Density-based routing ──────────────────────────────────────────
    // Break the corpus into words so we can measure the ratio of
    // imperative / analytical tokens, which is a strong signal for
    // the intended mode.
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
    ];
    let analytical_indicators = [
        "analyze",
        "explain",
        "debug",
        "investigate",
        "review",
        "check",
        "verify",
        "test",
        "why",
        "how",
        "what",
    ];

    let imperative_count = imperative_triggers
        .iter()
        .filter(|t| lower.contains(*t))
        .count();
    let planning_count = planning_triggers
        .iter()
        .filter(|t| lower.contains(*t))
        .count();
    let analytical_count = analytical_indicators
        .iter()
        .filter(|t| lower.contains(*t))
        .count();

    // Weighted heuristic (imperative > planning > analytical)
    let imperative_score = (imperative_count as f64) * 2.0;
    let planning_score = (planning_count as f64) * 1.5;
    let analytical_score = analytical_count as f64;

    // ── Adjust for conversation length ─────────────────────────────────
    let length_factor = (word_count as f64 / 100.0).min(3.0);
    let imperative_score = imperative_score * length_factor;
    let planning_score = planning_score * length_factor;
    let analytical_score = analytical_score * length_factor;

    // ── Score entry threshold ──────────────────────────────────────────
    // Require at least one trigger match to override the default.
    let total_triggers = imperative_count + planning_count + analytical_count;

    if total_triggers == 0 {
        // Fallback: if conversation has multiple turns, edit is the default
        if has_multiple_turns || word_count > 30 {
            return "edit".to_string();
        }
        return "edit".to_string();
    }

    // ── Multi-turn conversations with planning → "edit" ────────────────
    // If the user has sent multiple messages and any of them mention
    // planning or imperative keywords, route to edit for granular
    // multi-file workflows.
    if has_multiple_turns && (planning_score > 0.0 || imperative_score > 0.0) {
        return "edit".to_string();
    }

    // ── Route by highest score ─────────────────────────────────────────
    if planning_score >= imperative_score && planning_score >= analytical_score {
        "edit"
    } else if imperative_score >= analytical_score {
        "agent"
    } else {
        "edit"
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
#[allow(clippy::too_many_arguments)]
fn record_trace_event(
    _server: &AcpServer,
    _trace: &RequestTraceContext,
    event_type: &str,
    status: &str,
    stage: &str,
    _inputs: Value,
    _outputs: Option<Value>,
    _duration_ms: u64,
) {
    static EVENTS_RECEIVED: AtomicU64 = AtomicU64::new(0);
    let count = EVENTS_RECEIVED.fetch_add(1, Ordering::Relaxed);

    debug!(
        event_type = %event_type,
        status = %status,
        stage = %stage,
        events_received = count,
        "record_trace_event"
    );
}
