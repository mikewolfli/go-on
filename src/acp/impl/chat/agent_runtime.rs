//! Agent runtime loop — collecting streamed responses from an agent
//!
//! Contains `collect_agent_responses` (shared core for stream collection)
//! and `run_agent_collecting` (ACP-specific tool execution + followup).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::warn;

use crate::acp::helpers::context::run_with_optional_timeout;
use crate::acp::server::AcpServer;
use crate::agent::Message;
use crate::i18n::runtime::{t, tf};
use crate::orchestration::autonomy_runtime::{
    build_tool_execution_followup_message, build_tool_result_block, parse_tool_call_token,
    REASONING_END, REASONING_START, TOKEN_THINKING_PREFIX,
};
use crate::orchestration::tool::executor::{execute_tools_concurrent, ToolExecConfig};

use super::streaming::{
    emit_stream_chunk, emit_stream_done, StreamEventMeta, StreamNotificationContext,
};

/// Collected result from streaming agent responses.
pub(crate) struct CollectedResponse {
    pub response: String,
    pub reasoning: String,
    pub tool_calls: Vec<(String, String)>,
}

/// Core streaming collection: reads tokens from the receiver, classifies them
/// (tool calls, reasoning markers, thinking tokens, model-used announcements),
/// and returns the collected response, reasoning text, tool calls, and optional
/// model info.
///
/// Callers handle their own progressive rendering before/after calling this
/// function. For example, the CLI caller can run a pre-loop that displays
/// tokens progressively, then re-collects the stream into this function.
pub(crate) async fn collect_agent_responses(
    mut receiver: mpsc::UnboundedReceiver<String>,
) -> Result<CollectedResponse> {
    let mut response = String::new();
    let mut reasoning_buffer = String::new();
    let mut tool_calls = Vec::new();

    while let Some(token) = receiver.recv().await {
        // Check for model-used token (prefixed with __model_used__)
        if token.strip_prefix("__model_used__:").is_some() {
            continue;
        }

        // Check for tool call tokens using shared parser
        if let Some((tool_name, tool_args)) = parse_tool_call_token(&token) {
            tool_calls.push((tool_name.to_string(), tool_args.to_string()));
            continue;
        }

        // Check for structured reasoning markers (control chars)
        if token == REASONING_START || token == REASONING_END {
            continue;
        }

        // Check for reasoning tokens (prefixed with __thinking__)
        if let Some(reasoning_token) = token.strip_prefix(TOKEN_THINKING_PREFIX) {
            reasoning_buffer.push_str(reasoning_token);
            continue;
        }

        // Skip finish_reason and usage telemetry control tokens
        if token.starts_with("__finish_reason__:") || token.starts_with("__usage__:") {
            continue;
        }

        response.push_str(&token);
    }

    // If the agent produced only reasoning (no content), use the
    // reasoning as the response text.
    if response.trim().is_empty() && !reasoning_buffer.trim().is_empty() {
        response = std::mem::take(&mut reasoning_buffer);
    }

    Ok(CollectedResponse {
        response,
        reasoning: reasoning_buffer,
        tool_calls,
    })
}

/// Calls an agent, collects its streamed response with ACP-specific
/// progressive SSE emission, then handles tool execution and followup.
///
/// Returns `(response_text, reasoning_text, selected_model)`.
pub(crate) async fn run_agent_collecting(
    server: &AcpServer,
    stream_ctx: StreamNotificationContext<'_>,
    agent: Arc<dyn crate::agent::Agent>,
    messages: Vec<Message>,
    principles: Option<Vec<String>>,
    options: Option<HashMap<String, Value>>,
    timeout_duration: Option<Duration>,
) -> Result<(String, String, Option<String>)> {
    // tool execution delegated to execute_tools_concurrent
    let base_messages = messages.clone();
    let followup_agent = Arc::clone(&agent);
    let followup_principles = principles.clone();
    let followup_options = options.clone();

    let (sender, mut receiver) = mpsc::unbounded_channel::<String>();
    let sender = crate::agent::StreamingSender::from(sender);
    let task = tokio::spawn(async move { agent.chat(messages, principles, options, sender).await });

    let collect = async move {
        let stream_started = Instant::now();
        let mut response = String::new();
        let mut reasoning_buffer = String::new();
        let mut tool_calls: Vec<(String, String)> = Vec::new();
        let mut chunk_index = 0usize;
        let mut total_chars = 0usize;
        let mut selected_model: Option<String> = None;
        // Wrap recv() in a per-chunk timeout so a stuck agent cannot
        // hang the pipeline indefinitely even when no outer timeout is set.
        let overall_timeout = std::time::Duration::from_secs(600); // 10 min hard cap
        let recv_timeout = std::time::Duration::from_secs(120);
        loop {
            // Check overall timeout before each recv attempt.
            if stream_started.elapsed() > overall_timeout {
                tracing::warn!(
                    "agent streaming overall timeout after {}s — aborting collect",
                    overall_timeout.as_secs()
                );
                break;
            }
            // Use min of per-chunk timeout and remaining overall timeout.
            let remaining = overall_timeout
                .checked_sub(stream_started.elapsed())
                .unwrap_or(std::time::Duration::from_secs(1));
            let chunk_timeout = recv_timeout.min(remaining);
            let token = match tokio::time::timeout(chunk_timeout, receiver.recv()).await {
                Ok(Some(t)) => t,
                Ok(None) => break,
                Err(_) => {
                    tracing::warn!(
                        "agent streaming recv() timed out after {}s — aborting collect",
                        recv_timeout.as_secs()
                    );
                    break;
                }
            };
            // ── Token classification ──
            // Model-used token
            if let Some(model_id) = token.strip_prefix("__model_used__:") {
                selected_model = Some(model_id.trim().to_string());
                continue;
            }
            // Tool call token
            if let Some((tool_name, tool_args)) = parse_tool_call_token(&token) {
                tool_calls.push((tool_name.to_string(), tool_args.to_string()));
                continue;
            }
            // Stream limits check
            let next_chars = token.chars().count();
            if crate::acp::helpers::conversation::stream_would_exceed_limits(
                chunk_index,
                total_chars,
                next_chars,
            ) {
                anyhow::bail!(t("error.chat.stream_output_limits"));
            }
            // Reasoning markers
            if token == REASONING_START || token == REASONING_END {
                continue;
            }
            // Thinking tokens (prefixed with __thinking__)
            if let Some(reasoning_token) = token.strip_prefix(TOKEN_THINKING_PREFIX) {
                reasoning_buffer.push_str(reasoning_token);
                // Fall through to emit_stream_chunk — it will split
                // the token into display_token="" and reasoning_token.
            } else if token.starts_with("__finish_reason__:") || token.starts_with("__usage__:") {
                // Skip finish_reason and usage telemetry tokens.
                // They should not be appended to the response text
                // nor emitted as display tokens in the SSE stream.
                continue;
            } else {
                response.push_str(&token);
            }

            chunk_index += 1;
            total_chars += next_chars;

            // ACP-specific: emit SSE stream chunk progressively
            emit_stream_chunk(
                server,
                stream_ctx.stream_observer.as_ref(),
                StreamEventMeta {
                    agent_name: stream_ctx.agent_name,
                    phase_name: stream_ctx.phase_name,
                    trace_id: stream_ctx.trace_id,
                    mode: None,
                    risk_score: None,
                    degrade_policy: None,
                },
                &token,
                chunk_index,
                total_chars,
            )
            .await?;
        }

        match task.await {
            Ok(Ok(())) => {
                emit_stream_done(
                    server,
                    stream_ctx.stream_observer.as_ref(),
                    StreamEventMeta {
                        agent_name: stream_ctx.agent_name,
                        phase_name: stream_ctx.phase_name,
                        trace_id: stream_ctx.trace_id,
                        mode: None,
                        risk_score: None,
                        degrade_policy: None,
                    },
                    chunk_index,
                    total_chars,
                    stream_started.elapsed().as_millis() as u64,
                    selected_model.clone(),
                    Some(&response),
                )
                .await?;
                // ── Execute tool calls ────────────────────────────────
                const MAX_TOOL_CALLS_PER_AGENT: usize = 100;
                // ── Skill dedup ──
                let tool_calls = {
                    let is_builtin = |name: &str| -> bool {
                        name == "skill-finder"
                            || name == "skill-creator"
                            || name == "acp_trace_get"
                            || name == "acp_debug_panel_get"
                            || name.starts_with("goon_")
                    };
                    let skill_names: Vec<&str> = tool_calls
                        .iter()
                        .filter(|(name, _)| !is_builtin(name))
                        .map(|(name, _)| name.as_str())
                        .collect();
                    if skill_names.len() > 1 {
                        let best = {
                            let reg = server
                                .orchestration_deps
                                .skill_registry
                                .read()
                                .unwrap_or_else(|poisoned| {
                                    warn!(
                                        "run_agent_collecting: skill_registry poisoned, recovering"
                                    );
                                    poisoned.into_inner()
                                });
                            skill_names
                                .iter()
                                .filter_map(|name| {
                                    let score = reg.score_of(name).unwrap_or(0.5);
                                    reg.get(name).map(|_| (name.to_string(), score))
                                })
                                .max_by(|a, b| {
                                    a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
                                })
                        };
                        if let Some((best_name, _)) = best {
                            warn!(
                                "skill dedup: AI called {} skills ({}), auto-selecting '{}'",
                                skill_names.len(),
                                skill_names.join(", "),
                                best_name
                            );
                            tool_calls
                                .into_iter()
                                .filter(|(name, _)| *name == best_name)
                                .collect::<Vec<_>>()
                        } else {
                            tool_calls
                        }
                    } else {
                        tool_calls
                    }
                };

                if tool_calls.len() >= MAX_TOOL_CALLS_PER_AGENT {
                    warn!(
                        "run_agent_collecting: tool_calls limit reached ({}), truncating",
                        MAX_TOOL_CALLS_PER_AGENT
                    );
                }
                // ── Execute tool calls concurrently via unified executor ──
                let exec_result = execute_tools_concurrent(
                    &tool_calls,
                    server.tool_registry(),
                    &ToolExecConfig {
                        max_concurrency: 10,
                        circuit_breaker_limit: 5,
                        operation_mode: "edit".to_string(),
                        acp_session_id: None,
                    },
                    None, // no progress_tx in ACP secondary path
                    "",
                    0,
                )
                .await;

                // Build tool result blocks from the executor output
                let mut tool_results: Vec<String> = Vec::new();
                for item in &exec_result.tool_results {
                    let block = if item.success {
                        let result_text = item
                            .output
                            .result
                            .as_ref()
                            .and_then(|r| serde_json::to_string_pretty(r).ok())
                            .unwrap_or_default();
                        build_tool_result_block(&item.tool_name, &result_text, false)
                    } else {
                        let err_text = item
                            .output
                            .error
                            .as_deref()
                            .unwrap_or("tool execution failed");
                        build_tool_result_block(&item.tool_name, err_text, true)
                    };
                    tool_results.push(block);
                }
                if !tool_results.is_empty() {
                    let combined = tool_results.join("\n");
                    let mut followup_messages = base_messages.clone();
                    if !response.trim().is_empty() {
                        followup_messages.push(Message {
                            role: "assistant".to_string(),
                            content: response.clone(),
                        });
                    }
                    followup_messages.push(Message {
                        role: "user".to_string(),
                        content: build_tool_execution_followup_message(&tool_results, true),
                    });

                    let followup =
                        crate::acp::helpers::autonomy::run_followup_after_tool_observation(
                            Arc::clone(&followup_agent),
                            followup_messages,
                            followup_principles.clone(),
                            followup_options.clone(),
                            timeout_duration,
                        )
                        .await;
                    crate::acp::helpers::autonomy_metrics::record_tool_followup_attempt();

                    match followup {
                        Ok((followup_response, followup_reasoning, _followup_model))
                            if !followup_response.trim().is_empty() =>
                        {
                            crate::acp::helpers::autonomy_metrics::record_tool_followup_success();
                            response = followup_response;
                            if !followup_reasoning.is_empty() {
                                reasoning_buffer.push_str(&followup_reasoning);
                            }
                        }
                        _ => {
                            crate::acp::helpers::autonomy_metrics::record_tool_followup_fallback();
                            response.push_str("\n\n");
                            response.push_str(&combined);
                        }
                    }

                    // Emit the tool result block via stream if an observer is attached.
                    if let Some(ref observer) = stream_ctx.stream_observer {
                        let meta = StreamEventMeta {
                            agent_name: stream_ctx.agent_name,
                            phase_name: stream_ctx.phase_name,
                            trace_id: stream_ctx.trace_id,
                            mode: None,
                            risk_score: None,
                            degrade_policy: None,
                        };
                        emit_stream_chunk(
                            server,
                            Some(observer),
                            meta,
                            &combined,
                            chunk_index,
                            total_chars,
                        )
                        .await?;
                    }
                }
                // If the agent produced only reasoning (no content), use the
                // reasoning as the response text.
                if response.trim().is_empty() && !reasoning_buffer.trim().is_empty() {
                    response = std::mem::take(&mut reasoning_buffer);
                }
                Ok::<(String, String, Option<String>), anyhow::Error>((
                    response,
                    reasoning_buffer,
                    selected_model,
                ))
            }
            Ok(Err(err)) => Err(err.into()),
            Err(join_err) => Err(anyhow::anyhow!(tf(
                "error.chat.agent_task_panicked",
                &[("error", &join_err.to_string())]
            ))),
        }
    };

    run_with_optional_timeout(timeout_duration, collect, |duration| {
        anyhow::anyhow!(tf(
            "error.chat.agent_request_timeout",
            &[("seconds", &duration.as_secs().max(1).to_string())]
        ))
    })
    .await
    .inspect_err(|err| {
        if err.to_string().to_ascii_lowercase().contains("timed out") {
            server.observability.metrics.inc_agent_timeout_failure();
        }
    })
}
