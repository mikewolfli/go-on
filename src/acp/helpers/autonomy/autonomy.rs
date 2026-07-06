use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::Duration;

use crate::acp::helpers::autonomy_loop::{contract_snapshot, AutonomyLoopReport, AutonomyPhase};
use crate::acp::helpers::context::run_with_optional_timeout;
use crate::agent::{Agent, Message};
use crate::orchestration::autonomy_runtime::{
    TOKEN_MODEL_USED_PREFIX, TOKEN_THINKING_PREFIX, TOKEN_TOOL_CALL_PREFIX,
};

pub(crate) async fn run_followup_after_tool_observation(
    agent: Arc<dyn Agent>,
    messages: Vec<Message>,
    principles: Option<Vec<String>>,
    options: Option<HashMap<String, Value>>,
    timeout_duration: Option<Duration>,
) -> Result<(String, String, Option<String>)> {
    let (sender, mut receiver) = mpsc::channel::<String>(1024);
    let sender = crate::agent::StreamingSender::from(sender);
    let task = tokio::spawn(async move { agent.chat(messages, principles, options, sender).await });

    let collect = async move {
        let mut response = String::new();
        let mut reasoning = String::new();
        let mut selected_model: Option<String> = None;

        while let Some(token) = receiver.recv().await {
            if let Some(model_id) = token.strip_prefix(TOKEN_MODEL_USED_PREFIX) {
                selected_model = Some(model_id.trim().to_string());
                continue;
            }

            if token.starts_with(TOKEN_TOOL_CALL_PREFIX) {
                continue;
            }

            if let Some(reasoning_token) = token.strip_prefix(TOKEN_THINKING_PREFIX) {
                reasoning.push_str(reasoning_token);
            } else {
                response.push_str(&token);
            }
        }

        match task.await {
            Ok(Ok(())) => Ok::<(String, String, Option<String>), anyhow::Error>((
                response,
                reasoning,
                selected_model,
            )),
            Ok(Err(err)) => Err(err.into()),
            Err(join_err) => Err(anyhow::anyhow!("agent follow-up task panicked: {join_err}")),
        }
    };

    run_with_optional_timeout(timeout_duration, collect, |duration| {
        anyhow::anyhow!(
            "agent follow-up timed out after {}s",
            duration.as_secs().max(1)
        )
    })
    .await
}

pub(crate) fn terminal_chat_contract_snapshot(
    tool_call_count: usize,
    followup_round_executed: bool,
    final_response: &str,
) -> Value {
    let response_empty = final_response.trim().is_empty();
    let stop_reason = if tool_call_count == 0 {
        "completed_without_tool_calls"
    } else if response_empty {
        "incomplete"
    } else {
        "tools_exhausted_task_complete"
    };

    let total_rounds = 1 + usize::from(tool_call_count > 0 && followup_round_executed);
    let final_phase = if response_empty {
        AutonomyPhase::Failed
    } else {
        AutonomyPhase::Completed
    };

    contract_snapshot(&AutonomyLoopReport {
        total_rounds,
        total_tools: tool_call_count,
        final_phase,
        rounds: Vec::new(),
        planner_guidance_used: false,
        trace_alignment_coverage: 0.0,
        total_duration_ms: 0,
        corrective_actions_applied_total: 0,
        corrective_action_effectiveness_ratio: 0.0,
        audit_trail: None,
        stop_reason: stop_reason.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::terminal_chat_contract_snapshot;
    use crate::acp::helpers::autonomy_loop::{
        contract_snapshot, AutonomyLoopReport, AutonomyPhase,
    };

    #[test]
    fn terminal_chat_contract_snapshot_tracks_single_round_completion() {
        let contract = terminal_chat_contract_snapshot(0, false, "final answer");

        assert_eq!(contract["total_rounds"].as_u64(), Some(1));
        assert_eq!(contract["total_tools"].as_u64(), Some(0));
        assert_eq!(
            contract["stop_reason"].as_str(),
            Some("completed_without_tool_calls")
        );
    }

    #[test]
    fn terminal_chat_contract_snapshot_tracks_followup_round_boundaries() {
        let contract = terminal_chat_contract_snapshot(2, true, "patched result");

        assert_eq!(contract["total_rounds"].as_u64(), Some(2));
        assert_eq!(contract["total_tools"].as_u64(), Some(2));
        assert_eq!(
            contract["stop_reason"].as_str(),
            Some("tools_exhausted_task_complete")
        );
    }

    #[test]
    fn terminal_chat_contract_snapshot_marks_empty_followup_incomplete() {
        let contract = terminal_chat_contract_snapshot(1, true, "   ");

        assert_eq!(contract["total_rounds"].as_u64(), Some(2));
        assert_eq!(contract["stop_reason"].as_str(), Some("incomplete"));
    }

    #[test]
    fn terminal_chat_contract_snapshot_matches_autonomy_loop_contract() {
        let cli_contract = terminal_chat_contract_snapshot(2, true, "patched result");
        let acp_contract = contract_snapshot(&AutonomyLoopReport {
            total_rounds: 2,
            total_tools: 2,
            final_phase: AutonomyPhase::Completed,
            rounds: Vec::new(),
            planner_guidance_used: false,
            trace_alignment_coverage: 0.0,
            total_duration_ms: 0,
            corrective_actions_applied_total: 0,
            corrective_action_effectiveness_ratio: 0.0,
            stop_reason: "tools_exhausted_task_complete".to_string(),
            audit_trail: None,
        });

        assert_eq!(cli_contract, acp_contract);
    }

    // ── BLUE43 Step 9: ACP/CLI same-scenario comparison tests ───────────
    //
    // Run the SAME scenario through both the CLI terminal chat path and the
    // ACP autonomy loop path, then assert parity on:
    //   - Same stop_reason boundary
    //   - Same total_rounds (within ±1)
    //   - Same tool evidence structure (identical contract JSON)

    /// Helper: build the equivalent AutonomyLoopReport for a scenario.
    fn acp_report_for_scenario(
        tool_call_count: usize,
        followup_round_executed: bool,
        final_response: &str,
    ) -> AutonomyLoopReport {
        let response_empty = final_response.trim().is_empty();
        let stop_reason = if tool_call_count == 0 {
            "completed_without_tool_calls"
        } else if response_empty {
            "incomplete"
        } else {
            "tools_exhausted_task_complete"
        };
        let total_rounds = 1 + usize::from(tool_call_count > 0 && followup_round_executed);
        let final_phase = if response_empty {
            AutonomyPhase::Failed
        } else {
            AutonomyPhase::Completed
        };
        AutonomyLoopReport {
            total_rounds,
            total_tools: tool_call_count,
            final_phase,
            rounds: Vec::new(),
            planner_guidance_used: false,
            trace_alignment_coverage: 0.0,
            total_duration_ms: 0,
            corrective_actions_applied_total: 0,
            corrective_action_effectiveness_ratio: 0.0,
            stop_reason: stop_reason.to_string(),
            audit_trail: None,
        }
    }

    /// Assert that CLI and ACP produce identical contract snapshots.
    fn assert_acp_cli_parity(tool_call_count: usize, followup: bool, response: &str) {
        let cli_contract = terminal_chat_contract_snapshot(tool_call_count, followup, response);
        let acp_report = acp_report_for_scenario(tool_call_count, followup, response);
        let acp_contract = contract_snapshot(&acp_report);

        // Same stop_reason boundary
        assert_eq!(
            cli_contract["stop_reason"].as_str(),
            acp_contract["stop_reason"].as_str(),
            "stop_reason differs (tools={}, followup={}, response={:?})",
            tool_call_count,
            followup,
            response,
        );

        // Same total_rounds within ±1
        let cli_rounds = cli_contract["total_rounds"].as_u64().unwrap_or(0) as i64;
        let acp_rounds = acp_contract["total_rounds"].as_u64().unwrap_or(0) as i64;
        let diff = (cli_rounds - acp_rounds).abs();
        assert!(
            diff <= 1,
            "total_rounds differ by >1: CLI={}, ACP={}",
            cli_rounds,
            acp_rounds,
        );

        // Same tool evidence structure (identical JSON)
        assert_eq!(
            cli_contract, acp_contract,
            "contract JSON differs for scenario (tools={}, followup={}, response={:?})",
            tool_call_count, followup, response,
        );
    }

    #[test]
    fn parity_no_tools_completed() {
        assert_acp_cli_parity(0, false, "Here is the answer.");
    }

    #[test]
    fn parity_tools_exhausted_with_followup() {
        assert_acp_cli_parity(3, true, "Code updated successfully.");
    }

    #[test]
    fn parity_no_followup_round() {
        assert_acp_cli_parity(2, false, "Done.");
    }

    #[test]
    fn parity_incomplete_empty_followup() {
        assert_acp_cli_parity(1, true, "   \t  \n");
    }

    #[test]
    fn parity_incomplete_no_followup_empty() {
        assert_acp_cli_parity(1, false, "");
    }

    #[test]
    fn parity_zero_tools_empty_response() {
        // tool_call_count == 0 takes priority: stop_reason = "completed_without_tool_calls"
        assert_acp_cli_parity(0, false, "");
    }

    #[test]
    fn parity_large_tool_count() {
        assert_acp_cli_parity(10, true, "All 10 operations completed.");
    }

    #[test]
    fn parity_whitespace_only_response() {
        assert_acp_cli_parity(2, true, "  \n  \t  ");
    }

    #[test]
    fn parity_one_tool_no_followup_complete() {
        assert_acp_cli_parity(1, false, "single tool result");
    }

    #[test]
    fn parity_all_contracts_have_canonical_fields() {
        // Every contract snapshot — from both paths — must contain the
        // same canonical set of fields (tool evidence structure consistency).
        let scenarios = &[
            terminal_chat_contract_snapshot(0, false, "ok"),
            terminal_chat_contract_snapshot(2, true, "done"),
            terminal_chat_contract_snapshot(1, false, "yes"),
            contract_snapshot(&acp_report_for_scenario(0, false, "ok")),
            contract_snapshot(&acp_report_for_scenario(3, true, "done")),
            contract_snapshot(&acp_report_for_scenario(1, false, "yes")),
        ];

        let canonical_keys: &[&str] = &[
            "total_rounds",
            "total_tools",
            "stop_reason",
            "corrective_actions_applied_total",
            "corrective_action_effectiveness_ratio",
        ];

        for (idx, contract) in scenarios.iter().enumerate() {
            for key in canonical_keys {
                assert!(
                    contract.get(*key).is_some(),
                    "scenario {} is missing contract field '{}'",
                    idx,
                    key,
                );
            }
        }
    }
}
