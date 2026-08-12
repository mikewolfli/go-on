//! # Unified autonomy loop: plan → act → observe → replan
//!
//! This module provides the **execution layer** of go-on's unified execution
//! loop: call the LLM → parse tool call tokens → run tools concurrently →
//! collect results → loop.  It is a self-contained agent-driven tool executor.
//!
//! For structured plan management (DAG, deep reasoning, reflection, replanning),
//! see the `brain_loop` planner.  The two
//! are packaged together via `autonomy_loop_adapter::run_acp_autonomy_loop()`
//! which provides a single entry point for the real execution loop.
//!
//! ## Key types
//! - [`AutonomyLoopConfig`] — loop configuration
//! - [`AutonomyLoopResult`] — final result
//! - [`AutonomyLoopReport`] — detailed report with per-round metrics

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::acp::r#impl::chat::streaming::StreamFrame;
use crate::agent::{Agent, Message, StreamingSender};
use crate::orchestration::autonomy_runtime::{classify_agent_token, AgentToken};
use crate::orchestration::tool::executor::{execute_tools_concurrent, ToolExecConfig};
use crate::orchestration::tool::{ToolOutput, ToolRegistry};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Parameters for `run_autonomy_loop`, bundled to avoid clippy `too_many_arguments`.
#[allow(
    missing_docs,
    reason = "intentional field bundle for run_autonomy_loop"
)]
pub struct AutonomyLoopParams {
    pub agent: Arc<dyn Agent>,
    pub tool_registry: Option<Arc<ToolRegistry>>,
    pub objective: String,
    pub messages: Vec<Message>,
    pub principles: Option<Vec<String>>,
    pub options: Option<HashMap<String, Value>>,
}

/// Configuration for the autonomy loop execution.
/// Used by autonomy_loop_adapter to create the loop config.
#[derive(Clone, Serialize, Deserialize)]
pub struct AutonomyLoopConfig {
    pub max_iterations: usize,
    /// When true, the loop is more persistent: if the first round produces
    /// text without tool calls, it continues with a planning prompt to
    /// encourage tool-based execution. This enables FullAuto to work like
    /// Zed's agent mode — loop until the task is solved, not just one pass.
    pub persistent_loop: bool,
    /// Sender for SSE progress events during tool execution.
    /// If set, progress frames are sent before/after each tool call to
    /// keep the SSE inactivity timeout from firing during long tool runs.
    #[serde(skip)]
    pub(crate) progress_tx: Option<mpsc::UnboundedSender<StreamFrame>>,
    /// The operation mode (edit, safeguard, full_auto, etc.) used for
    /// tool approval events and mode-specific behavior.
    pub operation_mode: String,
    /// ACP session ID used for permission request round-trips with ACP clients.
    pub acp_session_id: Option<String>,
}

impl Default for AutonomyLoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 5,
            persistent_loop: false,
            operation_mode: "edit".to_string(),
            acp_session_id: None,
            progress_tx: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Report / Result types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomyLoopReport {
    pub total_rounds: usize,
    pub total_tools: usize,
    pub final_phase: AutonomyPhase,
    pub total_duration_ms: u64,
    pub stop_reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AutonomyPhase {
    Planning,
    Executing,
    Observing,
    Finalizing,
    Completed,
    Failed,
}

/// Result of the autonomy loop execution
#[derive(Debug, Clone)]
pub struct AutonomyLoopResult {
    pub response: String,
    pub report: AutonomyLoopReport,
    /// True when tools were requested but ALL of them failed validation or execution.
    /// The caller can use this flag to return an error status to the client instead
    /// of pretending the task succeeded.
    pub all_tools_failed: bool,
}

/// Execute a full autonomy loop: plan → (execute + observe × N) → finalize.
pub async fn run_autonomy_loop(
    params: AutonomyLoopParams,
    config: AutonomyLoopConfig,
    timeout_duration: Option<std::time::Duration>,
) -> Result<AutonomyLoopResult, anyhow::Error> {
    let start = Instant::now();
    let tool_registry = params
        .tool_registry
        .unwrap_or_else(|| Arc::new(ToolRegistry::new()));
    // Note: ToolRegistry::new() in the fallback is called only when
    // no registry is provided via params, which is the exceptional case.
    // The normal path passes a shared registry from the caller.

    tracing::debug!(
        target: "autonomy_loop",
        objective = %params.objective,
        messages = params.messages.len(),
        max_iterations = config.max_iterations,
        "autonomy loop starting"
    );

    let mut response = String::new();
    let mut reasoning = String::new();
    let mut actual_rounds: usize = 0;
    let mut total_tools_executed: usize = 0;
    let mut last_round_had_tools: bool = false;
    let mut any_tool_executed_successfully = false;
    let max_iterations = config.max_iterations.max(1);

    for iteration in 0..max_iterations {
        let mut tool_calls: Vec<(String, String)> = Vec::new();

        // $/cancel_request support: abort between rounds when the client
        // cancelled this request id (task-local set by handle_request).
        if crate::acp::r#impl::request::protocol_pack::current_request_cancelled() {
            return Err(crate::acp::r#impl::request::protocol_pack::log_and_cancel(
                "autonomy_loop",
            ));
        }

        // ── Emit round iteration progress status ─────────────────────
        if let Some(ref tx) = config.progress_tx {
            let _ = tx.send(StreamFrame {
                event: "status",
                payload: serde_json::json!({
                    "message": format!("Round {}/{}: planning next steps...",
                        iteration + 1, max_iterations),
                    "round_current": iteration + 1,
                    "round_total": max_iterations,
                }),
                status: Some("analyzing"),
            });
        }

        // ── Call agent with streaming ────────────────────────────────
        let (sender_inner, mut receiver) = mpsc::unbounded_channel::<String>();
        let sender = StreamingSender::from(sender_inner);

        let agent_messages = if iteration == 0 {
            params.messages.clone()
        } else {
            // For follow-up rounds, the response text serves as context
            let principles_context = params
                .principles
                .as_ref()
                .map(|p| format!("\n\nPUA principles:\n- {}", p.join("\n- ")))
                .unwrap_or_default();
            let is_last_round = iteration + 1 >= max_iterations;
            // Persistent mode (FullAuto): use a more explicit planning prompt
            // on the second round to encourage tool-based execution.
            let instruction = if config.persistent_loop && iteration == 1 && !is_last_round {
                "Plan the steps needed and use available tools (read_file, search_files, shell_exec, etc.) to accomplish the task. Execute each step one by one. Do NOT just describe what to do — actually use the tools to do it."
            } else if is_last_round {
                "Summarize what was accomplished and provide the final result."
            } else {
                "Continue with the task."
            };
            vec![Message {
                role: "user".to_string(),
                content: format!(
                    "{}. Context so far: {}{}\n\nOriginal objective: {}",
                    instruction, response, principles_context, params.objective
                ),
            }]
        };

        // ── Send thinking indicator before agent starts ─────────────
        // This eliminates the blank-wait period: the client sees
        // "Thinking..." immediately, before the first token arrives.
        if let Some(ref tx) = config.progress_tx {
            let _ = tx.send(StreamFrame {
                event: "chunk",
                payload: serde_json::json!({
                    "token": "",
                    "thinking": true,
                }),
                status: Some("thinking"),
            });
        }

        let agent_clone = Arc::clone(&params.agent);
        let principles_clone = params.principles.clone();
        let options_clone = params.options.clone();
        let chat_task = tokio::spawn(async move {
            agent_clone
                .chat(agent_messages, principles_clone, options_clone, sender)
                .await
                .map_err(|e| anyhow::anyhow!("agent chat failed: {}", e))
        });

        // ── Stream tokens ────────────────────────────────────────────
        let timeout_fut = async move {
            if let Some(dur) = timeout_duration {
                tokio::time::sleep(dur).await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        tokio::pin!(timeout_fut);
        loop {
            // $/cancel_request support: stop collecting tokens as soon as the
            // client cancels this request id — do not waste further LLM calls.
            if crate::acp::r#impl::request::protocol_pack::current_request_cancelled() {
                tracing::info!(
                    target: "autonomy_loop",
                    round = iteration,
                    "autonomy_loop: request cancelled by client, stopping round"
                );
                break;
            }
            tokio::select! {
                biased;
                token = receiver.recv() => {
                    match token {
                        Some(t) => {
                            // Single shared token classifier — the same vocabulary
                            // as the CLI and agent-runtime collection loops, so the
                            // stream protocol has exactly one parser.
                            match classify_agent_token(&t) {
                                // Model-used announcement / finish-reason / usage
                                // telemetry / reasoning start-end markers —
                                // metadata, not displayed content. (No agent
                                // currently emits reasoning markers, but the
                                // classifier keeps them out of response and SSE
                                // if one ever does.)
                                AgentToken::ModelUsed(_)
                                | AgentToken::Telemetry
                                | AgentToken::ReasoningMarker => continue,
                                // Tool call — record for execution and append a
                                // visible marker to response so the context fed
                                // back to the model on the next iteration includes
                                // what tool it decided to call and with what args.
                                AgentToken::ToolCall(tool_name, tool_args) => {
                                    let tool_call_text = format!(
                                        "\n[Calling tool: {} with arguments: {}]\n",
                                        tool_name, tool_args
                                    );
                                    tool_calls.push((tool_name, tool_args));
                                    response.push_str(&tool_call_text);
                                }
                                // Reasoning content — forward to SSE as a reasoning
                                // frame so the GUI shows it inline (same as Zed chat).
                                AgentToken::Reasoning(reasoning_token) => {
                                    reasoning.push_str(&reasoning_token);
                                    if let Some(ref tx) = config.progress_tx {
                                        if tx.send(StreamFrame {
                                            event: "chunk",
                                            payload: serde_json::json!({
                                                "token": "",
                                                "reasoning": reasoning_token,
                                            }),
                                            status: None,
                                        }).is_err() {
                                            tracing::warn!(
                                                "autonomy_loop: progress_tx send failed: receiver dropped"
                                            );
                                        }
                                    }
                                }
                                // Regular token — forward to SSE as chunk token
                                // so the GUI displays it inline.
                                AgentToken::Content(token) => {
                                    response.push_str(&token);
                                    if let Some(ref tx) = config.progress_tx {
                                        let _ = tx.send(StreamFrame {
                                            event: "chunk",
                                            payload: serde_json::json!({
                                                "token": token,
                                            }),
                                            status: None,
                                        });
                                    }
                                }
                            }
                        }
                        None => break,
                    }
                }
                _ = &mut timeout_fut => {
                    tracing::warn!("autonomy_loop: round {iteration} timed out");
                    break;
                }
            }
        }

        // $/cancel_request support: abort the whole loop (not just the current
        // round) on cancellation. Abort the in-flight agent chat task first —
        // previously the JoinHandle was dropped here without abort, so the
        // orphaned LLM call kept streaming into a dropped channel.
        if crate::acp::r#impl::request::protocol_pack::current_request_cancelled() {
            chat_task.abort();
            return Err(crate::acp::r#impl::request::protocol_pack::log_and_cancel(
                "autonomy_loop",
            ));
        }
        // Await the chat task, surfacing panics/errors instead of silently
        // continuing with a possibly-partial tool_calls list.
        if let Err(e) = chat_task.await {
            tracing::warn!("autonomy_loop: agent chat task failed: {e}");
        }

        // ── Execute tool calls ───────────────────────────────────────
        if tool_calls.is_empty() {
            // No tools to execute in this round.
            //
            // Normal mode: break immediately — the AI is done.
            // Persistent mode (FullAuto): if this is the first round, do
            // one more round with an explicit planning prompt to encourage
            // tool-based execution. After the second round, break normally.
            if config.persistent_loop && iteration == 0 {
                // Fall through to next iteration with planning prompt below
            } else {
                actual_rounds += 1; // Terminal response round
                break; // No tools to execute — final response ready
            }
        }

        if iteration + 1 >= max_iterations {
            if !tool_calls.is_empty() {
                actual_rounds += 1; // Tools were called this round
            }
            break; // Max iterations reached
        }

        // ── Enforce the mode's tool policy (allowed tools + max calls) ──
        // Shared with the CLI chat path (`filter_tool_calls_by_policy` in
        // orchestration/mode.rs). Previously the ACP path bypassed the mode
        // policy entirely: Ask mode executed tools, Plan mode could run write
        // tools, and the per-agent cap was not enforced.
        let mode_kind = crate::orchestration::mode::ModeKind::from(config.operation_mode.as_str());
        let (tool_calls, blocked) =
            crate::orchestration::mode::filter_tool_calls_by_policy(&tool_calls, &mode_kind);
        if !blocked.is_empty() {
            tracing::warn!(
                "autonomy_loop: mode {:?} blocked {} tool call(s): {:?}",
                mode_kind,
                blocked.len(),
                blocked
            );
        }
        if tool_calls.is_empty() {
            // The mode policy blocked every tool call — nothing to execute.
            actual_rounds += 1;
            break;
        }

        // ── Execute all tool calls concurrently via unified executor ──
        const MAX_CONSECUTIVE_TOOL_FAILURES: usize = 5;
        let exec_result = execute_tools_concurrent(
            &tool_calls,
            &tool_registry,
            &ToolExecConfig {
                max_concurrency: 10,
                circuit_breaker_limit: MAX_CONSECUTIVE_TOOL_FAILURES,
                operation_mode: config.operation_mode.clone(),
                governance_required: config.operation_mode == "edit"
                    || config.operation_mode == "safeguard",
                is_safeguard: config.operation_mode == "safeguard",
                acp_session_id: config.acp_session_id.clone(),
            },
            config.progress_tx.clone(),
            &params.objective,
            iteration,
        )
        .await;

        // ── Write tool results back to response ──
        for item in &exec_result.tool_results {
            if item.success {
                let (summary, body) = format_tool_output(&item.tool_name, &item.output);
                response.push_str(&body);
                any_tool_executed_successfully = true;

                // ── Send completion notification as visible chunk token ──
                if let Some(ref tx) = config.progress_tx {
                    let _ = tx.send(StreamFrame {
                        event: "chunk",
                        payload: serde_json::json!({
                            "token": summary,
                            "tool_status": "completed",
                        }),
                        status: Some("generating"),
                    });
                }
            } else {
                response.push_str(&item.formatted);

                // ── Send failure notification ──
                if let Some(ref tx) = config.progress_tx {
                    let _ = tx.send(StreamFrame {
                        event: "chunk",
                        payload: serde_json::json!({
                            "token": format!("❌ **{}** failed ", item.tool_name),
                            "tool_status": "failed",
                        }),
                        status: Some("generating"),
                    });
                }
            }
        }

        if exec_result.circuit_breaker_triggered {
            let breaker_msg = format!(
                "\n[Circuit breaker: stopped after {} consecutive tool call failures]",
                exec_result.failure_count
            );
            response.push_str(&breaker_msg);
        }

        // Count rounds where tools actually executed
        if !tool_calls.is_empty() {
            actual_rounds += 1;
        }
        // `total_tools` mirrors the removed per-round `tools_executed` vector:
        // it counts the tool calls requested in each round.
        total_tools_executed += tool_calls.len();
        last_round_had_tools = !tool_calls.is_empty();
    }

    let total_duration_ms = start.elapsed().as_millis() as u64;

    // ── Emit final summary phase status ─────────────────────────────
    if let Some(ref tx) = config.progress_tx {
        let _ = tx.send(StreamFrame {
            event: "status",
            payload: serde_json::json!({
                "message": "Generating final summary...",
            }),
            status: Some("generating"),
        });
    }

    // Post-loop summary: if tools were executed in the last round, the
    // agent may not have produced a final text response. Do one more call
    // asking for a summary so the user always sees a final answer.
    let total_tools = total_tools_executed;
    let any_tool_executed_successfully_value = any_tool_executed_successfully;
    let mut final_response = response;
    // Skip summary if all tools failed — the failure context is more
    // useful to the user than an LLM-summarized version of the same errors.
    let all_tools_failed = total_tools > 0 && !any_tool_executed_successfully_value;
    if last_round_had_tools && !all_tools_failed {
        // In FullAuto mode, if the last round already produced response
        // text alongside tool calls, skip the extra summary LLM call to
        // avoid n+1 calls. Only keep this behavior for Edit mode, where
        // an explicit summary is always desirable after tool execution.
        let should_skip = config.operation_mode == "full_auto" && !final_response.trim().is_empty();
        if !should_skip {
            let summary_msg = Message {
                role: "user".to_string(),
                content: format!(
                    "Summarize what was accomplished and provide the final result.\n\nContext: {}\n\nOriginal objective: {}",
                    final_response, params.objective
                ),
            };
            let (tx, mut rx) = mpsc::unbounded_channel::<String>();
            let summary_sender = StreamingSender::from(tx);
            if params
                .agent
                .chat(
                    vec![summary_msg],
                    params.principles.clone(),
                    params.options.clone(),
                    summary_sender,
                )
                .await
                .is_ok()
            {
                let mut summary = String::new();
                while let Some(token) = rx.recv().await {
                    summary.push_str(&token);
                }
                if !summary.trim().is_empty() {
                    final_response = summary;
                }
            }
        }
    }

    // If the final response is empty but we have reasoning, use reasoning
    if final_response.trim().is_empty() && !reasoning.trim().is_empty() {
        final_response = reasoning;
    }

    Ok(AutonomyLoopResult {
        response: final_response,
        report: AutonomyLoopReport {
            total_rounds: actual_rounds,
            total_tools,
            final_phase: AutonomyPhase::Completed,
            total_duration_ms,
            stop_reason: if all_tools_failed {
                "all_tools_failed".to_string()
            } else if total_tools > 0 {
                "tools_executed".to_string()
            } else {
                "completed".to_string()
            },
        },
        all_tools_failed,
    })
}

/// Format a tool's output into a clean, human-readable markdown block.
///
/// Replaces raw `{:?}` debug formatting with structured Markdown that
/// can be rendered inline by Zed / GUI / CLI chat clients.
fn format_tool_output(tool_name: &str, output: &ToolOutput) -> (String, String) {
    let success = output.error.is_none();
    let status_icon = if success { "✅" } else { "❌" };

    // Summary line (streamed as chunk token)
    let summary = if success {
        format!("{} **{}** ", status_icon, tool_name)
    } else {
        format!("{} **{}** failed ", status_icon, tool_name)
    };

    // Body (appended to response text)
    // Use the structured result field when available, fall back to debug fmt
    let body_content = if success {
        output
            .result
            .as_ref()
            .and_then(|r| {
                if let Some(s) = r.as_str() {
                    if !s.is_empty() {
                        return Some(s.to_string());
                    }
                }
                None
            })
            .unwrap_or_else(|| format!("{:?}", output))
    } else {
        output
            .error
            .as_ref()
            .cloned()
            .unwrap_or_else(|| format!("{:?}", output))
    };

    let body = format!(
        "\n<details>\
         \n<summary>{} {}</summary>\
         \n```\n{}\n```\
         \n</details>\n",
        status_icon, tool_name, body_content
    );

    (summary, body)
}

/// Build a contract snapshot from the loop report.
pub fn contract_snapshot(report: &AutonomyLoopReport) -> Value {
    json!({
        "total_rounds": report.total_rounds,
        "total_tools": report.total_tools,
        "final_phase": format!("{:?}", report.final_phase),
        "total_duration_ms": report.total_duration_ms,
        "stop_reason": report.stop_reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autonomy_phases_are_distinct() {
        use std::collections::HashSet;
        let phases = [
            AutonomyPhase::Planning,
            AutonomyPhase::Executing,
            AutonomyPhase::Observing,
            AutonomyPhase::Finalizing,
            AutonomyPhase::Completed,
            AutonomyPhase::Failed,
        ];
        let unique: HashSet<_> = phases.iter().collect();
        assert_eq!(unique.len(), 6);
    }

    #[test]
    fn autonomy_phases_roundtrip_serde() {
        for phase in &[
            AutonomyPhase::Planning,
            AutonomyPhase::Executing,
            AutonomyPhase::Completed,
            AutonomyPhase::Failed,
        ] {
            let json_val = serde_json::to_value(phase).unwrap();
            let back: AutonomyPhase = serde_json::from_value(json_val).unwrap();
            assert_eq!(*phase, back);
        }
    }

    #[test]
    fn default_config_is_reasonable() {
        let cfg = AutonomyLoopConfig::default();
        assert_eq!(cfg.max_iterations, 5);
        assert!(!cfg.persistent_loop, "persistent loop must be opt-in");
        assert_eq!(cfg.operation_mode, "edit");
    }

    #[test]
    fn report_is_success_when_completed() {
        let report = AutonomyLoopReport {
            total_rounds: 3,
            total_tools: 10,
            final_phase: AutonomyPhase::Completed,
            total_duration_ms: 5000,
            stop_reason: "completed".to_string(),
        };
        assert_eq!(report.final_phase, AutonomyPhase::Completed);
        assert!(!report.stop_reason.is_empty());
    }

    #[test]
    fn report_contains_all_required_fields() {
        let report = AutonomyLoopReport {
            total_rounds: 0,
            total_tools: 0,
            final_phase: AutonomyPhase::Planning,
            total_duration_ms: 0,
            stop_reason: "initial".to_string(),
        };
        let json_val = serde_json::to_value(&report).unwrap();
        assert!(json_val.get("total_rounds").is_some());
        assert!(json_val.get("final_phase").is_some());
        assert!(json_val.get("stop_reason").is_some());
    }

    #[test]
    fn empty_response_has_empty_result() {
        let result = AutonomyLoopResult {
            response: String::new(),
            report: AutonomyLoopReport {
                total_rounds: 0,
                total_tools: 0,
                final_phase: AutonomyPhase::Failed,
                total_duration_ms: 0,
                stop_reason: "no_response".to_string(),
            },
            all_tools_failed: false,
        };
        assert!(result.response.is_empty());
        assert_eq!(result.report.final_phase, AutonomyPhase::Failed);
    }

    #[test]
    fn contract_snapshot_includes_key_metrics() {
        let report = AutonomyLoopReport {
            total_rounds: 2,
            total_tools: 5,
            final_phase: AutonomyPhase::Completed,
            total_duration_ms: 3000,
            stop_reason: "completed".to_string(),
        };
        let snapshot = contract_snapshot(&report);
        assert_eq!(snapshot["total_rounds"], 2);
        assert_eq!(snapshot["total_tools"], 5);
        assert_eq!(snapshot["total_duration_ms"], 3000);
        assert_eq!(snapshot["stop_reason"], "completed");
    }
}
