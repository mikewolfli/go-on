//! ARCH-11: Workflow Optimizer Plugin
//!
//! Pluggable optimization strategies for workflow execution.
//!
//! The `ConcurrencyOptimizer` / `HistoryBasedOptimizer` plugins and the
//! `OptimizerRegistry` aggregation layer were removed as unwired dead code:
//! production optimization runs through
//! `capability_bus::optimization_bus` (`OptimizationBus::recommend`), which
//! delegates directly to the [`CostOptimizer`] kept here.

use serde::{Deserialize, Serialize};

/// A single execution record for a phase in a workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRecord {
    /// Name of the phase that was executed.
    pub phase_name: String,
    /// Name of the agent that handled the phase.
    pub agent_name: String,
    /// Whether the phase completed successfully.
    pub success: bool,
    /// Duration of execution in milliseconds.
    pub duration_ms: u64,
    /// Token cost incurred by this execution.
    pub token_cost: u64,
}

/// Context provided to optimization plugins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationContext {
    /// The type / identifier of the workflow being optimized.
    pub workflow_type: String,
    /// Ordered list of phase names in this workflow.
    pub phases: Vec<String>,
    /// Execution history for recent runs of this workflow.
    pub history: Vec<ExecutionRecord>,
    /// Total token usage across recent runs.
    pub token_usage: u64,
    /// Observed end-to-end latency for the workflow in milliseconds.
    pub latency_ms: u64,
}

/// A suggestion produced by an optimization plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationSuggestion {
    /// Name of the optimizer that produced this suggestion.
    pub optimizer_name: String,
    /// Type/category of suggestion (e.g. "phase_reorder", "agent_change", "budget_adjust").
    pub suggestion_type: String,
    /// Human-readable explanation of the suggestion.
    pub description: String,
    /// Confidence level in the suggestion (0.0 – 1.0).
    pub confidence: f64,
    /// Estimated improvement factor (0.0 – 1.0+) if the suggestion is applied.
    pub estimated_improvement: f64,
}

/// Trait for workflow optimization plugins.
///
/// Each plugin inspects the optimization context and returns
/// an `OptimizationSuggestion` if it identifies an opportunity.
pub trait WorkflowOptimizerPlugin: Send + Sync {
    fn optimize(&self, ctx: &OptimizationContext) -> OptimizationSuggestion;
}

// ---------------------------------------------------------------------------
// Built-in plugin: CostOptimizer
// ---------------------------------------------------------------------------

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

impl WorkflowOptimizerPlugin for CostOptimizer {
    fn optimize(&self, ctx: &OptimizationContext) -> OptimizationSuggestion {
        let total = ctx.history.len() as f64;
        let successes = ctx.history.iter().filter(|r| r.success).count() as f64;
        let success_rate = if total > 0.0 { successes / total } else { 1.0 };

        if success_rate >= self.min_success_rate {
            OptimizationSuggestion {
                optimizer_name: "cost_optimizer".into(),
                suggestion_type: "downgrade_model_tier".into(),
                description: format!(
                    "High success rate ({:.1}%) — consider downgrading model tier to reduce cost by {:.0}%",
                    success_rate * 100.0,
                    self.cost_factor * 100.0,
                ),
                confidence: 0.7,
                estimated_improvement: self.cost_factor,
            }
        } else {
            OptimizationSuggestion {
                optimizer_name: "cost_optimizer".into(),
                suggestion_type: "no_action".into(),
                description: format!(
                    "Success rate ({:.1}%) is below the {:.0}% threshold — no cost optimization recommended.",
                    success_rate * 100.0,
                    self.min_success_rate * 100.0,
                ),
                confidence: 0.0,
                estimated_improvement: 0.0,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_context() -> OptimizationContext {
        OptimizationContext {
            workflow_type: "test_workflow".into(),
            phases: vec!["fetch".into(), "process".into(), "output".into()],
            history: vec![
                ExecutionRecord {
                    phase_name: "fetch".into(),
                    agent_name: "agent_a".into(),
                    success: true,
                    duration_ms: 200,
                    token_cost: 100,
                },
                ExecutionRecord {
                    phase_name: "process".into(),
                    agent_name: "agent_a".into(),
                    success: false,
                    duration_ms: 5000,
                    token_cost: 800,
                },
                ExecutionRecord {
                    phase_name: "output".into(),
                    agent_name: "agent_b".into(),
                    success: true,
                    duration_ms: 300,
                    token_cost: 150,
                },
                ExecutionRecord {
                    phase_name: "fetch".into(),
                    agent_name: "agent_a".into(),
                    success: true,
                    duration_ms: 180,
                    token_cost: 110,
                },
                ExecutionRecord {
                    phase_name: "process".into(),
                    agent_name: "agent_b".into(),
                    success: true,
                    duration_ms: 4200,
                    token_cost: 700,
                },
                ExecutionRecord {
                    phase_name: "process".into(),
                    agent_name: "agent_a".into(),
                    success: false,
                    duration_ms: 4500,
                    token_cost: 750,
                },
                ExecutionRecord {
                    phase_name: "output".into(),
                    agent_name: "agent_b".into(),
                    success: true,
                    duration_ms: 280,
                    token_cost: 140,
                },
            ],
            token_usage: 2050,
            latency_ms: 5480,
        }
    }

    #[test]
    fn test_cost_optimizer_recommends_downgrade() {
        let opt = CostOptimizer::default();
        let ctx = OptimizationContext {
            history: vec![
                ExecutionRecord {
                    phase_name: "p1".into(),
                    agent_name: "a".into(),
                    success: true,
                    duration_ms: 100,
                    token_cost: 50,
                };
                10
            ],
            ..sample_context()
        };
        let suggestion = opt.optimize(&ctx);
        assert_eq!(suggestion.suggestion_type, "downgrade_model_tier");
    }

    #[test]
    fn test_cost_optimizer_no_recommendation() {
        let opt = CostOptimizer::default();
        let ctx = OptimizationContext {
            history: vec![
                ExecutionRecord {
                    phase_name: "p1".into(),
                    agent_name: "a".into(),
                    success: false,
                    duration_ms: 100,
                    token_cost: 50,
                };
                10
            ],
            ..sample_context()
        };
        let suggestion = opt.optimize(&ctx);
        assert_eq!(suggestion.suggestion_type, "no_action");
    }
}
