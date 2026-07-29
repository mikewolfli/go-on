//! Flow management with integrated model selection (Phase 10+)
//!
//! This module is now a thin re-export layer. All model selection logic has
//! been merged into [`FlowManager`] in `flow.rs`.
//!
//! **New code should use `FlowManager::resolve_with_model()` directly.**

pub use crate::flow::{AgentModelSelection, ResolvedRoutingWithModel};

use crate::agent::Agent;
use crate::config::AppConfig;
use crate::flow::{FlowManager, ResolvedRouting};
use crate::model_selector::{AutomaticModePolicy, ModelSelectionStrategy, SelectionCriteria};
use crate::orchestration::orchestrator::OrchestrationContext;
use anyhow::Result;

/// Legacy helper for model selection.
///
/// ⚠️ Prefer `FlowManager::resolve_with_model()` in new code.
/// All methods delegate to [`FlowManager`].
pub struct FlowModelSelector;

impl FlowModelSelector {
    /// Select model based on automatic policy settings.
    pub fn resolve_with_model_selection(
        ctx: &OrchestrationContext,
        routing: ResolvedRouting,
        config: &AppConfig,
        task_description: Option<&str>,
    ) -> Result<ResolvedRoutingWithModel> {
        let resolved = if let Some((_, agent)) = routing.agents.first() {
            let selection =
                Self::select_model_for_agent(ctx, agent.as_ref(), config, task_description);
            ResolvedRoutingWithModel {
                routing,
                selected_model: selection.selected_model,
                selection_strategy: selection.selection_strategy,
                task_complexity: selection.task_complexity,
            }
        } else {
            ResolvedRoutingWithModel {
                routing,
                selected_model: None,
                selection_strategy: Self::selection_strategy(config),
                task_complexity: Self::analyze_task_complexity(task_description),
            }
        };
        Ok(resolved)
    }

    /// Select model for a specific agent.
    pub fn select_model_for_agent(
        ctx: &OrchestrationContext,
        agent: &dyn Agent,
        config: &AppConfig,
        task_description: Option<&str>,
    ) -> AgentModelSelection {
        FlowManager::select_model_for_agent(ctx, agent, config, task_description)
    }

    /// Get recommended policy based on system configuration.
    pub fn recommended_policy(config: &AppConfig) -> AutomaticModePolicy {
        FlowManager::recommended_model_policy(config)
    }

    /// Analyze task complexity from description.
    pub fn analyze_task_complexity(task_description: Option<&str>) -> u8 {
        FlowManager::analyze_task_complexity(task_description)
    }

    /// Build selection criteria from task characteristics.
    pub fn build_selection_criteria(
        complexity: u8,
        task_description: Option<&str>,
    ) -> SelectionCriteria {
        FlowManager::build_selection_criteria(complexity, task_description)
    }

    fn selection_strategy(config: &AppConfig) -> ModelSelectionStrategy {
        FlowManager::selection_strategy(config)
    }
}
