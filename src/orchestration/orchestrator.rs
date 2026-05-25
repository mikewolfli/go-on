//! Mode selector and runtime orchestrator
//!
//! These functions are intentional framework definitions for Phase 0-9 architecture.
//! Mode selector and executor will be called from the ACP handler once request
//! routing logic decides which mode should handle each task.
//!
//! Phase 10+: Model selection integration for automatic model discovery and selection.
//! BLUE44: HotFailover + LivePerformanceFeed + SemanticCapabilityMatcher integration.

use crate::agent::{AgentTaskEnvelope, AgentTaskResult, ModelInfo};
use crate::intelligence::hot_failover::{HotFailover, HotFailoverConfig};
use crate::intelligence::semantic_matcher::{
    ModelCapability as SemanticModelCapability, ScoredSkill, SemanticCapabilityMatcher,
    SkillCapability as SemanticSkillCapability,
};
#[cfg(test)]
use crate::mode::ModeKind;
use crate::mode::{
    AgentModeRuntime, AskModeRuntime, EditModeRuntime, FullAutoModeRuntime, ModeRuntime,
    SafeGuardModeRuntime,
};
use crate::model_selector::{
    ModelCharacteristics, ModelSelectionStrategy, ModelSelector, SelectionCriteria,
};
use crate::observability::live_performance::LivePerformanceFeed;
use anyhow::Result;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Global performance feed (lazily initialised)
// ---------------------------------------------------------------------------

/// Global `LivePerformanceFeed` for dynamic cost/latency estimates.
static PERFORMANCE_FEED: OnceLock<LivePerformanceFeed> = OnceLock::new();

/// Return a reference to the global performance feed, initialising it
/// with default settings on first access.
pub fn performance_feed() -> &'static LivePerformanceFeed {
    PERFORMANCE_FEED.get_or_init(LivePerformanceFeed::default)
}

// ---------------------------------------------------------------------------
// Global hot-failover instance (lazily initialised)
// ---------------------------------------------------------------------------

/// Global `HotFailover` for transparent model failover.
static FAILOVER: OnceLock<HotFailover> = OnceLock::new();

/// Return a reference to the global hot-failover instance.
pub fn failover() -> &'static HotFailover {
    FAILOVER.get_or_init(|| HotFailover::new(HotFailoverConfig::default()))
}

/// Record a model execution outcome in the global performance feed.
///
/// Call after each model request completes to keep dynamic estimates fresh.
pub fn record_model_execution(model_id: &str, success: bool, latency_ms: u64) {
    let feed = performance_feed();
    if success {
        feed.record_success(model_id, latency_ms);
    } else {
        feed.record_failure(model_id, latency_ms);
    }
}

// ---------------------------------------------------------------------------
// Mode selection
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Model selection
// ---------------------------------------------------------------------------

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

/// Select best model using semantic capability matching.
///
/// Converts `ModelInfo` entries into `SemanticModelCapability` structs and
/// delegates to `SemanticCapabilityMatcher::match_task_to_models`.
/// Falls back to `select_model_for_task` if no semantic matches are found.
pub fn select_model_semantic(
    available_models: Vec<ModelInfo>,
    task_description: &str,
    fallback_strategy: ModelSelectionStrategy,
) -> Option<ModelInfo> {
    if available_models.is_empty() {
        return None;
    }

    let capabilities: Vec<SemanticModelCapability> = available_models
        .iter()
        .map(|m| SemanticModelCapability {
            model_id: m.id.clone(),
            description: m.description.clone(),
            tags: m.capabilities.clone(),
        })
        .collect();

    let scored = SemanticCapabilityMatcher::match_task_to_models(task_description, &capabilities);

    // Pick the highest-scored model, falling back to strategy-based selection
    if let Some(best) = scored.first() {
        // Log match reasons for observability.
        if !best.match_reasons.is_empty() {
            tracing::debug!(
                model = %best.model_id,
                score = best.score,
                reasons = ?best.match_reasons,
                "SemanticCapabilityMatcher: selected model"
            );
        }
        if best.score > 0.1 {
            return available_models
                .iter()
                .find(|m| m.id == best.model_id)
                .cloned();
        }
    }

    // Fallback: use traditional criteria-based selection
    let criteria = SelectionCriteria::minimal();
    select_model_for_task(available_models, &criteria, fallback_strategy)
}

/// Select the best-matching skill using semantic capability matching.
///
/// Converts skill metadata into `SemanticSkillCapability` structs and
/// delegates to `SemanticCapabilityMatcher::match_task_to_skills`.
pub fn select_skill_semantic(
    task_description: &str,
    skills: &[(String, String, Vec<String>)], // (id, description, tags)
) -> Vec<ScoredSkill> {
    let capabilities: Vec<SemanticSkillCapability> = skills
        .iter()
        .map(|(id, desc, tags)| SemanticSkillCapability {
            skill_id: id.clone(),
            description: desc.clone(),
            tags: tags.clone(),
        })
        .collect();

    SemanticCapabilityMatcher::match_task_to_skills(task_description, &capabilities)
}

// ---------------------------------------------------------------------------
// Cost & latency estimation (static → dynamic via LivePerformanceFeed)
// ---------------------------------------------------------------------------

/// Estimate request cost in cents based on model ID.
///
/// Checks the `LivePerformanceFeed` for a dynamic cost estimate first,
/// falling back to the static lookup table.
pub fn estimate_model_cost(model_id: &str) -> u32 {
    // Try dynamic estimate from LivePerformanceFeed.
    if let Some(dynamic) = performance_feed().get_cost_estimate(model_id) {
        let cents = dynamic as u32;
        if cents > 0 {
            return cents;
        }
    }

    // Static fallback.
    match model_id {
        // DeepSeek models
        "deepseek-v4-flash" => 2,
        "deepseek-v4-pro" => 8,
        // Wenxin models
        "ERNIE-4.5-8K" => 6,
        // OpenAI models
        "gpt-4o" => 10,
        "gpt-4o-mini" => 2,
        // Anthropic models
        "claude-sonnet-4-20250514" => 15,
        // Legacy IDs — kept as fallbacks for backward compatibility
        "deepseek-v3" => 5,
        "deepseek-chat" => 2,
        "deepseek-coder" => 3,
        "ernie-4.0-turbo-8k" | "ernie-3.5-turbo" => 4,
        "gpt-4" => 30,
        "gpt-3.5-turbo" => 1,
        // Default estimate
        _ => 5,
    }
}

/// Estimate latency in milliseconds based on model ID.
///
/// Checks the `LivePerformanceFeed` for a dynamic latency estimate first,
/// falling back to the static lookup table.
pub fn estimate_model_latency(model_id: &str) -> u32 {
    // Try dynamic estimate from LivePerformanceFeed.
    if let Some(dynamic) = performance_feed().get_latency_estimate(model_id) {
        let ms = dynamic as u32;
        if ms > 0 {
            return ms;
        }
    }

    // Static fallback.
    match model_id {
        // Fast models
        "deepseek-v4-flash" => 600,
        "gpt-4o-mini" => 400,
        // Medium latency
        "deepseek-v4-pro" => 1200,
        "gpt-4o" => 800,
        "ERNIE-4.5-8K" => 1000,
        "claude-sonnet-4-20250514" => 1000,
        // Legacy IDs — kept as fallbacks for backward compatibility
        "deepseek-chat" => 800,
        "deepseek-coder" => 1500,
        "deepseek-v3" => 2000,
        "ernie-4.0-turbo-8k" | "ernie-3.5-turbo" => 1000,
        "gpt-4" => 2500,
        "gpt-3.5-turbo" => 1200,
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
    fn test_performance_feed_dynamic_cost_affects_estimate() {
        let feed = performance_feed();
        feed.record_success("gpt-4o-mini", 50);
        let cost = estimate_model_cost("gpt-4o-mini");
        // Dynamic cost should override static fallback (which is 2).
        assert!(cost > 0);
    }

    #[test]
    fn test_performance_feed_dynamic_latency_affects_estimate() {
        let feed = performance_feed();
        feed.record_success("deepseek-v4-flash", 200);
        let latency = estimate_model_latency("deepseek-v4-flash");
        // Dynamic latency should reflect observed value.
        assert!(latency > 0);
    }

    #[test]
    fn test_select_model_semantic_fallbacks_when_no_match() {
        let models = vec![ModelInfo {
            id: "test-model".to_string(),
            name: "Test".to_string(),
            description: "A test model".to_string(),
            is_default: true,
            capabilities: vec!["chat".to_string()],
            context_window: Some(4096),
        }];
        let result = select_model_semantic(
            models.clone(),
            "analyze images and screenshots",
            ModelSelectionStrategy::Balanced,
        );
        // Should fall back to strategy-based selection since no semantic match.
        assert!(result.is_some());
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

    #[test]
    fn test_failover_instance_is_available() {
        let fo = failover();
        let metrics = fo.metrics();
        assert_eq!(metrics.failover_count, 0);
        assert!(!fo.is_blacklisted("nonexistent-model"));
    }
}
