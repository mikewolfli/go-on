//! Flow management with integrated model selection (Phase 10+)
//!
//! This module extends the flow manager with automatic model selection capabilities,
//! enabling the system to choose optimal models based on task characteristics.

use crate::agent::{Agent, ModelInfo};
use crate::config::AppConfig;
use crate::flow::ResolvedRouting;
use crate::model_selector::{AutomaticModePolicy, ModelSelectionStrategy, SelectionCriteria};
use crate::orchestrator::select_model_for_task;
use anyhow::Result;

/// Extended routing information with selected model
pub struct ResolvedRoutingWithModel {
    /// Original routing information
    pub routing: ResolvedRouting,
    /// Selected model for execution
    pub selected_model: Option<ModelInfo>,
    /// Model selection strategy used
    pub selection_strategy: ModelSelectionStrategy,
    /// Task complexity for diagnostics
    pub task_complexity: u8,
}

pub struct AgentModelSelection {
    pub selected_model: Option<ModelInfo>,
    pub selection_strategy: ModelSelectionStrategy,
    pub task_complexity: u8,
}

/// Helper for model selection in flow management
pub struct FlowModelSelector;

impl FlowModelSelector {
    /// Select model based on automatic policy settings
    ///
    /// # Arguments
    /// * `routing` - Resolved routing information
    /// * `config` - App configuration with model_selection_mode
    /// * `task_description` - Optional task description for complexity analysis
    ///
    /// # Returns
    /// * `Result<ResolvedRoutingWithModel>` - Routing with selected model
    pub fn resolve_with_model_selection(
        routing: ResolvedRouting,
        config: &AppConfig,
        task_description: Option<&str>,
    ) -> Result<ResolvedRoutingWithModel> {
        let agent_selection = routing
            .agents
            .first()
            .map(|(_, agent_arc)| {
                Self::select_model_for_agent(agent_arc.as_ref(), config, task_description)
            })
            .unwrap_or_else(|| AgentModelSelection {
                selected_model: None,
                selection_strategy: Self::selection_strategy(config),
                task_complexity: Self::analyze_task_complexity(task_description),
            });

        Ok(ResolvedRoutingWithModel {
            routing,
            selected_model: agent_selection.selected_model,
            selection_strategy: agent_selection.selection_strategy,
            task_complexity: agent_selection.task_complexity,
        })
    }

    pub fn select_model_for_agent(
        agent: &dyn Agent,
        config: &AppConfig,
        task_description: Option<&str>,
    ) -> AgentModelSelection {
        let task_complexity = Self::analyze_task_complexity(task_description);
        let strategy = Self::selection_strategy(config);
        let selected_model = if agent.supports_model_override() {
            let available = agent.available_models();
            if !available.is_empty() && strategy != ModelSelectionStrategy::Explicit {
                let criteria = Self::build_selection_criteria(task_complexity, task_description);
                select_model_for_task(available, &criteria, strategy.clone())
            } else if !available.is_empty() {
                agent.default_model()
            } else {
                None
            }
        } else {
            None
        };

        AgentModelSelection {
            selected_model,
            selection_strategy: strategy,
            task_complexity,
        }
    }

    fn selection_strategy(config: &AppConfig) -> ModelSelectionStrategy {
        match config.model_selection_mode.as_str() {
            "explicit" => ModelSelectionStrategy::Explicit,
            "capable" => ModelSelectionStrategy::MostCapable,
            "cost" => ModelSelectionStrategy::Cheapest,
            "speed" => ModelSelectionStrategy::Fastest,
            _ => ModelSelectionStrategy::Balanced,
        }
    }

    /// Analyze task complexity from description
    ///
    /// Returns complexity level 1-5 based on keywords and characteristics
    fn analyze_task_complexity(task_description: Option<&str>) -> u8 {
        let desc = task_description.unwrap_or("");
        let lower = desc.to_lowercase();

        // Check for complexity indicators
        let mut complexity = 2u8; // default to medium

        // Increase complexity for complex tasks
        if lower.contains("complex")
            || lower.contains("multi-step")
            || lower.contains("algorithm")
            || lower.contains("refactor")
        {
            complexity = 4;
        }

        // Decrease complexity for simple tasks
        if lower.contains("simple")
            || lower.contains("quick")
            || lower.contains("comment")
            || lower.contains("format")
        {
            complexity = 1;
        }

        // Code-related tasks are generally complex
        if lower.contains("code") || lower.contains("function") {
            complexity = complexity.max(3);
        }

        complexity.clamp(1, 5)
    }

    /// Build selection criteria from task characteristics
    fn build_selection_criteria(
        complexity: u8,
        task_description: Option<&str>,
    ) -> SelectionCriteria {
        let desc = task_description.unwrap_or("");
        let lower = desc.to_lowercase();

        SelectionCriteria {
            complexity_level: complexity,
            requires_vision: lower.contains("image")
                || lower.contains("vision")
                || lower.contains("screenshot"),
            requires_function_calling: lower.contains("function")
                || lower.contains("api")
                || lower.contains("tool"),
            requires_code: lower.contains("code")
                || lower.contains("generate")
                || lower.contains("implement"),
            min_context_window: if complexity >= 4 { Some(4096) } else { None },
            max_cost_cents: None,
            prefer_speed: lower.contains("quick") || lower.contains("fast"),
        }
    }

    /// Get recommended policy based on system configuration
    pub fn recommended_policy(config: &AppConfig) -> AutomaticModePolicy {
        match config.model_selection_mode.as_str() {
            "cost" => AutomaticModePolicy::CostOptimized,
            "speed" => AutomaticModePolicy::SpeedOptimized,
            "capable" => AutomaticModePolicy::AlwaysMostCapable,
            _ => AutomaticModePolicy::AdaptiveCapability,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::agent::{Agent, Message};
    use async_trait::async_trait;
    use serde_json::Value;
    use std::collections::HashMap;

    use super::*;

    struct MockSelectableAgent {
        supports_override: bool,
        models: Vec<ModelInfo>,
    }

    #[async_trait]
    impl Agent for MockSelectableAgent {
        async fn chat(
            &self,
            _messages: Vec<Message>,
            _principles: Option<Vec<String>>,
            _options: Option<HashMap<String, Value>>,
            _sender: crate::agent::StreamingSender,
        ) -> crate::core::error::Result<()> {
            Ok(())
        }

        fn available_models(&self) -> Vec<ModelInfo> {
            self.models.clone()
        }

        fn default_model(&self) -> Option<ModelInfo> {
            self.models.iter().find(|m| m.is_default).cloned()
        }

        fn supports_model_override(&self) -> bool {
            self.supports_override
        }
    }

    fn test_config(mode: &str) -> AppConfig {
        AppConfig {
            default_phase: "coding".to_string(),
            agents: HashMap::new(),
            flow: crate::config::FlowConfig {
                name: "flow".to_string(),
                phases: vec!["coding".to_string()],
            },
            phases: HashMap::new(),
            runtime: None,
            cache: None,
            vector: None,
            autotune: None,
            model_selection_mode: mode.to_string(),
        }
    }

    #[test]
    fn test_analyze_task_complexity() {
        assert_eq!(
            FlowModelSelector::analyze_task_complexity(Some("simple comment")),
            1
        );
        assert_eq!(
            FlowModelSelector::analyze_task_complexity(Some("format code")),
            3
        );
        assert_eq!(
            FlowModelSelector::analyze_task_complexity(Some("complex multi-step algorithm")),
            4
        );
        assert_eq!(
            FlowModelSelector::analyze_task_complexity(Some("refactor this function")),
            4
        );
    }

    #[test]
    fn test_build_selection_criteria() {
        let criteria = FlowModelSelector::build_selection_criteria(
            3,
            Some("generate code with function calls"),
        );
        assert!(criteria.requires_code);
        assert!(criteria.requires_function_calling);
        assert!(!criteria.requires_vision);
    }

    #[test]
    fn test_build_selection_criteria_vision() {
        let criteria =
            FlowModelSelector::build_selection_criteria(2, Some("analyze this screenshot image"));
        assert!(criteria.requires_vision);
    }

    #[test]
    fn test_build_selection_criteria_speed() {
        let criteria = FlowModelSelector::build_selection_criteria(1, Some("quick fix"));
        assert!(criteria.prefer_speed);
    }

    #[test]
    fn select_model_for_agent_uses_override_capability() {
        let agent = MockSelectableAgent {
            supports_override: true,
            models: vec![
                ModelInfo {
                    id: "fast-model".to_string(),
                    name: "Fast".to_string(),
                    description: "fast".to_string(),
                    is_default: true,
                    capabilities: vec!["chat".to_string()],
                    context_window: Some(4096),
                },
                ModelInfo {
                    id: "code-model".to_string(),
                    name: "Code".to_string(),
                    description: "code".to_string(),
                    is_default: false,
                    capabilities: vec!["chat".to_string(), "code".to_string()],
                    context_window: Some(8192),
                },
            ],
        };

        let selection = FlowModelSelector::select_model_for_agent(
            &agent,
            &test_config("capable"),
            Some("implement code generation changes"),
        );

        assert_eq!(
            selection.selected_model.as_ref().map(|m| m.id.as_str()),
            Some("code-model")
        );
    }

    #[test]
    fn select_model_for_agent_skips_providers_without_override() {
        let agent = MockSelectableAgent {
            supports_override: false,
            models: vec![ModelInfo {
                id: "chat-model".to_string(),
                name: "Chat".to_string(),
                description: "chat".to_string(),
                is_default: true,
                capabilities: vec!["chat".to_string()],
                context_window: Some(4096),
            }],
        };

        let selection = FlowModelSelector::select_model_for_agent(
            &agent,
            &test_config("capable"),
            Some("simple chat"),
        );

        assert!(selection.selected_model.is_none());
    }
}
