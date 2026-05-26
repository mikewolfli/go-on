//! ACP-aware autonomy loop adapter (AUTON-01).
//!
//! Bridges the shared `run_autonomy_loop` into the ACP chat context with
//! streaming output support, so the ACP entrypoint can run multi-round
//! think → act → observe → replan → finalize cycles without bloating
//! the large chat.rs handler.

use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::agent::{Agent, Message};

use super::autonomy::is_execution_like_request;
use super::autonomy_loop::{
    run_autonomy_loop, AutonomyLoopConfig, AutonomyLoopReport, AutonomyLoopResult,
};
use crate::orchestration::full_auto::FullAutoFlow;
use crate::orchestration::skill::SkillRegistry;
use crate::orchestration::tool::ToolRegistry;

/// Run the multi-round autonomy loop in an ACP-compatible way.
///
/// This function wraps `run_autonomy_loop` while respecting the ACP
/// streaming contract:
///   - Each round's assistant output is streamed to `stream_tx`.
///   - Tool-observation follow-up is handled transparently inside the loop.
///   - The final response, reasoning, and model are returned.
pub(crate) async fn run_acp_autonomy_loop(
    agent: Arc<dyn Agent>,
    tool_registry: Option<Arc<ToolRegistry>>,
    messages: Vec<Message>,
    _principles: Option<Vec<String>>,
    options: Option<std::collections::HashMap<String, Value>>,
    timeout_duration: Option<std::time::Duration>,
    stream_tx: Option<mpsc::UnboundedSender<String>>,
) -> Result<AutonomyLoopResult> {
    let objective = extract_objective(&messages);
    let option_bool = |key: &str, default: bool| -> bool {
        options
            .as_ref()
            .and_then(|map| map.get(key))
            .and_then(Value::as_bool)
            .unwrap_or(default)
    };
    let config = AutonomyLoopConfig {
        max_iterations: 5,
        max_tools_per_round: 8,
        enable_planner_guidance: true,
        enable_trace_alignment: false,
        require_replan_for_complex: true,
        replan_complexity_threshold: 4,
        enable_early_stop: true,
        early_stop_confidence_threshold: 0.85,
        capability_signals: None,
        use_dag_execution: option_bool("enable_dag_execution", true), // DAG on by default for autonomy loop
        enable_agent_reroute: option_bool("enable_agent_reroute", true),
        enable_execution_intelligence: option_bool("enable_metacognitive_feedback", true),
        recovery_orchestrator: None,
    };

    let result = run_autonomy_loop(
        agent,
        tool_registry,
        &objective,
        messages,
        config,
        timeout_duration,
    )
    .await?;

    // Stream the final response if a channel was provided
    if let Some(tx) = stream_tx {
        for chunk in split_for_streaming(&result.response, 256) {
            let _ = tx.send(chunk);
        }
    }

    Ok(result)
}

/// Run the FullAutoFlow orchestrator for `full_auto` mode.
///
/// Creates a `FullAutoFlow` instance from the shared skill registry and a
/// new tool registry, then executes the flow against the given task text.
/// Returns an `AutonomyLoopResult` with the execution report embedded as a
/// JSON response string.
pub(crate) async fn run_full_auto_flow(
    skill_registry: Arc<Mutex<SkillRegistry>>,
    task_text: &str,
) -> Result<AutonomyLoopResult> {
    let tool_registry = Arc::new(ToolRegistry::new());
    let mut flow = FullAutoFlow::new(skill_registry, tool_registry);
    let report = flow.run(task_text).await;

    let success = report.is_success();
    let success_count = report.success_count();
    let failure_count = report.failure_count();

    let response = serde_json::json!({
        "flow": "full_auto",
        "status": if success { "success" } else { "partial" },
        "success_count": success_count,
        "failure_count": failure_count,
        "total_duration_ms": report.total_duration_ms,
        "output": report.final_output,
        "errors": report.errors,
        "task_intent": {
            "goals": report.task_intent.goals,
            "constraints": report.task_intent.constraints,
            "prerequisites": report.task_intent.prerequisites,
            "deliverables": report.task_intent.deliverables,
        },
        "matched_skills": report.matched_skills.iter().map(|s| {
            serde_json::json!({
                "name": s.name,
                "score": s.score,
                "reason": s.reason,
            })
        }).collect::<Vec<_>>(),
        "execution_steps": report.execution_log.len(),
    });

    let reasoning = format!(
        "FullAutoFlow: {} skills matched, {} steps executed ({} ok, {} failed) in {}ms",
        report.matched_skills.len(),
        report.execution_log.len(),
        success_count,
        failure_count,
        report.total_duration_ms,
    );

    Ok(AutonomyLoopResult {
        response: response.to_string(),
        reasoning,
        selected_model: None,
        report: AutonomyLoopReport {
            total_rounds: 1,
            total_tools: report.execution_log.len(),
            final_phase: if success {
                super::autonomy_loop::AutonomyPhase::Completed
            } else {
                super::autonomy_loop::AutonomyPhase::Failed
            },
            rounds: report
                .execution_log
                .iter()
                .map(|step| super::autonomy_loop::AutonomyRound {
                    round_index: 0,
                    phase: if step.success {
                        super::autonomy_loop::AutonomyPhase::Executing
                    } else {
                        super::autonomy_loop::AutonomyPhase::Failed
                    },
                    tools_executed: vec![step.skill_name.clone()],
                    planner_guided: false,
                    duration_ms: step.duration_ms,
                    error: step.error.clone(),
                    round_start_offset_ms: step.timestamp_ms,
                    retry_count: 0,
                    round_stop_reason: if step.success {
                        "completed".to_string()
                    } else {
                        "failed".to_string()
                    },
                    agent_switched: false,
                    agent_switch_reason: None,
                    candidate_agent_count: 0,
                    corrective_actions: Vec::new(),
                    corrective_actions_applied: 0,
                    reroute_expected_gain: None,
                    reroute_health_score: None,
                    dag_trace: None,
                })
                .collect(),
            planner_guidance_used: false,
            trace_alignment_coverage: 0.0,
            total_duration_ms: report.total_duration_ms,
            corrective_actions_applied_total: 0,
            corrective_action_effectiveness_ratio: 0.0,
            audit_trail: None,
            stop_reason: if success {
                "completed".to_string()
            } else {
                "partial_failure".to_string()
            },
        },
    })
}

/// Extract a concise objective from the message list.
fn extract_objective(messages: &[Message]) -> String {
    messages
        .iter()
        .rfind(|m| m.role == "user")
        .map(|m| {
            let text = m.content.trim();
            // Take first 200 chars as the objective
            if text.len() > 200 {
                format!("{}...", &text[..200])
            } else {
                text.to_string()
            }
        })
        .unwrap_or_else(|| "complete the user request".to_string())
}

/// Split text into chunks for streaming (preserving word boundaries).
fn split_for_streaming(text: &str, chunk_size: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    while start < bytes.len() {
        let end = (start + chunk_size).min(bytes.len());
        // Try to break at a word boundary
        let break_at = if end < bytes.len() {
            let search_end = end.saturating_sub(1);
            let last_space = bytes[start..=search_end]
                .iter()
                .rposition(|&b| b == b' ')
                .map(|pos| start + pos + 1)
                .unwrap_or(end);
            last_space
        } else {
            end
        };
        chunks.push(text[start..break_at].to_string());
        start = break_at;
    }
    chunks
}

/// Check whether a chat request should use the multi-round autonomy loop.
///
/// Returns `true` when the request is execution-like (either by mode or by
/// message content).  In `full_auto` mode the existing TAO loop with review
/// gating takes precedence for non-execution requests; the autonomy loop
/// is reserved for cases where iterative tool use is clearly beneficial.
pub(crate) fn should_use_acp_autonomy_loop(mode: &str, messages: &[Message]) -> bool {
    // full_auto mode has its own execution path (review gate + TAO loop),
    // so the autonomy loop is only used for non-full_auto execution-like requests.
    // This prevents dual tool execution and preserves the review gate.
    let mode_lower = mode.trim().to_ascii_lowercase();
    if mode_lower == "full_auto" || mode_lower == "full-auto" {
        return false;
    }
    is_execution_like_request(mode, messages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_auto_mode_uses_tao_loop_instead_of_autonomy() {
        // full_auto mode always uses the TAO loop with review gating,
        // regardless of message content. The autonomy loop is reserved
        // for non-full_auto execution-like requests.
        let exec = vec![Message {
            role: "user".to_string(),
            content: "fix the bug in main.rs".to_string(),
        }];
        assert!(!should_use_acp_autonomy_loop("full_auto", &exec));
        assert!(!should_use_acp_autonomy_loop("full-auto", &exec));

        let generic = vec![Message {
            role: "user".to_string(),
            content: "review timeout collision".to_string(),
        }];
        assert!(!should_use_acp_autonomy_loop("full_auto", &generic));
    }

    #[test]
    fn execute_mode_triggers_autonomy_loop() {
        let messages = vec![Message {
            role: "user".to_string(),
            content: "fix the bug in main.rs".to_string(),
        }];
        assert!(should_use_acp_autonomy_loop("execute", &messages));
    }

    #[test]
    fn chat_mode_does_not_trigger_loop_for_generic_queries() {
        let messages = vec![Message {
            role: "user".to_string(),
            content: "what is the meaning of life?".to_string(),
        }];
        // "chat" mode with a non-execution message should not trigger
        assert!(!should_use_acp_autonomy_loop("chat", &messages));
    }

    #[test]
    fn agent_mode_triggers_for_fix_requests() {
        let messages = vec![Message {
            role: "user".to_string(),
            content: "update the configuration file".to_string(),
        }];
        assert!(should_use_acp_autonomy_loop("agent", &messages));
    }

    #[test]
    fn extract_objective_uses_last_user_message() {
        let messages = vec![
            Message {
                role: "assistant".to_string(),
                content: "I'll help".to_string(),
            },
            Message {
                role: "user".to_string(),
                content: "refactor the auth module".to_string(),
            },
        ];
        let obj = extract_objective(&messages);
        assert_eq!(obj, "refactor the auth module");
    }

    #[test]
    fn extract_objective_truncates_long_text() {
        let long = "a".repeat(300);
        let messages = vec![Message {
            role: "user".to_string(),
            content: long.clone(),
        }];
        let obj = extract_objective(&messages);
        assert_eq!(obj.len(), 203); // 200 chars + "..."
        assert!(obj.ends_with("..."));
    }

    #[test]
    fn split_for_streaming_respects_boundaries() {
        let text = "hello world foo bar baz";
        let chunks = split_for_streaming(text, 10);
        assert!(chunks.len() >= 2);
        // Each chunk should be ≤ 10 chars
        for chunk in &chunks {
            assert!(
                chunk.len() <= 10,
                "chunk '{}' is {} chars",
                chunk,
                chunk.len()
            );
        }
        // Concatenating should recover the original text
        let joined: String = chunks.join("");
        assert_eq!(joined, text);
    }
}
