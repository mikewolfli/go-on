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

use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::agent::{Agent, Message, StreamingSender};
use crate::orchestration::autonomy_runtime::{
    parse_tool_call_token, TOKEN_MODEL_USED_PREFIX, TOKEN_THINKING_PREFIX,
};
use crate::orchestration::tool::ToolRegistry;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

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
    pub max_messages: usize,
    pub replan_complexity_threshold: u8,
    pub enable_early_stop: bool,
    pub early_stop_confidence_threshold: f64,
    pub capability_signals: bool,
    pub use_dag_execution: bool,
    pub enable_agent_reroute: bool,
    pub recovery_orchestrator: Option<String>,
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
            max_messages: 200,
            replan_complexity_threshold: 5,
            enable_early_stop: true,
            early_stop_confidence_threshold: 0.9,
            capability_signals: false,
            use_dag_execution: true,
            enable_agent_reroute: true,
            recovery_orchestrator: None,
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
    agent: Arc<dyn Agent>,
    tool_registry: Option<Arc<ToolRegistry>>,
    objective: &str,
    messages: Vec<Message>,
    config: AutonomyLoopConfig,
    timeout_duration: Option<std::time::Duration>,
) -> Result<AutonomyLoopResult, anyhow::Error> {
    let start = Instant::now();
    let tool_registry = tool_registry.unwrap_or_else(|| Arc::new(ToolRegistry::new()));

    tracing::debug!(
        target: "autonomy_loop",
        objective = %objective,
        messages = messages.len(),
        max_iterations = config.max_iterations,
        "autonomy loop starting"
    );

    let mut response = String::new();
    let mut reasoning = String::new();
    let mut selected_model: Option<String> = None;
    let mut rounds: Vec<AutonomyRound> = Vec::new();
    let max_iterations = config.max_iterations.max(1);

    for iteration in 0..max_iterations {
        let round_start = Instant::now();
        let mut tool_calls: Vec<(String, String)> = Vec::new();

        // ── Call agent with streaming ────────────────────────────────
        let (sender, mut receiver) = mpsc::channel::<String>(1024);
        let sender = StreamingSender::from(sender);

        let agent_messages = if iteration == 0 {
            messages.clone()
        } else {
            // For follow-up rounds, the response text serves as context
            vec![Message {
                role: "user".to_string(),
                content: format!(
                    "Continue with the task. Context so far: {}\n\nOriginal objective: {}",
                    response, objective
                ),
            }]
        };

        let agent_clone = Arc::clone(&agent);
        let chat_task = tokio::spawn(async move {
            agent_clone
                .chat(agent_messages, None, None, sender)
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
                            // Model used detection
                            if let Some(model) = t.strip_prefix(TOKEN_MODEL_USED_PREFIX) {
                                selected_model = Some(model.trim().to_string());
                                continue;
                            }
                            // Tool call detection
                            if let Some((tool_name, tool_args)) = parse_tool_call_token(&t) {
                                tool_calls.push((tool_name.to_string(), tool_args.to_string()));
                                continue;
                            }
                            // Reasoning content
                            if let Some(rt) = t.strip_prefix(TOKEN_THINKING_PREFIX) {
                                reasoning.push_str(rt);
                                continue;
                            }
                            // Regular token
                            round_response.push_str(&t);
                            response.push_str(&t);
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
            round_start_offset_ms: (|| {
                round_start
                    .checked_duration_since(start)
                    .unwrap_or(std::time::Duration::from_secs(0))
                    .as_millis() as u64
            })(),
            retry_count: 0,
            round_stop_reason: "completed".to_string(),
            agent_switched: false,
            agent_switch_reason: None,
            trace: Vec::new(),
        });

        // ── Execute tool calls ───────────────────────────────────────
        if tool_calls.is_empty() {
            break; // No tools to execute — final response ready
        }

        if iteration + 1 >= max_iterations {
            break; // Max iterations reached
        }

        for (tool_name, tool_args) in &tool_calls {
            if let Some(tool) = tool_registry.get_arc(tool_name) {
                tracing::info!("autonomy_loop: executing tool {}", tool_name);
                let input = crate::orchestration::tool::ToolInput {
                    task_id: format!("autonomy-{}-{}", iteration, tool_name),
                    phase: "execute".to_string(),
                    agent_role: "assistant".to_string(),
                    objective: objective.to_string(),
                    constraints: None,
                    evidence: None,
                    payload: serde_json::from_str(tool_args).unwrap_or_default(),
                    allowed_base_dir: None,
                };
                let tool_output = tool.run_async(input).await;
                let result_str = format!("{:?}", tool_output);
                round_response
                    .push_str(&format!("\n[Tool {} result:]\n{}\n", tool_name, result_str));
                response.push_str(&format!("\n[Tool {} result:]\n{}\n", tool_name, result_str));
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

    // If the final response is empty but we have reasoning, use reasoning as response
    let final_response = if response.trim().is_empty() && !reasoning.trim().is_empty() {
        reasoning.clone()
    } else {
        response
    };

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
