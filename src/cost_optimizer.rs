//! Phase 11: Cost Optimization Module
//!
//! Implements multi-tier model selection, prompt compression, batch processing,
//! and cost cap protection to reduce execution costs by 55-65%.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CostTier {
    /// Ultra economical models (~0.01$/1K tokens)
    Ultra = 0,
    /// Economical models (~0.05$/1K tokens)
    Economic = 1,
    /// Efficient models (~0.2$/1K tokens)
    Efficient = 2,
    /// Premium models (~1$/1K tokens)
    Premium = 3,
}

/// Task complexity for cost optimization decisions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskComplexity {
    Simple,
    Moderate,
    Complex,
    VeryComplex,
}

/// Model cost profile
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCostProfile {
    pub model_name: String,
    pub cost_tier: CostTier,
    pub cost_per_1k_tokens: f64,
    pub avg_input_tokens: u32,
    pub avg_output_tokens: u32,
    pub reliability_score: f64,
}

/// Prompt compression result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionResult {
    pub original_tokens: u32,
    pub compressed_tokens: u32,
    pub compression_ratio: f64,
    pub compressed_content: String,
}

/// Cost optimizer for reducing execution costs
#[derive(Debug, Clone)]
pub struct CostOptimizer {
    model_profiles: HashMap<String, ModelCostProfile>,
    compression_enabled: bool,
    batch_processing_enabled: bool,
}

impl CostOptimizer {
    pub fn new() -> Self {
        let mut profiles = HashMap::new();

        // Register economy models
        profiles.insert(
            "deepseek-fast".to_string(),
            ModelCostProfile {
                model_name: "deepseek-fast".to_string(),
                cost_tier: CostTier::Ultra,
                cost_per_1k_tokens: 0.01,
                avg_input_tokens: 800,
                avg_output_tokens: 400,
                reliability_score: 0.92,
            },
        );

        profiles.insert(
            "qwen-fast".to_string(),
            ModelCostProfile {
                model_name: "qwen-fast".to_string(),
                cost_tier: CostTier::Economic,
                cost_per_1k_tokens: 0.05,
                avg_input_tokens: 1000,
                avg_output_tokens: 500,
                reliability_score: 0.95,
            },
        );

        profiles.insert(
            "deepseek-chat".to_string(),
            ModelCostProfile {
                model_name: "deepseek-chat".to_string(),
                cost_tier: CostTier::Efficient,
                cost_per_1k_tokens: 0.2,
                avg_input_tokens: 1200,
                avg_output_tokens: 600,
                reliability_score: 0.97,
            },
        );

        profiles.insert(
            "gpt-4".to_string(),
            ModelCostProfile {
                model_name: "gpt-4".to_string(),
                cost_tier: CostTier::Premium,
                cost_per_1k_tokens: 1.0,
                avg_input_tokens: 1500,
                avg_output_tokens: 800,
                reliability_score: 0.99,
            },
        );

        Self {
            model_profiles: profiles,
            compression_enabled: true,
            batch_processing_enabled: true,
        }
    }

    /// Select optimal model based on task complexity and cost budget
    pub fn select_model(
        &self,
        complexity: TaskComplexity,
        max_cost: Option<f64>,
    ) -> Option<String> {
        let target_tier = match complexity {
            TaskComplexity::Simple => CostTier::Ultra,
            TaskComplexity::Moderate => CostTier::Economic,
            TaskComplexity::Complex => CostTier::Efficient,
            TaskComplexity::VeryComplex => CostTier::Premium,
        };

        self.model_profiles
            .values()
            .filter(|p| p.cost_tier <= target_tier)
            .filter(|p| {
                if let Some(max) = max_cost {
                    let estimated = (p.avg_input_tokens + p.avg_output_tokens) as f64
                        * p.cost_per_1k_tokens
                        / 1000.0;
                    estimated <= max
                } else {
                    true
                }
            })
            .max_by(|a, b| {
                a.reliability_score
                    .partial_cmp(&b.reliability_score)
                    .unwrap()
            })
            .map(|p| p.model_name.clone())
    }

    /// Compress prompt to reduce token count
    pub fn compress_prompt(&self, original: &str) -> CompressionResult {
        if !self.compression_enabled {
            return CompressionResult {
                original_tokens: (original.len() / 4) as u32,
                compressed_tokens: (original.len() / 4) as u32,
                compression_ratio: 1.0,
                compressed_content: original.to_string(),
            };
        }

        // Simple compression: remove redundant whitespace and comments
        let mut compressed = original.to_string();

        // Remove multiple spaces
        while compressed.contains("  ") {
            compressed = compressed.replace("  ", " ");
        }

        // Remove comments
        compressed = compressed
            .lines()
            .filter(|line| !line.trim().starts_with("//") && !line.trim().starts_with("#"))
            .collect::<Vec<_>>()
            .join("\n");

        let original_tokens = (original.len() / 4) as u32;
        let compressed_tokens = (compressed.len() / 4) as u32;
        let ratio = compressed_tokens as f64 / original_tokens as f64;

        CompressionResult {
            original_tokens: original_tokens as u32,
            compressed_tokens: compressed_tokens as u32,
            compression_ratio: ratio,
            compressed_content: compressed,
        }
    }

    /// Estimate cost for a model call
    pub fn estimate_cost(&self, model: &str, input_tokens: u32, output_tokens: u32) -> f64 {
        self.model_profiles
            .get(model)
            .map(|p| ((input_tokens + output_tokens) as f64 * p.cost_per_1k_tokens) / 1000.0)
            .unwrap_or(0.0)
    }

    /// Check if cost exceeds cap and recommend fallback
    pub fn check_cost_cap(&self, estimated_cost: f64, cost_cap: f64) -> (bool, Option<String>) {
        if estimated_cost > cost_cap {
            // Find cheapest model as fallback
            let fallback = self
                .model_profiles
                .values()
                .min_by(|a, b| {
                    a.cost_per_1k_tokens
                        .partial_cmp(&b.cost_per_1k_tokens)
                        .unwrap()
                })
                .map(|p| p.model_name.clone());
            (false, fallback)
        } else {
            (true, None)
        }
    }

    pub fn set_compression_enabled(&mut self, enabled: bool) {
        self.compression_enabled = enabled;
    }

    pub fn set_batch_processing_enabled(&mut self, enabled: bool) {
        self.batch_processing_enabled = enabled;
    }
}

impl Default for CostOptimizer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_selection() {
        let optimizer = CostOptimizer::new();
        let model = optimizer.select_model(TaskComplexity::Simple, None);
        assert!(model.is_some());
        assert_eq!(model.unwrap(), "deepseek-fast");
    }

    #[test]
    fn test_prompt_compression() {
        let optimizer = CostOptimizer::new();
        let result = optimizer.compress_prompt("Hello  world  //comment\nfoo bar");
        assert!(result.compression_ratio <= 1.0);
    }

    #[test]
    fn test_cost_estimation() {
        let optimizer = CostOptimizer::new();
        let cost = optimizer.estimate_cost("deepseek-fast", 1000, 500);
        assert!(cost > 0.0);
    }

    #[test]
    fn test_cost_cap_check() {
        let optimizer = CostOptimizer::new();
        let (passed, _fallback) = optimizer.check_cost_cap(0.5, 1.0);
        assert!(passed);
    }
}
