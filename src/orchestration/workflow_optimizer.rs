
//! ARCH-11: Workflow Optimizer Plugin
//!
//! Pluggable optimization strategies for workflow execution.
//! Registered with CapabilityBus to influence execution policy
//! based on historical success rates, latency patterns, and
//! resource utilization.

use serde::{Deserialize, Serialize};

/// An optimization recommendation produced by a plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationRecommendation {
    /// Strategy name (e.g. "increase_concurrency", "reduce_timeout")
    pub strategy: String,
    /// Expected improvement factor (0.0–1.0)
    pub expected_improvement: f64,
    /// Human-readable explanation
    pub description: String,
}

/// Trait for workflow optimization plugins.
///
/// Each plugin inspects current execution metrics and returns
/// a list of recommendations.  The CapabilityBus aggregates
/// all recommendations and applies the highest-impact ones
/// that are compatible.
pub trait WorkflowOptimizer: Send + Sync {
    fn name(&self) -> &'static str;
    fn optimize(
        &self,
        current_plan: &str,
        success_rate: f64,
        avg_duration_ms: f64,
    ) -> Vec<OptimizationRecommendation>;
}

/// A simple concurrency-based optimizer.
///
/// When success rate is high and duration is low, recommends
/// increasing concurrency.  When duration is high, recommends
/// parallelizing independent steps.
pub struct ConcurrencyOptimizer {
    pub min_success_rate: f64,
    pub max_duration_ms: f64,
}

impl Default for ConcurrencyOptimizer {
    fn default() -> Self {
        Self {
            min_success_rate: 0.85,
            max_duration_ms: 10_000.0,
        }
    }
}

impl WorkflowOptimizer for ConcurrencyOptimizer {
    fn name(&self) -> &'static str {
        "concurrency_optimizer"
    }

    fn optimize(
        &self,
        _current_plan: &str,
        success_rate: f64,
        avg_duration_ms: f64,
    ) -> Vec<OptimizationRecommendation> {
        let mut recs = Vec::new();

        if success_rate >= self.min_success_rate && avg_duration_ms < self.max_duration_ms {
            recs.push(OptimizationRecommendation {
                strategy: "increase_concurrency".to_string(),
                expected_improvement: 0.3,
                description: format!(
                    "High success rate ({:.1}%) and low latency ({:.0}ms) — increase concurrency by 1",
                    success_rate * 100.0,
                    avg_duration_ms
                ),
            });
        }

        if avg_duration_ms > self.max_duration_ms {
            recs.push(OptimizationRecommendation {
                strategy: "parallelize_steps".to_string(),
                expected_improvement: 0.5,
                description: format!(
                    "High latency ({:.0}ms > {:.0}ms threshold) — parallelize independent steps",
                    avg_duration_ms, self.max_duration_ms
                ),
            });
        }

        recs
    }
}

/// A cost-aware optimizer that recommends model downgrades
/// when success rate is high enough to tolerate cheaper models.
pub struct CostOptimizer {
    pub min_success_rate: f64,
    pub cost_factor: f64,
}

impl Default for CostOptimizer {
    fn default() -> Self {
        Self {
            min_success_rate: 0.90,
            cost_factor: 0.7,
        }
    }
}

impl WorkflowOptimizer for CostOptimizer {
    fn name(&self) -> &'static str {
        "cost_optimizer"
    }

    fn optimize(
        &self,
        _current_plan: &str,
        success_rate: f64,
        _avg_duration_ms: f64,
    ) -> Vec<OptimizationRecommendation> {
        if success_rate >= self.min_success_rate {
            vec![OptimizationRecommendation {
                strategy: "downgrade_model_tier".to_string(),
                expected_improvement: self.cost_factor,
                description: format!(
                    "High success rate ({:.1}%) — consider downgrading model tier to reduce cost by {:.0}%",
                    success_rate * 100.0,
                    self.cost_factor * 100.0
                ),
            }]
        } else {
            vec![]
        }
    }
}

/// Registry of workflow optimizers — used by CapabilityBus
/// to collect recommendations before each routing decision.
#[derive(Default)]
pub struct OptimizerRegistry {
    optimizers: Vec<Box<dyn WorkflowOptimizer>>,
}

impl OptimizerRegistry {
    pub fn new() -> Self {
        let mut reg = Self::default();
        reg.register(Box::new(ConcurrencyOptimizer::default()));
        reg.register(Box::new(CostOptimizer::default()));
        reg
    }

    pub fn register(&mut self, optimizer: Box<dyn WorkflowOptimizer>) {
        self.optimizers.push(optimizer);
    }

    /// Run all registered optimizers and return aggregated recommendations.
    pub fn optimize_all(
        &self,
        current_plan: &str,
        success_rate: f64,
        avg_duration_ms: f64,
    ) -> Vec<OptimizationRecommendation> {
        let mut all = Vec::new();
        for opt in &self.optimizers {
            all.extend(opt.optimize(current_plan, success_rate, avg_duration_ms));
        }
        all.sort_by(|a, b| {
            b.expected_improvement
                .partial_cmp(&a.expected_improvement)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        all
    }

    pub fn count(&self) -> usize {
        self.optimizers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_concurrency_optimizer_high_performance() {
        let opt = ConcurrencyOptimizer::default();
        let recs = opt.optimize("plan", 0.95, 1000.0);
        assert!(!recs.is_empty());
        assert!(recs.iter().any(|r| r.strategy == "increase_concurrency"));
    }

    #[test]
    fn test_concurrency_optimizer_high_latency() {
        let opt = ConcurrencyOptimizer::default();
        let recs = opt.optimize("plan", 0.95, 20_000.0);
        assert!(recs.iter().any(|r| r.strategy == "parallelize_steps"));
    }

    #[test]
    fn test_cost_optimizer_recommends_downgrade() {
        let opt = CostOptimizer::default();
        let recs = opt.optimize("plan", 0.95, 1000.0);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].strategy, "downgrade_model_tier");
    }

    #[test]
    fn test_cost_optimizer_no_recommendation_on_low_success() {
        let opt = CostOptimizer::default();
        let recs = opt.optimize("plan", 0.5, 1000.0);
        assert!(recs.is_empty());
    }

    #[test]
    fn test_registry_aggregates_all_optimizers() {
        let reg = OptimizerRegistry::new();
        let recs = reg.optimize_all("plan", 0.95, 1000.0);
        // Both concurrency + cost optimizers should fire
        assert_eq!(recs.len(), 2);
        // Recommendations should be sorted by expected_improvement descending
        assert!(recs[0].expected_improvement >= recs[1].expected_improvement);
    }

    #[test]
    fn test_registry_count() {
        let reg = OptimizerRegistry::new();
        assert_eq!(reg.count(), 2);
    }
}
