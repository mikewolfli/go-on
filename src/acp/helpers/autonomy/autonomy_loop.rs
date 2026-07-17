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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomyLoopConfig {
    pub max_iterations: usize,
    pub max_tools_per_round: usize,
    pub enable_planner_guidance: bool,
    pub enable_trace_alignment: bool,
    pub require_replan_for_complex: bool,
    pub enable_execution_intelligence: bool,
    pub tool_timeout_ms: Option<u64>,
    pub max_tool_concurrency: usize,
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
            max_tool_concurrency: 4,
            max_tool_retries: 2,
            use_brain_loop: false,
            enable_governance_gate: true,
            persistent_loop: false,
            max_messages: 200,
            replan_complexity_threshold: 5,
            enable_early_stop: true,
            early_stop_confidence_threshold: 0.9,
            capability_signals: false,
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
    pub audit_trail: Option<Vec<AuditEntry>>,
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
                break; // No tools to execute — final response ready
            }
        }

        if iteration + 1 >= max_iterations {
            break; // Max iterations reached
        }

        let mut consecutive_failures = 0;
        const MAX_CONSECUTIVE_TOOL_FAILURES: usize = 5;

        for (tool_name, tool_args) in &tool_calls {
            // ── Stream tool execution progress as visible chat tokens ──
            // Send SSE progress event before executing tool ────────
            if let Some(ref tx) = config.progress_tx {
                let _ = tx.send(StreamFrame {
                    event: "progress",
                    payload: serde_json::json!({
                        "message": format!("executing tool {}...", tool_name),
                    }),
                    status: Some("analyzing"),
                });
            }

            // Validate tool arguments BEFORE execution
            let parsed_args: serde_json::Value =
                serde_json::from_str(tool_args).unwrap_or_default();
            if let Err(validation_err) =
                crate::shared::tool_descriptors::validate_required_arguments(
                    tool_name,
                    &parsed_args,
                )
            {
                consecutive_failures += 1;
                let err_msg = format!(
                    "Tool '{}' call rejected: {}. Required parameters were not provided.\n\
                     Please provide the required parameters in your next tool call.",
                    tool_name, validation_err
                );
                tracing::warn!(
                    "autonomy_loop: {} (failure {}/{})",
                    err_msg,
                    consecutive_failures,
                    MAX_CONSECUTIVE_TOOL_FAILURES
                );
                round_response.push_str(&format!(
                    "\n[Tool {} validation failed:]\n{}\n",
                    tool_name, err_msg
                ));
                response.push_str(&format!(
                    "\n[Tool {} validation failed:]\n{}\n",
                    tool_name, err_msg
                ));

                // Circuit breaker: break out if too many consecutive failures
                if consecutive_failures >= MAX_CONSECUTIVE_TOOL_FAILURES {
                    let breaker_msg = format!(
                        "\n[Circuit breaker: stopped after {} consecutive tool call failures. \
                         Please re-examine the tool schemas and retry with valid arguments.]",
                        consecutive_failures
                    );
                    round_response.push_str(&breaker_msg);
                    break;
                }
                continue;
            }

            if let Some(tool) = tool_registry.get_arc(tool_name) {
                tracing::info!("autonomy_loop: executing tool {}", tool_name);
                let input = crate::orchestration::tool::ToolInput {
                    task_id: format!("autonomy-{}-{}", iteration, tool_name),
                    phase: "execute".to_string(),
                    agent_role: "assistant".to_string(),
                    objective: params.objective.clone(),
                    constraints: None,
                    evidence: None,
                    payload: parsed_args,
                    allowed_base_dir: None,
                };
                let tool_output = match tool.run_async(input).await {
                    Ok(out) => out,
                    Err(e) => {
                        consecutive_failures += 1;
                        let err_msg = format!("Tool '{}' execution failed: {}", tool_name, e);
                        tracing::warn!("autonomy_loop: {}", err_msg);
                        round_response.push_str(&format!("\n❌ **{}** failed: {}\n", tool_name, e));
                        response.push_str(&format!("\n❌ **{}** failed: {}\n", tool_name, e));
                        if consecutive_failures >= MAX_CONSECUTIVE_TOOL_FAILURES {
                            let breaker = format!(
                                "\n[Circuit breaker: stopped after {} consecutive tool failures]\n",
                                consecutive_failures
                            );
                            round_response.push_str(&breaker);
                            break;
                        }
                        continue;
                    }
                };
                let (tool_summary, tool_body) = format_tool_output(tool_name, &tool_output);

                // ── Send completion notification as visible chunk token ──
                if let Some(ref tx) = config.progress_tx {
                    let _ = tx.send(StreamFrame {
                        event: "chunk",
                        payload: serde_json::json!({
                            "token": tool_summary,
                            "tool_status": "completed",
                        }),
                        status: Some("generating"),
                    });
                }
                round_response.push_str(&tool_body);
                response.push_str(&tool_body);
                consecutive_failures = 0;
            } else {
                tracing::warn!("autonomy_loop: tool '{}' not found in registry", tool_name);
                round_response.push_str(&format!("\n[Tool {} not available]\n", tool_name));
            }
        }

        // If we have a non-empty response (even from a tool), we're done.
        if !round_response.trim().is_empty() && !tool_calls.is_empty() {
            // Tools were executed; a follow-up round will continue with the context
        }
    }

    let total_duration_ms = start.elapsed().as_millis() as u64;
    let total_tools: usize = rounds.iter().map(|r| r.tools_executed.len()).sum();

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
    let last_round_had_tools = rounds.last().is_some_and(|r| !r.tools_executed.is_empty());
    let mut final_response = response;
    if last_round_had_tools {
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

    // If the final response is empty but we have reasoning, use reasoning
    if final_response.trim().is_empty() && !reasoning.trim().is_empty() {
        final_response = reasoning;
    }

    Ok(AutonomyLoopResult {
        response: final_response,
        report: AutonomyLoopReport {
            total_rounds: rounds.len(),
            total_tools,
            final_phase: AutonomyPhase::Completed,
            rounds,
            planner_guidance_used: false,
            trace_alignment_coverage: 0.0,
            total_duration_ms,
            corrective_actions_applied_total: 0,
            corrective_action_effectiveness_ratio: 0.0,
            audit_trail: None,
            stop_reason: if total_tools > 0 {
                "tools_executed".to_string()
            } else {
                "completed".to_string()
            },
        },
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

/// Compute and return a predictive reroute score.
#[allow(dead_code, reason = "reserved for future autonomy loop wiring")]
pub fn compute_predictive_reroute(
    consecutive_failures: u32,
    _avg_latency: f64,
    _avg_success_rate: f64,
    _total_tools: usize,
    _health_score: f64,
) -> RerouteDecision {
    let reroute = consecutive_failures >= 3;
    RerouteDecision {
        should_reroute: reroute,
        score: if reroute { 0.8 } else { 0.0 },
        reason: if reroute {
            Some(format!(
                "{} consecutive failures exceeded threshold",
                consecutive_failures
            ))
        } else {
            None
        },
    }
}

/// Decision result from the predictive reroute analysis.
#[allow(dead_code, reason = "reserved for future autonomy loop wiring")]
pub struct RerouteDecision {
    pub should_reroute: bool,
    pub score: f64,
    pub reason: Option<String>,
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

/// Audit entry for tracking governance events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub event: String,
    pub status: String,
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
            audit_trail: None,
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
            audit_trail: None,
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
                audit_trail: None,
                stop_reason: "no_response".to_string(),
            },
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
            audit_trail: None,
            stop_reason: "completed".to_string(),
        };
        let snapshot = contract_snapshot(&report);
        assert_eq!(snapshot["total_rounds"], 2);
        assert_eq!(snapshot["total_tools"], 5);
        assert_eq!(snapshot["total_duration_ms"], 3000);
        assert_eq!(snapshot["stop_reason"], "completed");
    }

    #[test]
    fn predictive_reroute_does_not_trigger_below_threshold() {
        let decision = compute_predictive_reroute(0, 0.5, 0.3, 2, 0.5);
        assert!(!decision.should_reroute);
    }

    #[test]
    fn predictive_reroute_detects_failure_recovery_when_consecutive_failures_high() {
        let decision = compute_predictive_reroute(3, 0.5, 0.3, 2, 0.5);
        assert!(decision.should_reroute);
        assert!(decision.score > 0.5);
    }

    #[test]
    fn predictive_reroute_threshold_edge() {
        let decision = compute_predictive_reroute(2, 0.5, 0.5, 2, 0.5);
        assert!(!decision.should_reroute);
        let decision = compute_predictive_reroute(3, 0.5, 0.5, 2, 0.5);
        assert!(decision.should_reroute);
    }

    #[test]
    fn build_tool_execution_dag_integrated() {
        let tool_calls: Vec<(String, String)> = vec![
            (
                "read_file".to_string(),
                r#"{"path": "test.txt"}"#.to_string(),
            ),
            ("grep".to_string(), r#"{"pattern": "fn"}"#.to_string()),
            (
                "search_files".to_string(),
                r#"{"query": "test"}"#.to_string(),
            ),
        ];
        let node_ids = crate::orchestration::dag_driver::build_tool_execution_dag(&tool_calls);
        assert!(!node_ids.is_empty());
        let mut node_ids = node_ids;
        node_ids.sort();
        assert_eq!(node_ids[0], "tool-grep-1");
        assert_eq!(node_ids[1], "tool-read_file-0");
        assert_eq!(node_ids[2], "tool-search_files-2");
    }
}
