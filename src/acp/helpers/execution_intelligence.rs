use std::collections::HashMap;
use std::sync::OnceLock;

use crate::intelligence::metacognitive::{MetacognitiveConfig, MetacognitiveController};
use crate::intelligence::self_model::{SelfModelConfig, SelfModelCore};
use crate::intelligence::world_model::{EntityType, WorldModel, WorldModelConfig};

#[allow(dead_code)]
pub(crate) struct ExecutionPreCheck {
    pub should_degrade: bool,
    pub reason: Option<String>,
    pub consecutive_failures: u32,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PostCheckOutcome {
    pub corrective_actions: Vec<String>,
}

static META_CTRL: OnceLock<MetacognitiveController> = OnceLock::new();
static WORLD_MODEL: OnceLock<WorldModel> = OnceLock::new();
static SELF_MODEL: OnceLock<SelfModelCore> = OnceLock::new();

fn metacognitive() -> &'static MetacognitiveController {
    META_CTRL.get_or_init(|| MetacognitiveController::new(MetacognitiveConfig::default()))
}

fn world_model() -> &'static WorldModel {
    WORLD_MODEL.get_or_init(|| WorldModel::new(WorldModelConfig::default()))
}

fn self_model() -> &'static SelfModelCore {
    SELF_MODEL.get_or_init(|| SelfModelCore::new(SelfModelConfig::default()))
}

pub(crate) fn should_degrade(limitations_count: usize, consecutive_failures: u32) -> bool {
    limitations_count > 2000 || consecutive_failures >= 3
}

pub(crate) fn pre_check(
    task_id: &str,
    agent: &str,
    consecutive_failures: u32,
) -> ExecutionPreCheck {
    let world = world_model();
    let self_profile = self_model().profile();

    let should_degrade = should_degrade(self_profile.limitations_count, consecutive_failures);
    let reason = if consecutive_failures >= 3 {
        Some(format!("consecutive_failures_{}", consecutive_failures))
    } else if should_degrade {
        Some("self_model_limitations_overflow".to_string())
    } else {
        None
    };

    let mut payload = HashMap::new();
    payload.insert("task_id".to_string(), task_id.to_string());
    payload.insert("agent".to_string(), agent.to_string());
    payload.insert("phase".to_string(), "pre_check".to_string());
    payload.insert(
        "consecutive_failures".to_string(),
        consecutive_failures.to_string(),
    );
    let _ = world.record_event("autonomy_precheck", "execution_intelligence", payload);

    ExecutionPreCheck {
        should_degrade,
        reason,
        consecutive_failures,
    }
}

fn corrective_actions_for_summary(summary: &str) -> Vec<String> {
    let lower = summary.to_ascii_lowercase();
    let mut actions = Vec::new();

    // Detect high-severity patterns first; if found, prepend an escalation action
    let high_severity_keywords = ["critical", "security", "data loss", "crash", "panic"];
    let is_high_severity = high_severity_keywords.iter().any(|kw| lower.contains(kw));
    if is_high_severity {
        actions.push("escalate_and_halt_immediately".to_string());
    }

    if lower.contains("timeout") || lower.contains("timed out") {
        actions.push("reduce_tool_fanout_and_adjust_timeout_budget".to_string());
    }
    if lower.contains("quota") || lower.contains("rate limit") || lower.contains("429") {
        actions.push("switch_to_fallback_agent_or_lower_cost_mode".to_string());
    }
    if lower.contains("empty") || lower.contains("no response") {
        actions.push("request_structured_intermediate_output_before_finalize".to_string());
    }
    if lower.contains("tool execution") || lower.contains("join error") || lower.contains("failed")
    {
        actions.push("retry_with_single_tool_path_then_replan".to_string());
    }

    if actions.is_empty() {
        actions.push("tighten_next_round_constraints_and_replan".to_string());
    }

    actions
}

pub(crate) fn post_check(
    task_id: &str,
    agent: &str,
    success: bool,
    summary: &str,
) -> PostCheckOutcome {
    let world = world_model();
    let _ = world.register_entity(&format!("autonomy-task-{}", task_id), EntityType::System);

    let corrective_actions = if success {
        Vec::new()
    } else {
        corrective_actions_for_summary(summary)
    };

    let mut payload = HashMap::new();
    payload.insert("task_id".to_string(), task_id.to_string());
    payload.insert("agent".to_string(), agent.to_string());
    payload.insert("success".to_string(), success.to_string());
    payload.insert("summary".to_string(), summary.to_string());
    payload.insert(
        "corrective_actions".to_string(),
        corrective_actions.join("|"),
    );
    payload.insert(
        "corrective_action_count".to_string(),
        corrective_actions.len().to_string(),
    );
    let _ = world.record_event("autonomy_postcheck", "execution_intelligence", payload);

    if !success {
        if let Ok(_id) =
            metacognitive().record_observation(task_id, agent, "tool_execution", "high", summary)
        {
            let _ = metacognitive().autoreflect();
        }
    }

    PostCheckOutcome { corrective_actions }
}

#[cfg(test)]
mod tests {
    use super::{corrective_actions_for_summary, post_check, should_degrade};

    #[test]
    fn degrade_threshold_checks_limits_and_failures() {
        assert!(!should_degrade(2000, 0));
        assert!(should_degrade(2001, 0));
        assert!(!should_degrade(0, 2));
        assert!(should_degrade(0, 3));
    }

    #[test]
    fn corrective_actions_cover_timeout_and_empty_response() {
        let actions = corrective_actions_for_summary("tool timeout and empty response from agent");
        assert!(actions
            .iter()
            .any(|a| a == "reduce_tool_fanout_and_adjust_timeout_budget"));
        assert!(actions
            .iter()
            .any(|a| a == "request_structured_intermediate_output_before_finalize"));
    }

    #[test]
    fn post_check_returns_actions_only_for_failures() {
        let fail = post_check("task-a", "agent-a", false, "join error after timeout");
        assert!(!fail.corrective_actions.is_empty());

        let ok = post_check("task-a", "agent-a", true, "all good");
        assert!(ok.corrective_actions.is_empty());
    }

    #[test]
    fn high_severity_patterns_produce_escalation_action() {
        let actions =
            corrective_actions_for_summary("critical security failure with data loss risk");
        assert!(actions
            .iter()
            .any(|a| a.contains("escalate") || a.contains("halt")));
    }
}
