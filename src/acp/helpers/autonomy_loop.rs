//! Unified autonomy loop combining plan → act → observe → replan into one runtime.
//!
//! This module provides a single `AutonomyLoop` that:
//! - Takes a `Planner::plan()` output and a `ToolRegistry`
//! - Drives a multi-round Think → Act → Observe → (Replan) → Finalize cycle
//! - Returns structured `AutonomyLoopReport` with governance metrics
//!
//! All entrypoints (CLI, ACP chat, task.execute, workflow.execute) converge here.

use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::agent::{Agent, Message, StreamingSender};
use crate::orchestration::planner_executor::Planner;
use crate::orchestration::tool::{execute_loop, LoopConfig, LoopDecision, ToolInput, ToolRegistry};

use super::autonomy::run_followup_after_tool_observation;
use super::autonomy_metrics::{
    record_explicit_tool_route, record_orchestration_alignment, record_planner_guided_route,
};
use super::orchestration_alignment::derive_plan_trace_alignment;

/// Autonomy loop state machine phases
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[allow(dead_code)]
pub enum AutonomyPhase {
    /// Initial planning phase — Planner produces ExecutionPlan
    Planning,
    /// Tool execution phase — each plan step maps to tool calls
    Executing,
    /// Observation phase — tool results are collected and structured
    Observing,
    /// Replanning phase — tool observations feed back into plan refinement
    Replanning,
    /// Final answer construction phase
    Finalizing,
    /// Loop completed
    Completed,
    /// Loop failed
    Failed,
}

/// Configuration for the autonomy loop
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AutonomyLoopConfig {
    /// Maximum rounds of plan → execute → observe (excluding planning round)
    pub max_iterations: usize,
    /// Maximum tools per execution round
    pub max_tools_per_round: usize,
    /// Whether to allow planner-guided tool preferences
    pub enable_planner_guidance: bool,
    /// Whether to produce tool-trace alignment analysis
    pub enable_trace_alignment: bool,
    /// Whether finalization requires explicit replan or just single follow-up
    pub require_replan_for_complex: bool,
    /// Minimum plan steps that trigger replan requirement
    pub replan_complexity_threshold: usize,
}

impl Default for AutonomyLoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 3,
            max_tools_per_round: 20,
            enable_planner_guidance: true,
            enable_trace_alignment: true,
            require_replan_for_complex: true,
            replan_complexity_threshold: 3,
        }
    }
}

/// A single round in the autonomy loop
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct AutonomyRound {
    /// Round index (0 = planning, 1+ = execute rounds)
    pub round_index: usize,
    /// Phase of this round
    pub phase: AutonomyPhase,
    /// Tools executed in this round
    pub tools_executed: Vec<String>,
    /// Whether planner guidance was applied
    pub planner_guided: bool,
    /// Duration of this round in ms
    pub duration_ms: u64,
    /// Error message if round failed
    pub error: Option<String>,
}

/// Final report from the autonomy loop
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct AutonomyLoopReport {
    /// Total rounds executed
    pub total_rounds: usize,
    /// Total tools executed across all rounds
    pub total_tools: usize,
    /// Final phase reached
    pub final_phase: AutonomyPhase,
    /// Per-round details
    pub rounds: Vec<AutonomyRound>,
    /// Whether planner guidance was used at any round
    pub planner_guidance_used: bool,
    /// Plan-trace alignment coverage ratio (0.0–1.0)
    pub trace_alignment_coverage: f64,
    /// Total duration in ms
    pub total_duration_ms: u64,
    /// Stop reason
    pub stop_reason: String,
}

/// Result of the autonomy loop execution
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AutonomyLoopResult {
    /// Final response text
    pub response: String,
    /// Reasoning text
    pub reasoning: String,
    /// Selected model (if available)
    pub selected_model: Option<String>,
    /// Loop report with governance metrics
    pub report: AutonomyLoopReport,
}

/// Execute a full autonomy loop: plan → (execute + observe × N) → finalize.
///
/// This is the unified entrypoint for CLI, ACP, task, and workflow.
///
/// # Parameters
/// - `agent`: The agent to use for planning, tool selection, and follow-up
/// - `tool_registry`: Registry of available tools for execution
/// - `objective`: The task objective/description
/// - `additional_context`: Additional messages context
/// - `config`: Loop configuration
/// - `timeout_duration`: Optional timeout per agent round
#[allow(dead_code)]
pub async fn run_autonomy_loop(
    agent: Arc<dyn Agent>,
    tool_registry: Option<Arc<ToolRegistry>>,
    objective: &str,
    additional_context: Vec<Message>,
    config: AutonomyLoopConfig,
    timeout_duration: Option<std::time::Duration>,
) -> Result<AutonomyLoopResult> {
    #[allow(unused_variables)]
    let start = Instant::now();
    let mut all_rounds: Vec<AutonomyRound> = Vec::new();
    let mut planner_guidance_used = false;
    let tool_execution_traces: Vec<Value> = Vec::new();

    // ── Phase 1: Planning ──
    let plan = {
        let envelope = crate::agent::AgentTaskEnvelope {
            task_id: "autonomy-loop".to_string(),
            phase: "planning".to_string(),
            role: "autonomy_planner".to_string(),
            objective: objective.to_string(),
            constraints: None,
            evidence: None,
            input: serde_json::json!({"objective": objective}),
        };
        Planner::plan(&envelope)
    };

    let planning_round = AutonomyRound {
        round_index: 0,
        phase: AutonomyPhase::Planning,
        tools_executed: Vec::new(),
        planner_guided: false,
        duration_ms: start.elapsed().as_millis() as u64,
        error: None,
    };
    all_rounds.push(planning_round);

    // ── Phase 2: Execution rounds ──
    let mut messages = additional_context.clone();
    messages.push(Message {
        role: "user".to_string(),
        content: format!(
            "Task: {}\n\nPlan:\n{}",
            objective,
            plan.steps
                .iter()
                .map(|s| format!("{}. {}", s.step_id, s.description))
                .collect::<Vec<_>>()
                .join("\n")
        ),
    });

    let mut iteration = 0usize;
    let mut final_response = String::new();
    let mut final_reasoning = String::new();
    let mut final_model: Option<String> = None;

    while iteration < config.max_iterations {
        let round_start = Instant::now();
        let mut round_tools: Vec<String> = Vec::new();
        let mut planner_guided = false;

        // Use planner-guided tool preferences when enabled
        if config.enable_planner_guidance && tool_registry.is_some() {
            let preferred = super::autonomy::planner_guided_tool_preferences(
                "autonomy-loop",
                "execute",
                "autonomy_agent",
                objective,
                "",
                config.max_tools_per_round,
            );
            if !preferred.is_empty() {
                planner_guided = true;
                planner_guidance_used = true;
                record_planner_guided_route();
            }
        }

        // Agent chat round to produce response and tool calls
        let (sender, mut receiver) = mpsc::channel::<String>(2048);
        let sender = StreamingSender::from(sender);
        let chat_messages = messages.clone();
        let chat_agent = Arc::clone(&agent);

        let chat_task =
            tokio::spawn(async move { chat_agent.chat(chat_messages, None, None, sender).await });

        let mut response = String::new();
        let mut reasoning = String::new();
        let mut tool_calls: Vec<(String, String)> = Vec::new();
        let mut model_id: Option<String> = None;

        while let Some(token) = receiver.recv().await {
            if let Some(mid) = token.strip_prefix("__model_used__:") {
                model_id = Some(mid.trim().to_string());
                continue;
            }
            if let Some(tc) = token.strip_prefix("__tool_call__:") {
                if let Some(colon_pos) = tc.find(':') {
                    let name = &tc[..colon_pos];
                    let args = &tc[colon_pos + 1..];
                    tool_calls.push((name.to_string(), args.to_string()));
                }
                continue;
            }
            if let Some(rt) = token.strip_prefix("__thinking__") {
                reasoning.push_str(rt);
            } else {
                response.push_str(&token);
            }
        }

        let _ = chat_task.await;

        // ── Tool execution ──
        if !tool_calls.is_empty() {
            if let Some(registry) = tool_registry.as_ref() {
                record_explicit_tool_route();

                for (tool_name, tool_args_str) in &tool_calls {
                    let parsed_args: Value =
                        serde_json::from_str(tool_args_str).unwrap_or(serde_json::json!({}));

                    let tool_input = ToolInput {
                        task_id: "autonomy-loop".to_string(),
                        phase: format!("round-{}", iteration),
                        agent_role: "autonomy_agent".to_string(),
                        objective: objective.to_string(),
                        constraints: None,
                        evidence: None,
                        payload: parsed_args,
                        allowed_base_dir: None,
                    };

                    let loop_cfg = LoopConfig {
                        max_iterations: 1,
                        max_retries_per_tool: 1,
                        enable_fallback: false,
                        verify_output: None,
                    };

                    let (result, _trace) =
                        execute_loop(tool_name, registry, &tool_input, &[], &loop_cfg);

                    round_tools.push(tool_name.clone());

                    match result {
                        LoopDecision::Complete(output) => {
                            let result_text =
                                serde_json::to_string_pretty(&output.result).unwrap_or_default();
                            let tool_block =
                                crate::orchestration::autonomy_runtime::build_tool_result_block(
                                    tool_name,
                                    &result_text,
                                    false,
                                );
                            messages.push(Message {
                                role: "user".to_string(),
                                content: tool_block,
                            });
                        }
                        LoopDecision::Failed { reason, .. } => {
                            let tool_block =
                                crate::orchestration::autonomy_runtime::build_tool_result_block(
                                    tool_name, &reason, true,
                                );
                            messages.push(Message {
                                role: "user".to_string(),
                                content: tool_block,
                            });
                        }
                        other => {
                            let msg = format!("tool loop ended: {:?}", other);
                            let tool_block =
                                crate::orchestration::autonomy_runtime::build_tool_result_block(
                                    tool_name, &msg, true,
                                );
                            messages.push(Message {
                                role: "user".to_string(),
                                content: tool_block,
                            });
                        }
                    }
                }
            }
        }

        // ── Follow-up round ──
        let followup_needed = !tool_calls.is_empty();
        if followup_needed && iteration + 1 < config.max_iterations {
            let followup_messages = vec![
                Message {
                    role: "assistant".to_string(),
                    content: response.clone(),
                },
                Message {
                    role: "user".to_string(),
                    content: "Tool observations above are completed. \
                         Continue the task. If the original task is fully complete, \
                         provide the final answer. Otherwise, use more tools as needed."
                        .to_string(),
                },
            ];

            let followup = run_followup_after_tool_observation(
                Arc::clone(&agent),
                followup_messages,
                None,
                None,
                timeout_duration,
            )
            .await;

            match followup {
                Ok((fr, fr_reasoning, fr_model)) if !fr.trim().is_empty() => {
                    response = fr;
                    if !fr_reasoning.is_empty() {
                        reasoning.push('\n');
                        reasoning.push_str(&fr_reasoning);
                    }
                    if model_id.is_none() {
                        model_id = fr_model;
                    }
                    // Replanning signal for complex tasks
                    if config.require_replan_for_complex
                        && plan.steps.len() >= config.replan_complexity_threshold
                        && iteration + 1 < config.max_iterations
                    {
                        messages.push(Message {
                            role: "assistant".to_string(),
                            content: response.clone(),
                        });
                        messages.push(Message {
                            role: "user".to_string(),
                            content: format!(
                                "The task is complex ({} plan steps). \
                                 Based on current progress, decide if replanning is needed \
                                 or if the task is complete. If complete, give the final answer.",
                                plan.steps.len()
                            ),
                        });
                    }
                }
                _ => {
                    // Follow-up failed — append raw results to response
                    if !response.is_empty() {
                        response.push('\n');
                    }
                    response.push_str("[tool execution completed — integrating results]");
                }
            }
        }

        final_response = response;
        final_reasoning = reasoning;
        if model_id.is_some() {
            final_model = model_id;
        }

        let round_record = AutonomyRound {
            round_index: iteration + 1,
            phase: if followup_needed {
                AutonomyPhase::Observing
            } else {
                AutonomyPhase::Finalizing
            },
            tools_executed: round_tools,
            planner_guided,
            duration_ms: round_start.elapsed().as_millis() as u64,
            error: None,
        };
        all_rounds.push(round_record);

        // Stop if no tools were called — the agent is done
        if tool_calls.is_empty() {
            break;
        }

        iteration += 1;
    }

    // ── Trace alignment ──
    let trace_alignment_coverage =
        if config.enable_trace_alignment && !tool_execution_traces.is_empty() {
            let plan_json = serde_json::to_value(&plan).unwrap_or_default();
            let alignment = derive_plan_trace_alignment(&plan_json, &tool_execution_traces);
            let coverage = alignment
                .get("coverage_ratio")
                .and_then(Value::as_f64)
                .unwrap_or(1.0);
            record_orchestration_alignment(coverage);
            coverage
        } else {
            1.0
        };

    let total_duration_ms = start.elapsed().as_millis() as u64;
    let stop_reason = if iteration == 0 {
        "completed_without_tool_calls"
    } else if iteration >= config.max_iterations {
        "max_iterations_reached"
    } else {
        "tools_exhausted_task_complete"
    };

    let report = AutonomyLoopReport {
        total_rounds: all_rounds.len(),
        total_tools: all_rounds.iter().map(|r| r.tools_executed.len()).sum(),
        final_phase: AutonomyPhase::Completed,
        rounds: all_rounds,
        planner_guidance_used,
        trace_alignment_coverage,
        total_duration_ms,
        stop_reason: stop_reason.to_string(),
    };

    Ok(AutonomyLoopResult {
        response: final_response,
        reasoning: final_reasoning,
        selected_model: final_model,
        report,
    })
}
