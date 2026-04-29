//! ARCH-10: Promotion Plugin System
//!
//! Pluggable promotion strategies that can be registered with CapabilityBus
//! and influence routing decisions based on agent performance, cost, or
//! other heuristic criteria.

use serde::{Deserialize, Serialize};

/// Outcome of a promotion check
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PromotionDecision {
    /// Promote this agent (increase routing weight)
    Promote,
    /// Demote this agent (decrease routing weight)
    Demote,
    /// No change
    Neutral,
    /// Escalate for human review
    Escalate(String),
}

/// A single promotion criterion
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromotionCriterion {
    pub name: String,
    pub weight: f64,
    pub threshold: f64,
}

/// Plugin trait: each promotion strategy implements this
pub trait PromotionPlugin: Send + Sync {
    fn name(&self) -> &'static str;
    fn evaluate(
        &self,
        agent: &str,
        success_rate: f64,
        avg_latency_ms: f64,
        cost_score: f64,
    ) -> PromotionDecision;
}

/// A simple threshold-based promotion plugin
pub struct ThresholdPromotion {
    pub min_success_rate: f64,
    pub max_latency_ms: f64,
    pub max_cost_score: f64,
}

impl ThresholdPromotion {
    pub fn new(min_success_rate: f64, max_latency_ms: f64, max_cost_score: f64) -> Self {
        Self {
            min_success_rate,
            max_latency_ms,
            max_cost_score,
        }
    }
}

impl Default for ThresholdPromotion {
    fn default() -> Self {
        Self {
            min_success_rate: 0.8,
            max_latency_ms: 5000.0,
            max_cost_score: 0.7,
        }
    }
}

impl PromotionPlugin for ThresholdPromotion {
    fn name(&self) -> &'static str {
        "threshold_promotion"
    }

    fn evaluate(
        &self,
        _agent: &str,
        success_rate: f64,
        avg_latency_ms: f64,
        cost_score: f64,
    ) -> PromotionDecision {
        let mut flags = Vec::new();
        if success_rate < self.min_success_rate {
            flags.push("low_success_rate");
        }
        if avg_latency_ms > self.max_latency_ms {
            flags.push("high_latency");
        }
        if cost_score > self.max_cost_score {
            flags.push("high_cost");
        }

        if flags.len() >= 2 {
            PromotionDecision::Demote
        } else if flags.is_empty() && success_rate >= self.min_success_rate + 0.15 {
            PromotionDecision::Promote
        } else {
            PromotionDecision::Neutral
        }
    }
}

/// Registry of promotion plugins
pub struct PromotionRegistry {
    plugins: Vec<Box<dyn PromotionPlugin>>,
    /// Named criteria used to configure threshold-based plugins.
    pub criteria: Vec<PromotionCriterion>,
}

impl Default for PromotionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl PromotionRegistry {
    pub fn new() -> Self {
        let criteria = vec![
            PromotionCriterion {
                name: "success_rate".to_string(),
                weight: 1.0,
                threshold: 0.8,
            },
            PromotionCriterion {
                name: "latency_ms".to_string(),
                weight: 0.5,
                threshold: 5000.0,
            },
            PromotionCriterion {
                name: "cost_score".to_string(),
                weight: 0.5,
                threshold: 0.7,
            },
        ];
        let mut reg = Self {
            plugins: Vec::new(),
            criteria,
        };
        reg.register(Box::new(ThresholdPromotion::new(0.8, 5000.0, 0.7)));
        reg
    }

    pub fn register(&mut self, plugin: Box<dyn PromotionPlugin>) {
        self.plugins.push(plugin);
    }

    pub fn evaluate_all(
        &self,
        agent: &str,
        success_rate: f64,
        avg_latency_ms: f64,
        cost_score: f64,
    ) -> Vec<PromotionDecision> {
        self.plugins
            .iter()
            .map(|p| p.evaluate(agent, success_rate, avg_latency_ms, cost_score))
            .collect()
    }

    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_threshold_promotion_promotes_high_performer() {
        let plugin = ThresholdPromotion::default();
        let decision = plugin.evaluate("agent-a", 0.98, 500.0, 0.2);
        assert_eq!(decision, PromotionDecision::Promote);
    }

    #[test]
    fn test_threshold_promotion_demotes_low_performer() {
        let plugin = ThresholdPromotion::default();
        let decision = plugin.evaluate("agent-b", 0.5, 10000.0, 0.9);
        assert_eq!(decision, PromotionDecision::Demote);
    }

    #[test]
    fn test_threshold_promotion_neutral_on_single_flag() {
        let plugin = ThresholdPromotion::default();
        let decision = plugin.evaluate("agent-c", 0.7, 500.0, 0.3);
        assert_eq!(decision, PromotionDecision::Neutral);
    }

    #[test]
    fn test_registry_evaluates_all_plugins() {
        let reg = PromotionRegistry::new();
        let decisions = reg.evaluate_all("agent-a", 0.95, 1000.0, 0.3);
        assert_eq!(decisions.len(), 1);
    }
}
