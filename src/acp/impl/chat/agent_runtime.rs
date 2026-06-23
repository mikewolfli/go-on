//! Agent runtime loop — collecting streamed responses from an agent
//!
//! Contains `run_agent_collecting`, which calls an agent, collects its
//! streamed response while handling tool calls, skill dedup, and follow-up
//! tool observation.  Extracted from the parent `chat.rs` to reduce the
//! monolithic file size.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tracing::warn;

use crate::acp::helpers::context::run_with_optional_timeout;
use crate::acp::server::AcpServer;
use crate::agent::Message;
use crate::i18n::runtime::{t, tf};
use crate::orchestration::autonomy_runtime::{
    build_tool_execution_followup_message, build_tool_result_block,
};

use super::streaming::{
    emit_stream_chunk, emit_stream_done, StreamEventMeta, StreamNotificationContext,
};

/// Calls an agent and collects its streamed response.
/// Returns `(response_text, reasoning_text, selected_model)`.
/// The third element is `Some(model_id)` when the agent
/// explicitly reports which model it used (e.g. Copilot auto-select).
pub(crate) async fn run_agent_collecting(
    server: &AcpServer,
    stream_ctx: StreamNotificationContext<'_>,
    agent: Arc<dyn crate::agent::Agent>,
    messages: Vec<Message>,
    principles: Option<Vec<String>>,
    options: Option<HashMap<String, Value>>,
    timeout_duration: Option<Duration>,
) -> Result<(String, String, Option<String>)> {
    use crate::acp::r#impl::request::tools_pack::execute_mcp_tool_call;
    let base_messages = messages.clone();
    let followup_agent = Arc::clone(&agent);
    let followup_principles = principles.clone();
    let followup_options = options.clone();

    let (sender, mut receiver) = mpsc::channel::<String>(2048);
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
                Ok(None) => break, // channel closed cleanly
                Err(_) => {
                    tracing::warn!(
                        "agent streaming recv() timed out after {}s — aborting collect",
                        recv_timeout.as_secs()
                    );
                    break;
                }
            };
            // Check for model-used token (prefixed with __model_used__)
            // This is sent by CopilotAgent after a successful auto-select.
            if let Some(model_id) = token.strip_prefix("__model_used__:") {
                selected_model = Some(model_id.trim().to_string());
                continue;
            }

            // Check for tool call tokens (prefixed with __tool_call__)
            if let Some(tool_call_data) = token.strip_prefix("__tool_call__:") {
                // Format: __tool_call__:<tool_name>:<json_arguments>
                if let Some(colon_pos) = tool_call_data.find(':') {
                    let tool_name = &tool_call_data[..colon_pos];
                    let tool_args = &tool_call_data[colon_pos + 1..];
                    tool_calls.push((tool_name.to_string(), tool_args.to_string()));
                }
                continue;
            }

            let next_chars = token.chars().count();
            if crate::acp::helpers::conversation::stream_would_exceed_limits(
                chunk_index,
                total_chars,
                next_chars,
            ) {
                anyhow::bail!(t("error.chat.stream_output_limits"));
            }

            // Check for reasoning tokens (prefixed with __thinking__)
            if let Some(reasoning_token) = token.strip_prefix("__thinking__") {
                reasoning_buffer.push_str(reasoning_token);
            } else {
                response.push_str(&token);
            }

            chunk_index += 1;
            total_chars += next_chars;

            let display_token = if token.starts_with("__thinking__") {
                ""
            } else {
                &token
            };
            emit_stream_chunk(
                server,
                stream_ctx.stream_observer.as_ref(),
                StreamEventMeta {
                    agent_name: stream_ctx.agent_name,
                    phase_name: stream_ctx.phase_name,
                    trace_id: stream_ctx.trace_id,
                },
                display_token,
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
                    },
                    chunk_index,
                    total_chars,
                    stream_started.elapsed().as_millis() as u64,
                    selected_model.clone(),
                )
                .await?;
                // ── Execute tool calls ────────────────────────────────
                // If the LLM responded with tool calls, execute each
                // registered skill and append the results to the response.
                const MAX_TOOL_CALLS_PER_AGENT: usize = 100;
                // ── Skill dedup: prevent AI from calling multiple skills at once ──
                // When the LLM tries to invoke several skills for the same request,
                // pick the single best one automatically. This stops indecisive AI
                // behavior where multiple nearly-identical skills are invoked together.
                let tool_calls = {
                    // Identify which tool calls are skills vs. built-in tools.
                    // Built-in tools (skill-finder, goon_*, etc.) are excluded
                    // from the multi-call dedup check.
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
                        // Multiple skills called at once — pick the best one by score.
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
                let mut tool_results: Vec<String> = Vec::new();
                for (tool_name, tool_args_str) in tool_calls.iter().take(MAX_TOOL_CALLS_PER_AGENT) {
                    let parsed_args: Value =
                        serde_json::from_str(tool_args_str).unwrap_or(json!({}));
                    match execute_mcp_tool_call(server, tool_name, &parsed_args).await {
                        Ok(result) => {
                            let result_text =
                                serde_json::to_string_pretty(&result).unwrap_or_default();
                            let tool_block =
                                build_tool_result_block(tool_name, &result_text, false);
                            tool_results.push(tool_block);
                        }
                        Err(err) => {
                            let err_block =
                                build_tool_result_block(tool_name, &err.to_string(), true);
                            tool_results.push(err_block);
                        }
                    }
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
                        Ok((followup_response, followup_reasoning, followup_model))
                            if !followup_response.trim().is_empty() =>
                        {
                            crate::acp::helpers::autonomy_metrics::record_tool_followup_success();
                            response = followup_response;
                            if !followup_reasoning.is_empty() {
                                reasoning_buffer.push_str(&followup_reasoning);
                            }
                            if selected_model.is_none() {
                                selected_model = followup_model;
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
