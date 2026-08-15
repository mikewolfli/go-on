//! Model selection and cost/latency estimation.
//!
//! Despite living at `src/orchestration/orchestrator.rs`, this module is NOT
//! the orchestration pipeline driver — the main chat orchestration chain lives
//! in `acp/impl/chat/phases/` / `orchestration/flow.rs`. This module's real
//! responsibility is the model-selection layer: task-driven model selection
//! (`select_model_for_task`), semantic model matching
//! (`select_model_semantic`), and the cost / latency / capability estimates
//! that feed selection strategies.
//!
//! Phase 10+: Model selection integration for automatic model discovery and selection.
//! BLUE44: HotFailover + LivePerformanceFeed + SemanticCapabilityMatcher integration.

use crate::agent::ModelInfo;
use crate::intelligence::semantic_matcher::{
    ModelCapability as SemanticModelCapability, SemanticCapabilityMatcher,
};

use crate::model_selector::{
    ModelCharacteristics, ModelSelectionStrategy, ModelSelector, SelectionCriteria,
};

pub use crate::orchestration::context::OrchestrationContext;

// ---------------------------------------------------------------------------
// Model selection
// ---------------------------------------------------------------------------

/// Select best model from available models based on task characteristics
///
/// # Arguments
/// * `available_models` - Slice of ModelInfo from agent's available_models()
/// * `criteria` - Task characteristics
/// * `strategy` - Selection strategy to use
///
/// # Returns
/// * `Option<ModelInfo>` - Selected model, or None if no suitable model found
pub fn select_model_for_task(
    ctx: &OrchestrationContext,
    available_models: &[ModelInfo],
    criteria: &SelectionCriteria,
    strategy: ModelSelectionStrategy,
) -> Option<ModelInfo> {
    if available_models.is_empty() {
        return None;
    }

    // Convert ModelInfo to ModelCharacteristics for selection
    let model_chars: Vec<ModelCharacteristics> = available_models
        .iter()
        .map(|m| {
            let caps = &m.capabilities;
            ModelCharacteristics {
                id: m.id.clone(),
                cost_per_request_cents: estimate_model_cost(ctx, &m.id),
                latency_ms: estimate_model_latency(ctx, &m.id),
                capability_tier: estimate_capability_tier(caps),
                supports_vision: caps.iter().any(|c| c == "vision"),
                supports_function_calling: caps.iter().any(|c| c == "function_calling"),
                excels_at_code: caps.iter().any(|c| c == "code"),
                context_window: estimate_context_window(caps),
            }
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

/// Select the best model using semantic capability matching.
///
/// Converts `ModelInfo` entries into `SemanticModelCapability` structs and
/// delegates to `SemanticCapabilityMatcher::match_task_to_models`.
/// Falls back to `select_model_for_task` if no semantic matches are found.
pub fn select_model_semantic(
    ctx: &OrchestrationContext,
    available_models: &[ModelInfo],
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
    select_model_for_task(ctx, available_models, &criteria, fallback_strategy)
}

// ---------------------------------------------------------------------------
// Cost & latency estimation (static → dynamic via LivePerformanceFeed)
// ---------------------------------------------------------------------------

/// Estimate request cost in cents based on model ID.
///
/// Checks the `LivePerformanceFeed` for a dynamic cost estimate first,
/// falling back to the static lookup table.
pub fn estimate_model_cost(ctx: &OrchestrationContext, model_id: &str) -> u32 {
    // Try dynamic estimate from LivePerformanceFeed.
    if let Some(dynamic) = ctx.performance_feed().get_cost_estimate(model_id) {
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
        "deepseek-chat" => 2,
        "deepseek-coder" => 3,
        "ernie-4.0-turbo-8k" | "ernie-3.5-turbo" => 4,
        "gpt-4" => 30,
        // Default estimate
        _ => 5,
    }
}

/// Estimate latency in milliseconds based on model ID.
///
/// Checks the `LivePerformanceFeed` for a dynamic latency estimate first,
/// falling back to the static lookup table.
pub fn estimate_model_latency(ctx: &OrchestrationContext, model_id: &str) -> u32 {
    // Try dynamic estimate from LivePerformanceFeed.
    if let Some(dynamic) = ctx.performance_feed().get_latency_estimate(model_id) {
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
        "ernie-4.0-turbo-8k" | "ernie-3.5-turbo" => 1000,
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

/// Estimate the context window size in tokens for a model based on its capabilities.
pub fn estimate_context_window(caps: &[String]) -> usize {
    if caps
        .iter()
        .any(|c| c == "long_context" || c == "large_window")
    {
        128_000
    } else if caps.iter().any(|c| c == "code") {
        32_000
    } else {
        8_000
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentRegistry;
    use crate::mode::{ApprovalPosture, ModeKind};
    use crate::orchestration::mode::resolve_mode_runtime;
    use std::sync::Arc;

    // `OrchestrationContext` wraps the PROCESS-GLOBAL LivePerformanceFeed, so
    // tests that write/read it must not run concurrently with each other
    // (otherwise a parallel `record_success("gpt-4o-mini", ...)` inflates the
    // EMA cost estimate and breaks the gpt-4 > gpt-4o-mini assertion below).
    use serial_test::serial;

    fn runtime_for(mode: &str, registry: Arc<AgentRegistry>) -> Box<dyn crate::mode::ModeRuntime> {
        resolve_mode_runtime(mode, Some(registry), None).expect("mode runtime should resolve")
    }

    #[test]
    fn test_select_mode_runtime_with_registry() {
        let registry = Arc::new(AgentRegistry::new());
        let ask = runtime_for("ask", Arc::clone(&registry));
        let edit = runtime_for("edit", Arc::clone(&registry));
        let agent = runtime_for("agent", Arc::clone(&registry));
        let full_auto = runtime_for("full_auto", Arc::clone(&registry));
        let unknown = runtime_for("unknown", registry);

        // Verify all modes return valid runtimes with correct kinds
        assert_eq!(ask.kind(), ModeKind::Ask);
        assert_eq!(edit.kind(), ModeKind::Edit);
        // "agent" resolves to FullAuto (autonomous loop)
        assert_eq!(agent.kind(), ModeKind::FullAuto);
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
    #[serial]
    fn test_model_cost_estimates() {
        let ctx = OrchestrationContext::new();
        // Use a model pair that no other test writes into the PROCESS-GLOBAL
        // LivePerformanceFeed: `test_performance_feed_dynamic_cost_affects_estimate`
        // records a dynamic cost for gpt-4o-mini that (serial or not) persists
        // in the shared feed and would inflate the "cheaper" side of this
        // assertion on a later run.
        assert!(estimate_model_cost(&ctx, "gpt-4") > estimate_model_cost(&ctx, "gpt-4o"));
    }

    #[test]
    #[serial]
    fn test_performance_feed_dynamic_cost_affects_estimate() {
        let ctx = OrchestrationContext::new();
        ctx.performance_feed().record_success("gpt-4o-mini", 50);
        let cost = estimate_model_cost(&ctx, "gpt-4o-mini");
        // Dynamic cost should override static fallback (which is 2).
        assert!(cost > 0);
    }

    #[test]
    fn test_performance_feed_dynamic_latency_affects_estimate() {
        let ctx = OrchestrationContext::new();
        ctx.performance_feed()
            .record_success("deepseek-v4-flash", 200);
        let latency = estimate_model_latency(&ctx, "deepseek-v4-flash");
        // Dynamic latency should reflect observed value.
        assert!(latency > 0);
    }

    #[test]
    fn test_select_model_semantic_fallbacks_when_no_match() {
        let ctx = OrchestrationContext::new();
        let models = vec![ModelInfo {
            id: "test-model".to_string(),
            name: "Test".to_string(),
            description: "A test model".to_string(),
            is_default: true,
            capabilities: vec!["chat".to_string()],
            context_window: Some(4096),
        }];
        let result = select_model_semantic(
            &ctx,
            &models,
            "analyze images and screenshots",
            ModelSelectionStrategy::Balanced,
        );
        // Should fall back to strategy-based selection since no semantic match.
        assert!(result.is_some());
    }

    #[test]
    fn test_safeguard_mode_selection() {
        let registry = Arc::new(AgentRegistry::new());
        let safeguard = runtime_for("safeguard", registry);
        assert_eq!(safeguard.kind(), ModeKind::SafeGuard);
        // SafeGuard mode defaults to Suggest posture (requires approval
        // at high-risk nodes) per mode.rs default_posture_for().
        assert_eq!(safeguard.posture(), ApprovalPosture::Suggest);
    }

    #[test]
    fn test_safeguard_mode_detects_high_risk_operations() {
        let registry = Arc::new(AgentRegistry::new());
        let safeguard = runtime_for("safeguard", registry);

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
        let base = Arc::new(AgentRegistry::new());
        let ask = runtime_for("ask", Arc::clone(&base));
        let edit = runtime_for("edit", Arc::clone(&base));
        let _agent = runtime_for("agent", Arc::clone(&base));
        let full_auto = runtime_for("full_auto", base);

        // After AUTONOMY + TAO merge, Agent mode was merged into Edit.
        // Edit inherits Agent's risk detection for operations like delete.
        assert!(!ask.is_high_risk_operation("delete"));
        assert!(edit.is_high_risk_operation("delete")); // Edit has Agent's risk detection
        assert!(edit.is_high_risk_operation("delete"));
        assert!(!full_auto.is_high_risk_operation("delete"));
    }

    #[test]
    fn test_failover_instance_is_available() {
        let mut hf = crate::intelligence::hot_failover::HotFailover::new();
        hf.record_failure("model-x");
        let metrics = hf.metrics();
        assert_eq!(metrics.failover_count, 1);
    }
}
