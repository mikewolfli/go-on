//! # Unified Execution Loop — AutonomyLoop + BrainLoop
//!
//! This module is the **single entry point** for all tool-execution loops
//! in go-on.  It packages two complementary engines into one cohesive API:
//!
//! | Layer | Module | Role |
//! |-------|--------|------|
//! | **Execution** | `acp/helpers/autonomy/autonomy_loop.rs` | Thin agent-driven tool executor: call LLM → parse tool tokens → run tools → loop |
//! | **Orchestration** | `orchestration/brain_loop/` | Plan state machine: Plan → Execute → Reflect → Replan, with DAG, deep reasoning, world model |
//!
//! ## Architecture
//!
//! ```text
//! chat.rs / chat_phases.rs
//!         │
//!         ▼
//!   run_acp_autonomy_loop()    ← YOU ARE HERE — the unified entry point
//!         │
//!         ├─ use_brain_loop=false ──► run_autonomy_loop()    [Execution layer]
//!         │                                  (agent → tools → loop)
//!         └─ use_brain_loop=true  ──► BrainLoop.run_async()  [Orchestration layer]
//!                                            (plan → execute → reflect → replan)
//! ```
//!
//! The two engines are **not** duplicates — they operate at different
//! abstraction levels.  AutonomyLoop handles the raw agent/tool cycle;
//! BrainLoop adds structured plan management on top.  The `use_brain_loop`
//! flag selects which engine drives execution; both return the same
//! [`AutonomyLoopResult`] format so callers are insulated from the choice.

use std::sync::Arc;

use anyhow::Result;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::acp::r#impl::chat::StreamFrame;
use crate::agent::{Agent, Message};

use super::autonomy_loop::{
    run_autonomy_loop, AutonomyLoopConfig, AutonomyLoopParams, AutonomyLoopReport,
    AutonomyLoopResult,
};
use crate::orchestration::brain_loop::{
    BrainLoop, BrainLoopConfig, BrainLoopPhase, BrainLoopProfile, BrainLoopStep, StepStatus,
};
use crate::orchestration::tool::ToolRegistry;

/// Parameters for `run_acp_autonomy_loop`, bundled to avoid clippy `too_many_arguments`.
#[allow(missing_docs)]
pub(crate) struct AcpAutonomyLoopParams {
    pub agent: Arc<dyn Agent>,
    pub tool_registry: Option<Arc<ToolRegistry>>,
    pub messages: Vec<Message>,
    pub acp_session_id: Option<String>,
    pub principles: Option<Vec<String>>,
    pub options: Option<std::collections::HashMap<String, Value>>,
    pub timeout_duration: Option<std::time::Duration>,
    pub stream_tx: Option<mpsc::UnboundedSender<String>>,
    pub progress_sse_tx: Option<mpsc::UnboundedSender<StreamFrame>>,
    /// Operation mode for tool approval events (edit, safeguard, full_auto, etc.)
    pub operation_mode: String,
}

/// Run the multi-round autonomy loop in an ACP-compatible way.
///
/// This function wraps `run_autonomy_loop` while respecting the ACP
/// streaming contract:
///   - Each round's assistant output is streamed to `stream_tx`.
///   - Tool-observation follow-up is handled transparently inside the loop.
///   - The final response, reasoning, and model are returned.
pub(crate) async fn run_acp_autonomy_loop(
    params: AcpAutonomyLoopParams,
) -> Result<AutonomyLoopResult> {
    let objective = extract_objective(&params.messages);
    let option_bool = |key: &str, default: bool| -> bool {
        params
            .options
            .as_ref()
            .and_then(|map| map.get(key))
            .and_then(Value::as_bool)
            .unwrap_or(default)
    };
    let config = AutonomyLoopConfig {
        // Autonomy loop configuration
        max_iterations: 5,
        max_tools_per_round: 8,
        enable_planner_guidance: true,
        enable_trace_alignment: false,
        require_replan_for_complex: true,
        replan_complexity_threshold: 4,
        enable_early_stop: true,
        early_stop_confidence_threshold: 0.85,
        capability_signals: false,
        use_dag_execution: option_bool("enable_dag_execution", true), // DAG on by default for autonomy loop
        enable_agent_reroute: option_bool("enable_agent_reroute", true),
        enable_execution_intelligence: option_bool("enable_metacognitive_feedback", true),
        recovery_orchestrator: Some("auto".to_string()),
        progress_tx: params.progress_sse_tx.clone(),
        max_messages: 200,
        use_brain_loop: option_bool("use_brain_loop", false), // Disabled by default.
        tool_timeout_ms: None,
        max_tool_retries: 2,
        enable_governance_gate: true,
        // Enable persistent loop so the autonomy loop doesn't stop after
        // one text-only round — it continues with a planning prompt to
        // encourage tool-based autonomous execution (like Zed's agent).
        persistent_loop: true,
        operation_mode: params.operation_mode.clone(),
        acp_session_id: params.acp_session_id.clone(),
    };

    let result = if config.use_brain_loop {
        run_acp_autonomy_loop_with_brain_loop(params.agent, &objective, &params.messages, config)
            .await?
    } else {
        let loop_params = AutonomyLoopParams {
            agent: params.agent,
            tool_registry: params.tool_registry,
            objective,
            messages: params.messages,
            principles: params.principles,
            options: params.options,
        };
        run_autonomy_loop(loop_params, config, params.timeout_duration).await?
    };

    // Stream the final response if a channel was provided
    if let Some(tx) = params.stream_tx {
        for chunk in split_for_streaming(&result.response, 256) {
            if tx.send(chunk).is_err() {
                tracing::warn!("autonomy_loop_adapter: streaming receiver disconnected");
                break;
            }
        }
    }

    Ok(result)
}

/// Run the autonomy loop via BrainLoop orchestrator (B51-07).
///
/// Converts the ACP messages and objective into a `BrainLoop` plan,
/// runs the plan → execute → reflect → replan cycle, then converts the
/// resulting [`BrainLoopProfile`] back into [`AutonomyLoopResult`] format.
async fn run_acp_autonomy_loop_with_brain_loop(
    _agent: Arc<dyn Agent>,
    objective: &str,
    messages: &[Message],
    config: AutonomyLoopConfig,
) -> Result<AutonomyLoopResult> {
    // ── Convert messages into BrainLoop steps ────────────────────────
    let steps: Vec<BrainLoopStep> = messages
        .iter()
        .enumerate()
        .map(|(i, msg)| BrainLoopStep {
            id: format!("msg-{}-{}", msg.role, i),
            phase: BrainLoopPhase::Planning,
            description: format!(
                "{} message: {}",
                msg.role,
                msg.content.chars().take(120).collect::<String>()
            ),
            input: msg.content.clone(),
            output: String::new(),
            started_ms: 0,
            completed_ms: 0,
            duration_ms: 0,
            status: StepStatus::Pending,
            context: None,
            depends_on: vec![],
            mode: "auto".to_string(),
            agent: None,
            timeout_seconds: 60,
            parallel_group: None,
        })
        .collect();

    // ── Run the BrainLoop ────────────────────────────────────────────
    let brain_config = BrainLoopConfig {
        max_iterations: config.max_iterations as u32,
        ..Default::default()
    };
    let brain_loop = BrainLoop::new(brain_config);
    let profile: BrainLoopProfile = brain_loop.run_async(objective, steps).await?;

    // ── Convert profile back to AutonomyLoopResult ───────────────────
    Ok(brain_loop_profile_to_result(&profile, objective))
}

/// Convert a [`BrainLoopProfile`] to an [`AutonomyLoopResult`].
fn brain_loop_profile_to_result(profile: &BrainLoopProfile, objective: &str) -> AutonomyLoopResult {
    let response = serde_json::json!({
        "brain_loop": true,
        "objective": objective,
        "total_plans": profile.total_plans,
        "active_plans": profile.active_plans,
        "completed_plans": profile.completed_plans,
        "failed_plans": profile.failed_plans,
        "total_cycles": profile.total_cycles,
        "total_steps": profile.total_steps,
        "avg_cycles_per_plan": profile.avg_cycles_per_plan,
        "avg_step_score": profile.avg_step_score,
        "convergence_info": profile.convergence_info,
    });

    let all_tools_failed = profile.failed_plans > 0 && profile.completed_plans == 0;
    AutonomyLoopResult {
        response: response.to_string(),
        report: AutonomyLoopReport {
            total_rounds: profile.total_cycles as usize,
            total_tools: profile.total_steps as usize,
            final_phase: if profile.failed_plans > 0 {
                super::autonomy_loop::AutonomyPhase::Failed
            } else if profile.completed_plans > 0 {
                super::autonomy_loop::AutonomyPhase::Completed
            } else {
                super::autonomy_loop::AutonomyPhase::Planning
            },
            rounds: Vec::new(),
            planner_guidance_used: false,
            trace_alignment_coverage: 0.0,
            total_duration_ms: 0,
            corrective_actions_applied_total: 0,
            corrective_action_effectiveness_ratio: 0.0,
            stop_reason: if all_tools_failed {
                "all_tools_failed".to_string()
            } else if profile.failed_plans > 0 {
                "failed".to_string()
            } else {
                "completed".to_string()
            },
        },
        all_tools_failed,
    }
}

/// Extract a concise objective from the message list.
fn extract_objective(messages: &[Message]) -> String {
    messages
        .iter()
        .rfind(|m| m.role == "user")
        .map(|m| {
            let text = m.content.trim();
            // Take first 200 chars as the objective
            // Use chars() to avoid panic on multi-byte UTF-8 boundary
            let char_count = text.chars().count();
            if char_count > 200 {
                format!("{}...", text.chars().take(200).collect::<String>())
            } else {
                text.to_string()
            }
        })
        .unwrap_or_else(|| "complete the user request".to_string())
}

/// Split text into chunks for streaming (preserving word boundaries).
/// Uses char_indices to avoid panic on multi-byte UTF-8 boundaries.
fn split_for_streaming(text: &str, chunk_size: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut start_byte = 0;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    if chars.is_empty() {
        return chunks;
    }
    let mut start_idx = 0; // char index
    while start_idx < chars.len() {
        let end_idx = (start_idx + chunk_size).min(chars.len());
        let end_byte = if end_idx < chars.len() {
            // end_byte is the byte offset of the char at end_idx — safe boundary
            chars[end_idx].0
        } else {
            text.len()
        };
        // Try to break at a word boundary (space)
        let break_at = if end_idx < chars.len() {
            // Search backwards in the visible bytes for a space
            let search_bytes = &text.as_bytes()[start_byte..end_byte];
            let last_space = search_bytes
                .iter()
                .rposition(|&b| b == b' ')
                .map(|pos| start_byte + pos + 1);
            match last_space {
                Some(boundary) => boundary,
                None => end_byte,
            }
        } else {
            text.len()
        };
        chunks.push(text[start_byte..break_at].to_string());
        // Advance start_byte to break_at, and start_idx to the corresponding char index
        start_byte = break_at;
        // Find the char index for the new start byte
        match chars.binary_search_by_key(&start_byte, |&(b, _)| b) {
            Ok(idx) => start_idx = idx,
            Err(idx) => start_idx = idx,
        }
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

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
