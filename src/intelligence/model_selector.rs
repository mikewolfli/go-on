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
}

/// Model selector that implements automatic selection strategies
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

        // Filter models that meet minimum requirements
        let qualified: Vec<_> = available_models
            .iter()
            .filter(|m| {
                if criteria.requires_vision && !m.supports_vision {
                    return false;
                }
                if criteria.requires_function_calling && !m.supports_function_calling {
                    return false;
                }
                if criteria.requires_code && !m.excels_at_code {
                    return false;
                }
                true
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
                // Balanced: compute score = capability + (10 - cost/100) for simple tasks
                // = capability + latency_penalty for complex tasks
                let best = qualified.iter().max_by(|a, b| {
                    let a_score = if criteria.complexity_level >= 4 {
                        (a.capability_tier as i32) * 100 - (a.latency_ms as i32)
                    } else {
                        (a.capability_tier as i32) * 100 - (a.cost_per_request_cents as i32 / 2)
                    };

                    let b_score = if criteria.complexity_level >= 4 {
                        (b.capability_tier as i32) * 100 - (b.latency_ms as i32)
                    } else {
                        (b.capability_tier as i32) * 100 - (b.cost_per_request_cents as i32 / 2)
                    };

                    a_score.cmp(&b_score)
                })?;
                Some(best.id.clone())
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
            },
            ModelCharacteristics {
                id: "model-2".to_string(),
                cost_per_request_cents: 50,
                latency_ms: 200,
                capability_tier: 5,
                supports_vision: true,
                supports_function_calling: true,
                excels_at_code: true,
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
            },
            ModelCharacteristics {
                id: "cheap".to_string(),
                cost_per_request_cents: 5,
                latency_ms: 150,
                capability_tier: 2,
                supports_vision: false,
                supports_function_calling: false,
                excels_at_code: false,
            },
        ];

        let criteria = SelectionCriteria::minimal();
        let selected =
            ModelSelector::select_model(&criteria, &models, ModelSelectionStrategy::Cheapest);

        assert_eq!(selected, Some("cheap".to_string()));
    }
}
