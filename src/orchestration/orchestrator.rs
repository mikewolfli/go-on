//! Mode selector and runtime orchestrator
//!
//! These functions are intentional framework definitions for Phase 0-9 architecture.
//! Mode selector and executor will be called from the ACP handler once request
//! routing logic decides which mode should handle each task.
//!
//! Phase 10+: Model selection integration for automatic model discovery and selection.

use crate::agent::{AgentTaskEnvelope, AgentTaskResult, ModelInfo};
#[allow(unused_imports)]
use crate::mode::{
    AgentModeRuntime, AskModeRuntime, EditModeRuntime, FullAutoModeRuntime, ModeKind, ModeRuntime,
    SafeGuardModeRuntime,
};
use crate::model_selector::{
    ModelCharacteristics, ModelSelectionStrategy, ModelSelector, SelectionCriteria,
};
use anyhow::Result;

/// Select mode runtime based on mode string
pub fn select_mode_runtime(mode: &str) -> Box<dyn ModeRuntime> {
    match mode {
        "ask" => Box::new(AskModeRuntime::default()),
        "edit" => Box::new(EditModeRuntime::default()),
        "agent" => Box::new(AgentModeRuntime::default()),
        "full_auto" => Box::new(FullAutoModeRuntime::default()),
        "safeguard" => Box::new(SafeGuardModeRuntime::default()),
        _ => Box::new(AskModeRuntime::default()), // default to ask
    }
}

/// Execute task using selected mode
pub fn execute_with_mode(mode: &str, task: AgentTaskEnvelope) -> Result<AgentTaskResult> {
    let runtime = select_mode_runtime(mode);
    runtime.run(task)
}

/// Select best model from available models based on task characteristics
///
/// # Arguments
/// * `available_models` - Vector of ModelInfo from agent's available_models()
/// * `criteria` - Task characteristics
/// * `strategy` - Selection strategy to use
///
/// # Returns
/// * `Option<ModelInfo>` - Selected model, or None if no suitable model found
pub fn select_model_for_task(
    available_models: Vec<ModelInfo>,
    criteria: &SelectionCriteria,
    strategy: ModelSelectionStrategy,
) -> Option<ModelInfo> {
    if available_models.is_empty() {
        return None;
    }

    // Convert ModelInfo to ModelCharacteristics for selection
    let model_chars: Vec<ModelCharacteristics> = available_models
        .iter()
        .map(|m| ModelCharacteristics {
            id: m.id.clone(),
            cost_per_request_cents: estimate_model_cost(&m.id),
            latency_ms: estimate_model_latency(&m.id),
            capability_tier: estimate_capability_tier(&m.capabilities),
            supports_vision: m.capabilities.contains(&"vision".to_string()),
            supports_function_calling: m.capabilities.contains(&"function_calling".to_string()),
            excels_at_code: m.capabilities.contains(&"code".to_string()),
        })
        .collect();

    // Use ModelSelector to find best model
    let selected_id = ModelSelector::select_model(criteria, &model_chars, strategy)?;

    // Return the ModelInfo for the selected model
    available_models
        .iter()
        .find(|m| m.id == selected_id)
        .cloned()
}

/// Estimate request cost in cents based on model ID
pub fn estimate_model_cost(model_id: &str) -> u32 {
    match model_id {
        // DeepSeek models
        "deepseek-v3" => 5,
        "deepseek-chat" => 2,
        "deepseek-coder" => 3,
        // Wenxin models
        "ernie-4.0-turbo-8k" => 8,
        "ernie-3.5-turbo" => 4,
        // OpenAI models
        "gpt-4" => 30,
        "gpt-3.5-turbo" => 1,
        // Default estimate
        _ => 5,
    }
}

/// Estimate latency in milliseconds based on model ID
pub fn estimate_model_latency(model_id: &str) -> u32 {
    match model_id {
        // Fast models
        "deepseek-chat" => 800,
        "ernie-3.5-turbo" => 900,
        "gpt-3.5-turbo" => 1200,
        // Medium latency
        "deepseek-coder" => 1500,
        // Slow/capable models
        "deepseek-v3" => 2000,
        "ernie-4.0-turbo-8k" => 2200,
        "gpt-4" => 2500,
        // Default estimate
        _ => 1500,
    }
}

/// Estimate capability tier (1-5) based on capabilities list
pub fn estimate_capability_tier(capabilities: &[String]) -> u8 {
    let mut score = 2u8; // base score

    if capabilities.iter().any(|c| c == "vision") {
        score += 1;
    }
    if capabilities.iter().any(|c| c == "function_calling") {
        score += 1;
    }
    if capabilities.iter().any(|c| c == "code") {
        score += 1;
    }

    score.min(5) // cap at 5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_select_mode_runtime() {
        let ask = select_mode_runtime("ask");
        let edit = select_mode_runtime("edit");
        let agent = select_mode_runtime("agent");
        let full_auto = select_mode_runtime("full_auto");
        let unknown = select_mode_runtime("unknown");

        // Verify all modes return valid runtimes with correct kinds
        assert_eq!(ask.kind(), ModeKind::Ask);
        assert_eq!(edit.kind(), ModeKind::Edit);
        assert_eq!(agent.kind(), ModeKind::Agent);
        assert_eq!(full_auto.kind(), ModeKind::FullAuto);
        // unknown should default to ask
        assert_eq!(unknown.kind(), ModeKind::Ask);
    }

    #[test]
    fn test_capability_tier_estimation() {
        let no_caps = vec![];
        assert_eq!(estimate_capability_tier(&no_caps), 2);

        let vision_only = vec!["vision".to_string()];
        assert_eq!(estimate_capability_tier(&vision_only), 3);

        let all_caps = vec![
            "vision".to_string(),
            "function_calling".to_string(),
            "code".to_string(),
        ];
        assert_eq!(estimate_capability_tier(&all_caps), 5);
    }

    #[test]
    fn test_model_cost_estimates() {
        assert!(estimate_model_cost("gpt-4") > estimate_model_cost("gpt-3.5-turbo"));
        assert!(estimate_model_cost("deepseek-v3") > estimate_model_cost("deepseek-chat"));
    }

    #[test]
    fn test_safeguard_mode_selection() {
        let safeguard = select_mode_runtime("safeguard");
        assert_eq!(safeguard.kind(), ModeKind::SafeGuard);
        assert!(!safeguard.user_approval_required()); // Base requirement is false
    }

    #[test]
    fn test_safeguard_mode_detects_high_risk_operations() {
        let safeguard = select_mode_runtime("safeguard");

        // Should detect delete operations
        assert!(safeguard.is_high_risk_operation("delete user data"));
        assert!(safeguard.is_high_risk_operation("remove old files"));

        // Should detect database operations
        assert!(safeguard.is_high_risk_operation("drop table users"));
        assert!(safeguard.is_high_risk_operation("drop database production"));

        // Should detect rollback/revert
        assert!(safeguard.is_high_risk_operation("rollback changes"));
        assert!(safeguard.is_high_risk_operation("revert commit"));

        // Should NOT flag safe operations
        assert!(!safeguard.is_high_risk_operation("read file"));
        assert!(!safeguard.is_high_risk_operation("run tests"));
        assert!(!safeguard.is_high_risk_operation("apply patch"));
    }

    #[test]
    fn test_other_modes_dont_flag_high_risk() {
        let ask = select_mode_runtime("ask");
        let edit = select_mode_runtime("edit");
        let agent = select_mode_runtime("agent");
        let full_auto = select_mode_runtime("full_auto");

        // Only Agent mode has some high-risk detection (for delete operations)
        // but Ask, Edit, and FullAuto rely on approval settings instead
        assert!(!ask.is_high_risk_operation("delete"));
        assert!(!edit.is_high_risk_operation("delete"));
        assert!(agent.is_high_risk_operation("delete")); // Agent mode has risk detection
        assert!(!full_auto.is_high_risk_operation("delete"));
    }
}
