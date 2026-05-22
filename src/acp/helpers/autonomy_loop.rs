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
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::acp::helpers::agent_router::record_task_agent_outcome;
use crate::agent::{Agent, Message, StreamingSender};
use crate::orchestration::planner_executor::Planner;
use crate::orchestration::tool::{
    execute_loop, LoopConfig, LoopDecision, ToolInput, ToolOutput, ToolRegistry,
};

use super::autonomy_metrics::{
    record_agent_switch, record_capability_selection_reason, record_explicit_tool_route,
    record_orchestration_alignment, record_parallel_tool_fanout, record_planner_guided_route,
    record_tool_followup_attempt, record_tool_followup_success,
};
use super::execution_intelligence::{post_check, pre_check};
use super::orchestration_alignment::derive_plan_trace_alignment;
use crate::orchestration::capability_signals::CapabilitySignals;

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
    /// BLUE41: Enable early-stop when completion confidence exceeds threshold
    pub enable_early_stop: bool,
    /// BLUE41: Confidence threshold (0.0–1.0) for early-stop decision
    pub early_stop_confidence_threshold: f64,
    /// BLUE41: CapabilityBus signals for structured tool/agent/mode selection
    pub capability_signals: Option<CapabilitySignals>,
    /// BLUE42: Enable DAG-driven tool execution path
    pub use_dag_execution: bool,
    /// BLUE42: Enable adaptive reroute checks after weak rounds
    pub enable_agent_reroute: bool,
    /// BLUE42: Enable metacognitive and world-model feedback hooks
    pub enable_execution_intelligence: bool,
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
            enable_early_stop: true,
            early_stop_confidence_threshold: 0.85,
            capability_signals: None,
            use_dag_execution: false,
            enable_agent_reroute: false,
            enable_execution_intelligence: true,
        }
    }
}

/// A single round of execution in the autonomy loop
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
    /// BLUE41: Round start offset from loop start (ms)
    pub round_start_offset_ms: u64,
    /// BLUE41: Number of tool retries in this round
    pub retry_count: u32,
    /// BLUE41: Why this round ended (tool_complete / max_tools / error / early_stop)
    pub round_stop_reason: String,
    /// BLUE42: Whether adaptive reroute was triggered this round
    pub agent_switched: bool,
    /// BLUE42: Optional reason for reroute trigger
    pub agent_switch_reason: Option<String>,
    /// BLUE42: Candidate agent count visible to this round
    pub candidate_agent_count: u32,
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
    _timeout_duration: Option<std::time::Duration>,
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
        round_start_offset_ms: 0,
        retry_count: 0,
        round_stop_reason: "planned".to_string(),
        agent_switched: false,
        agent_switch_reason: None,
        candidate_agent_count: 1,
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
        let mut agent_switched = false;
        let mut agent_switch_reason: Option<String> = None;

        // BLUE42 Step 5: Pre-check — query metacognitive / world model before execution
        if config.enable_execution_intelligence {
            let pre = super::execution_intelligence::pre_check(
                &format!("autonomy-{}", iteration),
                objective,
            );
            if pre.should_degrade {
                let round_record = AutonomyRound {
                    round_index: iteration + 1,
                    phase: AutonomyPhase::Failed,
                    tools_executed: Vec::new(),
                    planner_guided: false,
                    duration_ms: round_start.elapsed().as_millis() as u64,
                    error: pre.reason.clone(),
                    round_start_offset_ms: start.elapsed().as_millis() as u64,
                    retry_count: 0,
                    round_stop_reason: "degraded_by_execution_intelligence".to_string(),
                    agent_switched: false,
                    agent_switch_reason: None,
                    candidate_agent_count: 0,
                };
                all_rounds.push(round_record);
                break;
            }
        }
        let candidate_agent_count = if config
            .capability_signals
            .as_ref()
            .and_then(|sig| sig.preferred_agent.as_ref())
            .is_some()
        {
            2
        } else {
            1
        };

        let pre = if config.enable_execution_intelligence {
            pre_check("autonomy-loop", "autonomy_agent")
        } else {
            super::execution_intelligence::ExecutionPreCheck {
                should_degrade: false,
                reason: None,
            }
        };
        if pre.should_degrade {
            final_reasoning = pre
                .reason
                .unwrap_or_else(|| "execution intelligence requested degrade".to_string());
            let round_record = AutonomyRound {
                round_index: iteration + 1,
                phase: AutonomyPhase::Failed,
                tools_executed: Vec::new(),
                planner_guided: false,
                duration_ms: round_start.elapsed().as_millis() as u64,
                error: Some("degraded_by_execution_intelligence".to_string()),
                round_start_offset_ms: start.elapsed().as_millis() as u64,
                retry_count: 0,
                round_stop_reason: "degraded".to_string(),
                agent_switched: false,
                agent_switch_reason: None,
                candidate_agent_count,
            };
            all_rounds.push(round_record);
            break;
        }

        // Use capability signals or planner-guided tool preferences
        #[allow(unused_variables)]
        let preferred_tools: Vec<String> = if let Some(ref cap_sig) = config.capability_signals {
            let tools = cap_sig.resolve_tool_preferences(config.max_tools_per_round);
            if !tools.is_empty() {
                planner_guided = true;
                planner_guidance_used = true;
                record_capability_selection_reason("capability_bus_selected");
                tools
            } else {
                Vec::new()
            }
        } else if config.enable_planner_guidance && tool_registry.is_some() {
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
            preferred
        } else {
            Vec::new()
        };

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
                if tool_calls.len() > 1 {
                    record_parallel_tool_fanout(tool_calls.len() as u64);
                }

                let tool_results: Vec<(String, LoopDecision)> = if config.use_dag_execution {
                    let (nodes, _trace) = crate::orchestration::dag_driver::execute_tool_dag(
                        Arc::clone(registry),
                        objective,
                        iteration,
                        &tool_calls,
                    )
                    .await;
                    nodes
                        .iter()
                        .map(|n| {
                            let decision = match &n.state {
                                crate::orchestration::execution_graph::ExNodeState::Completed => {
                                    LoopDecision::Complete(ToolOutput {
                                        success: true,
                                        result: Some(serde_json::json!({})),
                                        error: None,
                                        verification: None,
                                        audit_log: None,
                                        pua_report: None,
                                    })
                                }
                                crate::orchestration::execution_graph::ExNodeState::Failed(
                                    reason,
                                ) => LoopDecision::Failed {
                                    reason: reason.clone(),
                                    last_output: None,
                                },
                                _ => LoopDecision::Failed {
                                    reason: "dag_node_skipped".to_string(),
                                    last_output: None,
                                },
                            };
                            (n.tool_name.clone(), decision)
                        })
                        .collect()
                } else {
                    let tool_jobs = tool_calls
                        .iter()
                        .map(|(tool_name, tool_args_str)| {
                            let registry = Arc::clone(registry);
                            let tool_name = tool_name.clone();
                            let tool_args_str = tool_args_str.clone();
                            let objective_text = objective.to_string();
                            let round_phase = format!("round-{}", iteration);
                            tokio::spawn(async move {
                                let parsed_args: Value = serde_json::from_str(&tool_args_str)
                                    .unwrap_or(serde_json::json!({}));

                                let tool_input = ToolInput {
                                    task_id: "autonomy-loop".to_string(),
                                    phase: round_phase,
                                    agent_role: "autonomy_agent".to_string(),
                                    objective: objective_text,
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

                                let (decision, _trace) = execute_loop(
                                    &tool_name,
                                    &registry,
                                    &tool_input,
                                    &[],
                                    &loop_cfg,
                                );
                                (tool_name, decision)
                            })
                        })
                        .collect::<Vec<_>>();

                    join_all(tool_jobs)
                        .await
                        .into_iter()
                        .filter_map(|task_result| match task_result {
                            Ok(output) => Some(output),
                            Err(err) => {
                                let tool_block =
                                    crate::orchestration::autonomy_runtime::build_tool_result_block(
                                        "tool_exec_runtime",
                                        &format!("tool execution join error: {}", err),
                                        true,
                                    );
                                messages.push(Message {
                                    role: "user".to_string(),
                                    content: tool_block,
                                });
                                None
                            }
                        })
                        .collect()
                };
                for (tool_name, result) in tool_results {
                    round_tools.push(tool_name.clone());

                    match result {
                        LoopDecision::Complete(output) => {
                            let result_text =
                                serde_json::to_string_pretty(&output.result).unwrap_or_default();
                            let tool_block =
                                crate::orchestration::autonomy_runtime::build_tool_result_block(
                                    &tool_name,
                                    &result_text,
                                    false,
                                );
                            messages.push(Message {
                                role: "user".to_string(),
                                content: tool_block,
                            });
                            if config.enable_execution_intelligence {
                                post_check("autonomy-loop", "autonomy_agent", true, &result_text);
                            }
                        }
                        LoopDecision::Failed { reason, .. } => {
                            let tool_block =
                                crate::orchestration::autonomy_runtime::build_tool_result_block(
                                    &tool_name, &reason, true,
                                );
                            messages.push(Message {
                                role: "user".to_string(),
                                content: tool_block,
                            });
                            if config.enable_execution_intelligence {
                                post_check("autonomy-loop", "autonomy_agent", false, &reason);
                            }
                        }
                        other => {
                            let msg = format!("tool loop ended: {:?}", other);
                            let tool_block =
                                crate::orchestration::autonomy_runtime::build_tool_result_block(
                                    &tool_name, &msg, true,
                                );
                            messages.push(Message {
                                role: "user".to_string(),
                                content: tool_block,
                            });
                            if config.enable_execution_intelligence {
                                post_check("autonomy-loop", "autonomy_agent", false, &msg);
                            }
                        }
                    }
                }
            }
        }

        // ── Chain observations into messages for next iteration ──
        // Instead of a separate follow-up agent call, we append the agent's
        // reasoning + tool observations + continuation prompt to `messages`.
        // The next while-loop iteration will naturally give the agent full
        // context: original task → previous reasoning → tool results → prompt.
        // This creates a true reason → tool → observe → replan → finalize chain.
        let tools_were_called = !tool_calls.is_empty();
        if tools_were_called && !response.trim().is_empty() {
            messages.push(Message {
                role: "assistant".to_string(),
                content: response.clone(),
            });
            let continuation = if config.require_replan_for_complex
                && plan.steps.len() >= config.replan_complexity_threshold
            {
                format!(
                    "Tool results above. Task has {} plan steps. \
                     Continue: use more tools if needed, or provide the final answer \
                     when the original task is complete.",
                    plan.steps.len()
                )
            } else {
                "Tool results above. Continue the original task. \
                 Use more tools as needed, then provide the final answer \
                 once the task is complete."
                    .to_string()
            };
            messages.push(Message {
                role: "user".to_string(),
                content: continuation,
            });
            record_tool_followup_attempt();
            record_tool_followup_success();
        }

        // BLUE42 Step 5: Post-check — record outcome into metacognitive / world model
        if config.enable_execution_intelligence && !objective.trim().is_empty() {
            let success = !response.trim().is_empty();
            super::execution_intelligence::post_check(
                &format!("autonomy-{}", iteration),
                objective,
                success,
                &if response.len() > 100 {
                    format!("{}...", &response[..100])
                } else {
                    response.clone()
                },
            );
        }

        // BLUE42 Step 6: Record agent outcome for learning feedback
        record_task_agent_outcome(objective, "autonomy_agent", !response.trim().is_empty());

        final_response = response.clone();
        final_reasoning = reasoning.clone();
        if model_id.is_some() {
            final_model = model_id.clone();
        }

        let round_stop_reason = if !tools_were_called {
            "no_tools_needed"
        } else if iteration >= config.max_iterations {
            "max_iterations_reached"
        } else if response.trim().is_empty() {
            "empty_response"
        } else {
            "tools_completed"
        };
        if config.enable_agent_reroute && tools_were_called && response.trim().is_empty() {
            agent_switched = true;
            agent_switch_reason = Some("failure".to_string());
            record_agent_switch("failure");
        }
        // Save round_tools before moving into round_record
        let rt_for_early_stop = round_tools.clone();

        let round_record = AutonomyRound {
            round_index: iteration + 1,
            phase: if tools_were_called {
                AutonomyPhase::Observing
            } else {
                AutonomyPhase::Finalizing
            },
            tools_executed: round_tools,
            planner_guided,
            duration_ms: round_start.elapsed().as_millis() as u64,
            error: None,
            round_start_offset_ms: start.elapsed().as_millis() as u64,
            retry_count: 0,
            round_stop_reason: round_stop_reason.to_string(),
            agent_switched,
            agent_switch_reason,
            candidate_agent_count,
        };
        all_rounds.push(round_record);

        // Stop if no tools were called — the agent is done
        if !tools_were_called {
            break;
        }

        // BLUE41: Early-stop when completion confidence is high
        // and the response has enough content to be useful.
        if config.enable_early_stop
            && !response.trim().is_empty()
            && response.len() > 100
            && iteration >= 1
        {
            let completed_steps = plan
                .steps
                .iter()
                .filter(|s| {
                    let desc = s.description.to_ascii_lowercase();
                    rt_for_early_stop.iter().any(|t| {
                        desc.contains(t.as_str()) || desc.contains(t.trim_end_matches("_file"))
                    })
                })
                .count();
            let total_steps = plan.steps.len().max(1);
            let completion_ratio = completed_steps as f64 / total_steps as f64;
            if completion_ratio >= config.early_stop_confidence_threshold {
                // High completion confidence — stop early
                break;
            }
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
