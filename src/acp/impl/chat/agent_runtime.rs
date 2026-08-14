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
    build_tool_execution_followup_message, build_tool_result_block, classify_agent_token,
    AgentToken,
};
use crate::orchestration::tool::executor::{execute_tools_concurrent, ToolExecConfig};

use super::streaming::{
    emit_stream_chunk, emit_stream_done, StreamEventMeta, StreamNotificationContext,
};

/// Aborts a spawned agent task when dropped. Used by
/// [`run_agent_collecting`] so a mid-flight cancellation (e.g. the fallback
/// path dropping losing candidates after the first success) stops the
/// orphaned `agent.chat` task instead of letting it stream to completion.
struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

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
    let mut chunks = 0usize;
    let mut total_chars = 0usize;

    // Shared stream cap (256k chars / 4096 chunks): the collected response
    // flows into the LLM follow-up context, so an unbounded stream would
    // bloat it. Truncation is explicit, never silent.
    macro_rules! append_capped {
        ($buf:expr, $token:expr) => {{
            let next_chars = $token.chars().count();
            if crate::acp::helpers::conversation::stream_would_exceed_limits(
                chunks,
                total_chars,
                next_chars,
            ) {
                tracing::warn!(
                    "collect_agent_responses: output truncated at {total_chars} chars (chunks {chunks})"
                );
                return Ok(CollectedResponse {
                    response,
                    reasoning: reasoning_buffer,
                    tool_calls,
                });
            }
            $buf.push_str(&$token);
            chunks += 1;
            total_chars += next_chars;
        }};
    }

    while let Some(token) = receiver.recv().await {
        match classify_agent_token(&token) {
            AgentToken::ModelUsed(_) => continue,
            AgentToken::ToolCall(tool_name, tool_args) => {
                tool_calls.push((tool_name, tool_args));
                continue;
            }
            AgentToken::ReasoningMarker | AgentToken::Telemetry => continue,
            AgentToken::Reasoning(reasoning_token) => {
                append_capped!(reasoning_buffer, reasoning_token);
                continue;
            }
            AgentToken::Content(text) => append_capped!(response, text),
        }
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
/// `operation_mode` / `is_safeguard` control governance approval events.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_agent_collecting(
    server: &AcpServer,
    stream_ctx: StreamNotificationContext<'_>,
    agent: Arc<dyn crate::agent::Agent>,
    messages: &[Message],
    principles: Option<Vec<String>>,
    options: Option<HashMap<String, Value>>,
    timeout_duration: Option<Duration>,
    operation_mode: &str,
    is_safeguard: bool,
) -> Result<(String, String, Option<String>)> {
    // tool execution delegated to execute_tools_concurrent
    let chat_messages = messages.to_vec();
    let followup_agent = Arc::clone(&agent);
    let followup_principles = principles.clone();
    let followup_options = options.clone();

    let (sender, mut receiver) = mpsc::unbounded_channel::<String>();
    let sender = crate::agent::StreamingSender::from(sender);
    let task =
        tokio::spawn(async move { agent.chat(chat_messages, principles, options, sender).await });
    // Abort capability independent of the JoinHandle: when the outer timeout
    // fires, `collect` (which owns the handle) is dropped without aborting —
    // this aborts the orphaned agent.chat so it stops burning tokens.
    let abort_handle = task.abort_handle();
    // Clone for use inside `collect` (the join-timeout branch) while the
    // original stays available to the outer `.inspect_err`.
    let collect_abort = abort_handle.clone();
    // RAII guard: if THIS future is dropped mid-flight (not via timeout — e.g.
    // the fallback path cancels losing candidates once the first success wins),
    // abort the spawned agent.chat task. A dropped JoinHandle alone would
    // detach the task and let it keep streaming/burning tokens to completion.
    let _abort_on_drop = AbortOnDrop(abort_handle.clone());

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
                                                                   // Per-chunk receive timeout (120s): a stuck provider must not hang the
                                                                   // pipeline; the overall 600s cap bounds the whole collection.
        let recv_timeout = std::time::Duration::from_secs(120);
        loop {
            // $/cancel_request support: abort token collection as soon as the
            // client cancelled this request id (checked against the request-id
            // task-local set by handle_request). Returning Err surfaces the
            // canonical cancellation error to the caller instead of emitting a
            // partial "success" result.
            if crate::acp::r#impl::request::protocol_pack::current_request_cancelled() {
                return Err(crate::acp::r#impl::request::protocol_pack::log_and_cancel(
                    "run_agent_collecting",
                ));
            }
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
            // ── Token classification (single shared classifier) ──
            let classified = classify_agent_token(&token);
            match &classified {
                AgentToken::ModelUsed(model_id) => {
                    selected_model = Some(model_id.clone());
                    continue;
                }
                AgentToken::ToolCall(tool_name, tool_args) => {
                    tool_calls.push((tool_name.clone(), tool_args.clone()));
                    continue;
                }
                _ => {}
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
            match classified {
                // Reasoning tokens accumulate and fall through to emission
                // (emit_stream_chunk splits display vs reasoning itself).
                AgentToken::Reasoning(reasoning_token) => {
                    reasoning_buffer.push_str(&reasoning_token);
                }
                AgentToken::ReasoningMarker | AgentToken::Telemetry => continue,
                AgentToken::Content(text) => response.push_str(&text),
                AgentToken::ModelUsed(_) | AgentToken::ToolCall(..) => {
                    unreachable!("model-used / tool-call tokens were handled above")
                }
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

        // The streaming caps (per-chunk / overall) only bound the recv loop
        // above. Bound the final join as well: a stalled provider (e.g. a
        // half-open SSE connection on a timeout-less reqwest client) must not
        // hang the whole request past the declared caps.
        let task_outcome = match tokio::time::timeout(recv_timeout, task).await {
            Ok(outcome) => outcome,
            Err(_) => {
                tracing::warn!(
                    "agent.chat join timed out after {}s — aborting task and returning partial response",
                    recv_timeout.as_secs()
                );
                collect_abort.abort();
                // Emit done with the partial response so the stream terminates
                // (callers assume a done was sent on the fallback path).
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
                return Ok((response, reasoning_buffer, selected_model));
            }
        };

        match task_outcome {
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
                // ── Skill dedup (shared logic with CLI chat) ──
                let (tool_calls, _) = crate::orchestration::tool::dedup_skill_calls(
                    &tool_calls,
                    &server.orchestration_deps.skill_registry,
                );

                // Enforce the per-agent tool-call ceiling: previously this only
                // logged "truncating" without truncating, so a runaway model
                // could emit an unbounded tool batch. Truncate for real and
                // report how many calls were dropped.
                let tool_calls: Vec<_> = if tool_calls.len() > MAX_TOOL_CALLS_PER_AGENT {
                    let dropped = tool_calls.len() - MAX_TOOL_CALLS_PER_AGENT;
                    warn!(
                        "run_agent_collecting: tool_calls limit reached ({}), dropping {}",
                        MAX_TOOL_CALLS_PER_AGENT, dropped
                    );
                    tool_calls
                        .into_iter()
                        .take(MAX_TOOL_CALLS_PER_AGENT)
                        .collect()
                } else {
                    tool_calls
                };

                // ── Enforce the mode's tool policy (allowed tools + max
                // calls) — shared with the CLI chat path. Previously the ACP
                // path bypassed the mode policy entirely: Ask mode executed
                // tools, Plan mode could run write tools, and the per-agent
                // cap above was log-only. An empty result after filtering
                // short-circuits the executor (it returns a default result
                // for empty input) and skips the followup block below.
                let mode_kind = crate::orchestration::mode::ModeKind::from(operation_mode);
                let (tool_calls, blocked) = crate::orchestration::mode::filter_tool_calls_by_policy(
                    &tool_calls,
                    &mode_kind,
                );
                if !blocked.is_empty() {
                    warn!(
                        "run_agent_collecting: mode {:?} blocked {} tool call(s): {:?}",
                        mode_kind,
                        blocked.len(),
                        blocked
                    );
                }

                // ── Execute tool calls concurrently via unified executor ──
                // operation_mode / is_safeguard are passed through from the
                // request (fallback path) or defaulted for read-only helper
                // paths (vote / phase-summary), so governance approval events
                // carry the real mode instead of a hard-coded "edit".
                let exec_result = execute_tools_concurrent(
                    &tool_calls,
                    server.tool_registry(),
                    &ToolExecConfig {
                        max_concurrency: 10,
                        circuit_breaker_limit: 5,
                        operation_mode: operation_mode.to_string(),
                        governance_required: operation_mode == "edit"
                            || operation_mode == "safeguard",
                        is_safeguard,
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
                        let mut result_text = item
                            .output
                            .result
                            .as_ref()
                            .and_then(|r| serde_json::to_string_pretty(r).ok())
                            .unwrap_or_default();
                        // Bound the block that flows into the LLM follow-up
                        // context (same cap as the executor's consolidated
                        // response path).
                        crate::orchestration::tool::exec_common::truncate_output(&mut result_text);
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
                    let mut followup_messages = messages.to_vec();
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
    .inspect_err(|_| {
        // The collect future (and its JoinHandle) was dropped by the timeout;
        // abort the still-running agent.chat task.
        abort_handle.abort();
    })
}
