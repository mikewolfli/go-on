//! # Unified autonomy loop: plan → act → observe → replan
//!
//! This module provides the orchestration loop for autonomous agent execution,
//! managing sequential attempts with fallback and configurable timeout.
//! Planning and tool-loop concerns are handled by parent modules.
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
use crate::orchestration::autonomy_runtime::{
    parse_tool_call_token, TOKEN_FINISH_REASON_PREFIX, TOKEN_MODEL_USED_PREFIX,
    TOKEN_THINKING_PREFIX, TOKEN_USAGE_PREFIX,
};
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
    pub max_tools_per_round: usize,
    pub enable_planner_guidance: bool,
    pub enable_trace_alignment: bool,
    pub require_replan_for_complex: bool,
    pub enable_execution_intelligence: bool,
    pub tool_timeout_ms: Option<u64>,
    pub max_tool_retries: usize,
    pub use_brain_loop: bool,
    pub enable_governance_gate: bool,
    /// When true, the loop is more persistent: if the first round produces
    /// text without tool calls, it continues with a planning prompt to
    /// encourage tool-based execution. This enables FullAuto to work like
    /// Zed's agent mode — loop until the task is solved, not just one pass.
    pub persistent_loop: bool,
    pub max_messages: usize,
    pub replan_complexity_threshold: u8,
    pub enable_early_stop: bool,
    pub early_stop_confidence_threshold: f64,
    pub capability_signals: bool,
    pub use_dag_execution: bool,
    pub enable_agent_reroute: bool,
    pub recovery_orchestrator: Option<String>,
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
            max_tools_per_round: 8,
            enable_planner_guidance: true,
            enable_trace_alignment: false,
            require_replan_for_complex: true,
            enable_execution_intelligence: true,
            tool_timeout_ms: None,
            max_tool_retries: 2,
            use_brain_loop: false,
            enable_governance_gate: true,
            persistent_loop: false,
            max_messages: 200,
            replan_complexity_threshold: 5,
            enable_early_stop: true,
            early_stop_confidence_threshold: 0.9,
            capability_signals: false,
            operation_mode: "edit".to_string(),
            acp_session_id: None,
            use_dag_execution: true,
            enable_agent_reroute: true,
            recovery_orchestrator: None,
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
    pub rounds: Vec<AutonomyRound>,
    pub planner_guidance_used: bool,
    pub trace_alignment_coverage: f64,
    pub total_duration_ms: u64,
    pub corrective_actions_applied_total: u64,
    pub corrective_action_effectiveness_ratio: f64,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomyRound {
    pub round_index: usize,
    pub phase: AutonomyPhase,
    pub tools_executed: Vec<String>,
    pub planner_guided: bool,
    pub duration_ms: u64,
    pub error: Option<String>,
    pub round_start_offset_ms: u64,
    pub retry_count: usize,
    pub round_stop_reason: String,
    pub agent_switched: bool,
    pub agent_switch_reason: Option<String>,
    pub trace: Vec<String>,
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
    let mut rounds: Vec<AutonomyRound> = Vec::new();
    let mut actual_rounds: usize = 0;
    let mut any_tool_executed_successfully = false;
    let max_iterations = config.max_iterations.max(1);

    for iteration in 0..max_iterations {
        let round_start = Instant::now();
        let mut tool_calls: Vec<(String, String)> = Vec::new();

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
        let mut round_response = String::new();
        let timeout_fut = async move {
            if let Some(dur) = timeout_duration {
                tokio::time::sleep(dur).await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        tokio::pin!(timeout_fut);
        loop {
            tokio::select! {
                biased;
                token = receiver.recv() => {
                    match token {
                        Some(t) => {
                            // Model used detection (detected but not currently consumed)
                            if t.strip_prefix(TOKEN_MODEL_USED_PREFIX).is_some() {
                                continue;
                            }
                            // Finish reason — metadata, not displayed content
                            if t.starts_with(TOKEN_FINISH_REASON_PREFIX) {
                                continue;
                            }
                            // Token usage — metadata, not displayed content
                            if t.starts_with(TOKEN_USAGE_PREFIX) {
                                continue;
                            }
                            // Tool call detection
                            if let Some((tool_name, tool_args)) = parse_tool_call_token(&t) {
                                tool_calls.push((tool_name.to_string(), tool_args.to_string()));
                                // Also append a visible marker to round_response so the
                                // context fed back to the model on the next iteration
                                // includes what tool it decided to call and with what args.
                                let tool_call_text = format!(
                                    "\n[Calling tool: {} with arguments: {}]\n",
                                    tool_name, tool_args
                                );
                                round_response.push_str(&tool_call_text);
                                response.push_str(&tool_call_text);
                                continue;
                            }
                            // Reasoning content — forward to SSE as chunk token
                            // so the GUI shows it inline (same as Zed chat).
                            if let Some(rt) = t.strip_prefix(TOKEN_THINKING_PREFIX) {
                                reasoning.push_str(rt);
                                if let Some(ref tx) = config.progress_tx {
                                    if tx.send(StreamFrame {
                                        event: "chunk",
                                        payload: serde_json::json!({
                                            "token": "",
                                            "reasoning": rt,
                                        }),
                                        status: None,
                                    }).is_err() {
                                        tracing::warn!(
                                            "autonomy_loop: progress_tx send failed: receiver dropped"
                                        );
                                    }
                                }
                                continue;
                            }
                            // Regular token — forward to SSE as chunk token
                            // so the GUI displays it inline.
                            round_response.push_str(&t);
                            response.push_str(&t);
                            if let Some(ref tx) = config.progress_tx {
                                let _ = tx.send(StreamFrame {
                                    event: "chunk",
                                    payload: serde_json::json!({
                                        "token": t,
                                    }),
                                    status: None,
                                });
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

        let _ = chat_task.await;
        let round_duration_ms = round_start.elapsed().as_millis() as u64;

        // Track this round
        let tool_names: Vec<String> = tool_calls.iter().map(|(n, _)| n.clone()).collect();
        rounds.push(AutonomyRound {
            round_index: iteration,
            phase: if tool_calls.is_empty() {
                AutonomyPhase::Completed
            } else {
                AutonomyPhase::Executing
            },
            tools_executed: tool_names.clone(),
            planner_guided: false,
            duration_ms: round_duration_ms,
            error: None,
            round_start_offset_ms: round_start
                .checked_duration_since(start)
                .unwrap_or(std::time::Duration::from_secs(0))
                .as_millis() as u64,
            retry_count: 0,
            round_stop_reason: "completed".to_string(),
            agent_switched: false,
            agent_switch_reason: None,
            trace: Vec::new(),
        });

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

        // ── Execute all tool calls concurrently via unified executor ──
        const MAX_CONSECUTIVE_TOOL_FAILURES: usize = 5;
        let exec_result = execute_tools_concurrent(
            &tool_calls,
            &tool_registry,
            &ToolExecConfig {
                max_concurrency: 10,
                circuit_breaker_limit: MAX_CONSECUTIVE_TOOL_FAILURES,
                operation_mode: config.operation_mode.clone(),
                acp_session_id: config.acp_session_id.clone(),
            },
            config.progress_tx.clone(),
            &params.objective,
            iteration,
        )
        .await;

        // ── Write tool results back to round_response and response ──
        for item in &exec_result.tool_results {
            if item.success {
                let (summary, body) = format_tool_output(&item.tool_name, &item.output);
                round_response.push_str(&body);
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
                round_response.push_str(&item.formatted);
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
            round_response.push_str(&breaker_msg);
            response.push_str(&breaker_msg);
        }

        // Count rounds where tools actually executed
        if !tool_calls.is_empty() {
            actual_rounds += 1;
        }
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
    let total_tools = rounds.iter().map(|r| r.tools_executed.len()).sum();
    let any_tool_executed_successfully_value = any_tool_executed_successfully;
    let last_round_had_tools = rounds.last().is_some_and(|r| !r.tools_executed.is_empty());
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

    let all_tools_failed = total_tools > 0 && !any_tool_executed_successfully;

    Ok(AutonomyLoopResult {
        response: final_response,
        report: AutonomyLoopReport {
            total_rounds: actual_rounds,
            total_tools,
            final_phase: AutonomyPhase::Completed,
            rounds,
            planner_guidance_used: false,
            trace_alignment_coverage: 0.0,
            total_duration_ms,
            corrective_actions_applied_total: 0,
            corrective_action_effectiveness_ratio: 0.0,
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
        "corrective_actions_applied_total": report.corrective_actions_applied_total,
        "corrective_action_effectiveness_ratio": report.corrective_action_effectiveness_ratio,
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
        assert_eq!(cfg.max_tools_per_round, 8);
    }

    #[test]
    fn report_is_success_when_completed() {
        let report = AutonomyLoopReport {
            total_rounds: 3,
            total_tools: 10,
            final_phase: AutonomyPhase::Completed,
            rounds: vec![],
            planner_guidance_used: true,
            trace_alignment_coverage: 0.0,
            total_duration_ms: 5000,
            corrective_actions_applied_total: 0,
            corrective_action_effectiveness_ratio: 0.0,
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
            rounds: vec![],
            planner_guidance_used: false,
            trace_alignment_coverage: 0.0,
            total_duration_ms: 0,
            corrective_actions_applied_total: 0,
            corrective_action_effectiveness_ratio: 0.0,
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
                rounds: vec![],
                planner_guidance_used: false,
                trace_alignment_coverage: 0.0,
                total_duration_ms: 0,
                corrective_actions_applied_total: 0,
                corrective_action_effectiveness_ratio: 0.0,
                stop_reason: "no_response".to_string(),
            },
            all_tools_failed: false,
        };
        assert!(result.response.is_empty());
        assert_eq!(result.report.final_phase, AutonomyPhase::Failed);
    }

    #[test]
    fn round_constructs_with_minimal_fields() {
        let round_record = AutonomyRound {
            round_index: 1,
            phase: AutonomyPhase::Executing,
            tools_executed: vec!["read_file".to_string()],
            planner_guided: false,
            duration_ms: 100,
            error: None,
            round_start_offset_ms: 10,
            retry_count: 0,
            round_stop_reason: "completed".to_string(),
            agent_switched: false,
            agent_switch_reason: None,
            trace: vec![],
        };
        assert_eq!(round_record.round_index, 1);
    }

    #[test]
    fn contract_snapshot_includes_key_metrics() {
        let report = AutonomyLoopReport {
            total_rounds: 2,
            total_tools: 5,
            final_phase: AutonomyPhase::Completed,
            rounds: vec![],
            planner_guidance_used: false,
            trace_alignment_coverage: 0.0,
            total_duration_ms: 3000,
            corrective_actions_applied_total: 1,
            corrective_action_effectiveness_ratio: 0.0,
            stop_reason: "completed".to_string(),
        };
        let snapshot = contract_snapshot(&report);
        assert_eq!(snapshot["total_rounds"], 2);
        assert_eq!(snapshot["total_tools"], 5);
        assert_eq!(snapshot["total_duration_ms"], 3000);
        assert_eq!(snapshot["stop_reason"], "completed");
    }
}
