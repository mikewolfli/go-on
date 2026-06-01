//! Model Selection and Automatic Mode Policies (Phase 10+)
//!
//! This module provides model selection strategies and automatic mode policies
//! for choosing models based on task requirements, complexity, and cost/performance tradeoffs.

use serde::{Deserialize, Serialize};

/// Strategy for automatic model selection
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ModelSelectionStrategy {
    /// Use the most capable model available
    MostCapable,
    /// Use the fastest model available
    Fastest,
    /// Use the cheapest model available
    Cheapest,
    /// Balance cost and capability
    Balanced,
    /// User explicitly selects a model
    Explicit,
}

/// Automatic mode policies for different scenarios
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AutomaticModePolicy {
    /// Always use most capable model regardless of cost
    AlwaysMostCapable,
    /// Use capability-based selection: simple→fast, complex→capable
    AdaptiveCapability,
    /// Minimize cost while meeting minimum capability threshold
    CostOptimized,
    /// Maximize speed (prefer faster models)
    SpeedOptimized,
}

/// Model selection criteria based on task characteristics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionCriteria {
    /// Task complexity level (1-5): 1=simple, 5=very complex)
    pub complexity_level: u8,
    /// Whether task requires vision capabilities
    pub requires_vision: bool,
    /// Whether task requires function calling
    pub requires_function_calling: bool,
    /// Whether task requires code analysis/generation
    pub requires_code: bool,
    /// Minimum acceptable context window size
    pub min_context_window: Option<usize>,
    /// Maximum acceptable cost per request (in cents)
    pub max_cost_cents: Option<u32>,
    /// Whether to prefer speed over capability
    pub prefer_speed: bool,
}

impl SelectionCriteria {
    /// Create minimal selection criteria (simple task, any model)
    pub fn minimal() -> Self {
        Self {
            complexity_level: 1,
            requires_vision: false,
            requires_function_calling: false,
            requires_code: false,
            min_context_window: None,
            max_cost_cents: None,
            prefer_speed: false,
        }
    }

    /// Create selection criteria for complex task requiring best model
    pub fn complex() -> Self {
        Self {
            complexity_level: 5,
            requires_vision: false,
            requires_function_calling: false,
            requires_code: false,
            min_context_window: Some(4096),
            max_cost_cents: None,
            prefer_speed: false,
        }
    }

    /// Create selection criteria for code generation task
    pub fn code_generation() -> Self {
        Self {
            complexity_level: 3,
            requires_vision: false,
            requires_function_calling: false,
            requires_code: true,
            min_context_window: Some(4096),
            max_cost_cents: None,
            prefer_speed: false,
        }
    }

    /// Create selection criteria for speed-critical task
    pub fn fast_response() -> Self {
        Self {
            complexity_level: 2,
            requires_vision: false,
            requires_function_calling: false,
            requires_code: false,
            min_context_window: None,
            max_cost_cents: None,
            prefer_speed: true,
        }
    }
}

/// Model characteristics for selection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCharacteristics {
    /// Model ID
    pub id: String,
    /// Estimated cost per request in cents
    pub cost_per_request_cents: u32,
    /// Approximate latency in milliseconds
    pub latency_ms: u32,
    /// Model capability tier (1-5): 1=basic, 5=most advanced
    pub capability_tier: u8,
    /// Whether model supports vision
    pub supports_vision: bool,
    /// Whether model supports function calling
    pub supports_function_calling: bool,
    /// Whether model excels at code
    pub excels_at_code: bool,
    /// Maximum context window size in tokens
    pub context_window: usize,
}

/// Model selector that implements automatic selection strategies
#[derive(Debug)]
pub struct ModelSelector;

impl ModelSelector {
    /// Select best model based on criteria
    ///
    /// # Arguments
    /// * `criteria` - Selection criteria for the task
    /// * `available_models` - List of available model characteristics
    /// * `strategy` - Selection strategy to use
    ///
    /// # Returns
    /// * `Option<String>` - Selected model ID, or None if no suitable model found
    pub fn select_model(
        criteria: &SelectionCriteria,
        available_models: &[ModelCharacteristics],
        strategy: ModelSelectionStrategy,
    ) -> Option<String> {
        if available_models.is_empty() {
            return None;
        }

        // Filter models that meet minimum requirements (no cloning of references)
        let qualified: Vec<&ModelCharacteristics> = available_models
            .iter()
            .filter(|m| {
                (m.excels_at_code || !criteria.requires_code)
                    && (m.supports_function_calling || !criteria.requires_function_calling)
                    && (m.supports_vision || !criteria.requires_vision)
                    && criteria
                        .min_context_window
                        .is_none_or(|min| m.context_window >= min)
                    && criteria
                        .max_cost_cents
                        .is_none_or(|max| m.cost_per_request_cents <= max)
            })
            .collect();

        if qualified.is_empty() {
            return None;
        }

        match strategy {
            ModelSelectionStrategy::MostCapable => qualified
                .iter()
                .max_by_key(|m| m.capability_tier)
                .map(|m| m.id.clone()),
            ModelSelectionStrategy::Fastest => qualified
                .iter()
                .min_by_key(|m| m.latency_ms)
                .map(|m| m.id.clone()),
            ModelSelectionStrategy::Cheapest => qualified
                .iter()
                .min_by_key(|m| m.cost_per_request_cents)
                .map(|m| m.id.clone()),
            ModelSelectionStrategy::Balanced => {
                // Pre-compute complexity threshold once to avoid redundant calculations
                let is_complex = criteria.complexity_level >= 4;
                qualified
                    .iter()
                    .max_by_key(|m| {
                        // Balanced score computation using pre-computed threshold
                        if is_complex {
                            (m.capability_tier as i32) * 100 - (m.latency_ms as i32)
                        } else {
                            (m.capability_tier as i32) * 100 - (m.cost_per_request_cents as i32 / 2)
                        }
                    })
                    .map(|m| m.id.clone())
            }
            ModelSelectionStrategy::Explicit => None, // User must select explicitly
        }
    }

    /// Suggest strategy based on automatic mode policy
    ///
    /// # Arguments
    /// * `policy` - Automatic mode policy
    /// * `criteria` - Task selection criteria
    ///
    /// # Returns
    /// * `ModelSelectionStrategy` - Recommended selection strategy
    pub fn recommended_strategy(
        policy: &AutomaticModePolicy,
        criteria: &SelectionCriteria,
    ) -> ModelSelectionStrategy {
        match policy {
            AutomaticModePolicy::AlwaysMostCapable => ModelSelectionStrategy::MostCapable,
            AutomaticModePolicy::AdaptiveCapability => {
                if criteria.complexity_level >= 4 || criteria.requires_function_calling {
                    ModelSelectionStrategy::MostCapable
                } else if criteria.prefer_speed {
                    ModelSelectionStrategy::Fastest
                } else {
                    ModelSelectionStrategy::Balanced
                }
            }
            AutomaticModePolicy::CostOptimized => ModelSelectionStrategy::Cheapest,
            AutomaticModePolicy::SpeedOptimized => ModelSelectionStrategy::Fastest,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_selection_most_capable() {
        let models = vec![
            ModelCharacteristics {
                id: "model-1".to_string(),
                cost_per_request_cents: 10,
                latency_ms: 100,
                capability_tier: 3,
                supports_vision: false,
                supports_function_calling: false,
                excels_at_code: false,
                context_window: 4096,
            },
            ModelCharacteristics {
                id: "model-2".to_string(),
                cost_per_request_cents: 50,
                latency_ms: 200,
                capability_tier: 5,
                supports_vision: true,
                supports_function_calling: true,
                excels_at_code: true,
                context_window: 32768,
            },
        ];

        let criteria = SelectionCriteria::complex();
        let selected =
            ModelSelector::select_model(&criteria, &models, ModelSelectionStrategy::MostCapable);

        assert_eq!(selected, Some("model-2".to_string()));
    }

    #[test]
    fn test_model_selection_cheapest() {
        let models = vec![
            ModelCharacteristics {
                id: "expensive".to_string(),
                cost_per_request_cents: 50,
                latency_ms: 100,
                capability_tier: 5,
                supports_vision: true,
                supports_function_calling: true,
                excels_at_code: true,
                context_window: 16384,
            },
            ModelCharacteristics {
                id: "cheap".to_string(),
                cost_per_request_cents: 5,
                latency_ms: 150,
                capability_tier: 2,
                supports_vision: false,
                supports_function_calling: false,
                excels_at_code: false,
                context_window: 4096,
            },
        ];

        let criteria = SelectionCriteria::minimal();
        let selected =
            ModelSelector::select_model(&criteria, &models, ModelSelectionStrategy::Cheapest);

        assert_eq!(selected, Some("cheap".to_string()));
    }

    #[test]
    fn test_model_selection_fastest() {
        let models = vec![
            ModelCharacteristics {
                id: "slow".to_string(),
                cost_per_request_cents: 10,
                latency_ms: 500,
                capability_tier: 5,
                supports_vision: true,
                supports_function_calling: true,
                excels_at_code: true,
                context_window: 16384,
            },
            ModelCharacteristics {
                id: "fast".to_string(),
                cost_per_request_cents: 20,
                latency_ms: 50,
                capability_tier: 3,
                supports_vision: false,
                supports_function_calling: false,
                excels_at_code: false,
                context_window: 8192,
            },
        ];

        let criteria = SelectionCriteria::fast_response();
        let selected =
            ModelSelector::select_model(&criteria, &models, ModelSelectionStrategy::Fastest);

        assert_eq!(selected, Some("fast".to_string()));
    }

    #[test]
    fn test_model_selection_empty_models() {
        let criteria = SelectionCriteria::minimal();
        let selected =
            ModelSelector::select_model(&criteria, &[], ModelSelectionStrategy::MostCapable);

        assert_eq!(selected, None);
    }

    #[test]
    fn test_model_selection_vision_filtering() {
        let models = vec![ModelCharacteristics {
            id: "no-vision".to_string(),
            cost_per_request_cents: 10,
            latency_ms: 100,
            capability_tier: 5,
            supports_vision: false,
            supports_function_calling: false,
            excels_at_code: false,
            context_window: 4096,
        }];

        let criteria = SelectionCriteria {
            requires_vision: true,
            ..SelectionCriteria::minimal()
        };
        let selected =
            ModelSelector::select_model(&criteria, &models, ModelSelectionStrategy::MostCapable);

        assert_eq!(selected, None, "no-vision model should be filtered out");
    }

    #[test]
    fn test_recommended_strategy_adaptive() {
        let policy = AutomaticModePolicy::AdaptiveCapability;

        let complex = SelectionCriteria::complex();
        assert_eq!(
            ModelSelector::recommended_strategy(&policy, &complex),
            ModelSelectionStrategy::MostCapable,
            "complex tasks should use most capable"
        );

        let fast = SelectionCriteria::fast_response();
        assert_eq!(
            ModelSelector::recommended_strategy(&policy, &fast),
            ModelSelectionStrategy::Fastest,
            "fast response should use fastest"
        );

        let simple = SelectionCriteria::minimal();
        assert_eq!(
            ModelSelector::recommended_strategy(&policy, &simple),
            ModelSelectionStrategy::Balanced,
            "simple tasks should use balanced"
        );
    }
}
