//! ARCH-11: Workflow Optimizer Plugin
//!
//! Pluggable optimization strategies for workflow execution.
//! Registered with CapabilityBus to influence execution policy
//! based on historical success rates, latency patterns, and
//! resource utilization.

use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, MutexGuard};

/// Lock a Mutex, recovering from poison with a log.
/// Uses shared `crate::lock_or_recover!` macro.
fn lock_guard<T>(mtx: &Mutex<T>) -> MutexGuard<'_, T> {
    crate::lock_or_recover!(mtx, "workflow_optimizer")
}

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
/// The registry aggregates all suggestions and can apply the
/// highest-impact ones.
pub trait WorkflowOptimizerPlugin: Send + Sync {
    fn name(&self) -> &str;
    fn optimize(&self, ctx: &OptimizationContext) -> OptimizationSuggestion;
}

// ---------------------------------------------------------------------------
// Built-in plugin: ConcurrencyOptimizer
// ---------------------------------------------------------------------------

/// A concurrency-based optimizer.
///
/// Analyzes current latency and token usage to recommend
/// concurrency or parallelism adjustments.
pub struct ConcurrencyOptimizer {
    pub min_success_rate: f64,
    pub max_latency_threshold_ms: u64,
}

impl Default for ConcurrencyOptimizer {
    fn default() -> Self {
        Self {
            min_success_rate: 0.85,
            max_latency_threshold_ms: 10_000,
        }
    }
}

impl WorkflowOptimizerPlugin for ConcurrencyOptimizer {
    fn name(&self) -> &str {
        "concurrency_optimizer"
    }

    fn optimize(&self, ctx: &OptimizationContext) -> OptimizationSuggestion {
        // Compute average success rate from history.
        let total = ctx.history.len() as f64;
        let successes = ctx.history.iter().filter(|r| r.success).count() as f64;
        let success_rate = if total > 0.0 { successes / total } else { 1.0 };

        if success_rate >= self.min_success_rate && ctx.latency_ms < self.max_latency_threshold_ms {
            OptimizationSuggestion {
                optimizer_name: "concurrency_optimizer".into(),
                suggestion_type: "increase_concurrency".into(),
                description: format!(
                    "High success rate ({:.1}%) and low latency ({}ms) — increase concurrency by 1",
                    success_rate * 100.0,
                    ctx.latency_ms,
                ),
                confidence: 0.75,
                estimated_improvement: 0.3,
            }
        } else if ctx.latency_ms > self.max_latency_threshold_ms {
            OptimizationSuggestion {
                optimizer_name: "concurrency_optimizer".into(),
                suggestion_type: "parallelize_steps".into(),
                description: format!(
                    "High latency ({}ms > {}ms threshold) — parallelize independent steps",
                    ctx.latency_ms, self.max_latency_threshold_ms,
                ),
                confidence: 0.8,
                estimated_improvement: 0.5,
            }
        } else {
            OptimizationSuggestion {
                optimizer_name: "concurrency_optimizer".into(),
                suggestion_type: "no_action".into(),
                description: "Current performance is within acceptable bounds.".into(),
                confidence: 0.0,
                estimated_improvement: 0.0,
            }
        }
    }
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
    fn name(&self) -> &str {
        "cost_optimizer"
    }

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

// ---------------------------------------------------------------------------
// Built-in plugin: HistoryBasedOptimizer
// ---------------------------------------------------------------------------

/// Analyzes past execution history and suggests:
/// - Phase reordering if a different ordering improves success rate.
/// - Agent preference changes for phases with poor success.
/// - Budget adjustments based on token cost trends.
pub struct HistoryBasedOptimizer;

impl HistoryBasedOptimizer {
    /// Returns the name of the agent most associated with successful runs
    /// for a given phase. Returns `None` if no history exists.
    fn best_agent_for_phase(phase: &str, history: &[ExecutionRecord]) -> Option<String> {
        let mut agent_stats: std::collections::HashMap<&str, (u32, u32)> =
            std::collections::HashMap::new();

        for rec in history {
            if rec.phase_name != phase {
                continue;
            }
            let entry = agent_stats.entry(&rec.agent_name).or_insert((0, 0));
            entry.0 += 1; // total runs
            if rec.success {
                entry.1 += 1; // successful runs
            }
        }

        // Find agent with the highest success rate (minimum 1 run).
        agent_stats
            .into_iter()
            .filter(|(_, (total, _))| *total >= 1)
            .max_by(|a, b| {
                let a_rate = a.1 .1 as f64 / a.1 .0 as f64;
                let b_rate = b.1 .1 as f64 / b.1 .0 as f64;
                a_rate
                    .partial_cmp(&b_rate)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(name, _)| name.to_string())
    }

    /// Computes average success rate for a given phase across history.
    fn phase_success_rate(phase: &str, history: &[ExecutionRecord]) -> f64 {
        let phase_records: Vec<_> = history.iter().filter(|r| r.phase_name == phase).collect();
        let total = phase_records.len();
        if total == 0 {
            return 1.0;
        }
        let successes = phase_records.iter().filter(|r| r.success).count();
        successes as f64 / total as f64
    }
}

impl WorkflowOptimizerPlugin for HistoryBasedOptimizer {
    fn name(&self) -> &str {
        "history_based_optimizer"
    }

    fn optimize(&self, ctx: &OptimizationContext) -> OptimizationSuggestion {
        // --- Phase reordering suggestion ---
        // If earlier phases have low success and later phases have high success,
        // suggest reordering phases to front-load more reliable phases.
        let phase_rates: Vec<(usize, &str, f64)> = ctx
            .phases
            .iter()
            .enumerate()
            .map(|(i, p)| (i, p.as_str(), Self::phase_success_rate(p, &ctx.history)))
            .collect();

        // Check for a pattern where an early phase underperforms relative to later ones.
        let phase_reorder_desc = phase_rates
            .windows(2)
            .filter(|pair| {
                let (_, _, prev_rate) = pair[0];
                let (_, _, curr_rate) = pair[1];
                prev_rate < 0.7 && curr_rate > prev_rate + 0.15
            })
            .map(|pair| {
                format!(
                    "Phase \"{}\" (success rate {:.0}%) is followed by \"{}\" ({:.0}%) — consider swapping or reordering.",
                    pair[0].1, pair[0].2 * 100.0,
                    pair[1].1, pair[1].2 * 100.0,
                )
            })
            .collect::<Vec<_>>();

        // --- Agent preference suggestion ---
        // For phases with below-average success, find a better agent.
        let avg_success_rate: f64 = if !ctx.phases.is_empty() {
            phase_rates.iter().map(|(_, _, r)| r).sum::<f64>() / ctx.phases.len() as f64
        } else {
            1.0
        };

        let agent_change_desc: Vec<String> = ctx
            .phases
            .iter()
            .filter_map(|phase| {
                let rate = Self::phase_success_rate(phase, &ctx.history);
                if rate < avg_success_rate && rate < 0.75 {
                    if let Some(best) = Self::best_agent_for_phase(phase, &ctx.history) {
                        // Find current agent for the phase from history.
                        let current_agent = ctx
                            .history
                            .iter()
                            .rev()
                            .find(|r| r.phase_name == *phase)
                            .map(|r| &r.agent_name);
                        if current_agent != Some(&best) {
                            Some(format!(
                                "Phase \"{}\" has {:.0}% success rate — consider switching from \"{}\" to \"{}\".",
                                phase,
                                rate * 100.0,
                                current_agent.unwrap_or(&"unknown".to_string()),
                                best,
                            ))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

        // --- Budget adjustment suggestion ---
        let budget_desc = if !ctx.history.is_empty() {
            let avg_token_cost: f64 = ctx.history.iter().map(|r| r.token_cost).sum::<u64>() as f64
                / ctx.history.len() as f64;
            let recent: Vec<_> = ctx.history.iter().rev().take(5).collect();
            let recent_avg: f64 =
                recent.iter().map(|r| r.token_cost).sum::<u64>() as f64 / recent.len() as f64;

            if recent_avg > avg_token_cost * 1.2 {
                Some(format!(
                    "Recent token cost ({:.0}) is {:.1}% above historical average ({:.0}) — consider increasing budget.",
                    recent_avg,
                    (recent_avg / avg_token_cost - 1.0) * 100.0,
                    avg_token_cost,
                ))
            } else if recent_avg < avg_token_cost * 0.8 {
                Some(format!(
                    "Recent token cost ({:.0}) is {:.1}% below historical average ({:.0}) — consider reducing budget.",
                    recent_avg,
                    (1.0 - recent_avg / avg_token_cost) * 100.0,
                    avg_token_cost,
                ))
            } else {
                None
            }
        } else {
            None
        };

        // --- Pick the most impactful suggestion ---
        // Prefer agent changes > phase reordering > budget adjustments.
        if let Some(desc) = agent_change_desc.into_iter().next() {
            OptimizationSuggestion {
                optimizer_name: "history_based_optimizer".into(),
                suggestion_type: "agent_change".into(),
                description: desc,
                confidence: 0.65,
                estimated_improvement: 0.35,
            }
        } else if let Some(desc) = phase_reorder_desc.into_iter().next() {
            OptimizationSuggestion {
                optimizer_name: "history_based_optimizer".into(),
                suggestion_type: "phase_reorder".into(),
                description: desc,
                confidence: 0.5,
                estimated_improvement: 0.25,
            }
        } else if let Some(desc) = budget_desc {
            OptimizationSuggestion {
                optimizer_name: "history_based_optimizer".into(),
                suggestion_type: "budget_adjust".into(),
                description: desc,
                confidence: 0.6,
                estimated_improvement: 0.2,
            }
        } else {
            OptimizationSuggestion {
                optimizer_name: "history_based_optimizer".into(),
                suggestion_type: "no_action".into(),
                description:
                    "Execution patterns are stable — no history-based optimization needed.".into(),
                confidence: 0.0,
                estimated_improvement: 0.0,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Thread-safe registry
// ---------------------------------------------------------------------------

/// Thread-safe registry of workflow optimizer plugins.
///
/// Optimizers are registered once during initialization and then
/// invoked on each optimization pass.  The registry is `Send + Sync`
/// and can be shared via `Arc<OptimizerRegistry>`.
#[derive(Default, Clone)]
pub struct OptimizerRegistry {
    optimizers: Arc<Mutex<Vec<Box<dyn WorkflowOptimizerPlugin>>>>,
}

impl OptimizerRegistry {
    /// Creates a new registry pre-populated with built-in optimizers.
    pub fn new() -> Self {
        let reg = Self::default();
        reg.register(Box::new(ConcurrencyOptimizer::default()));
        reg.register(Box::new(CostOptimizer::default()));
        reg.register(Box::new(HistoryBasedOptimizer));
        reg
    }

    /// Registers a new optimizer plugin.
    pub fn register(&self, optimizer: Box<dyn WorkflowOptimizerPlugin>) {
        lock_guard(&self.optimizers).push(optimizer);
    }

    /// Runs all registered optimizers and returns aggregated suggestions,
    /// sorted by estimated improvement (highest first).
    pub fn optimize_all(&self, ctx: &OptimizationContext) -> Vec<OptimizationSuggestion> {
        let mut all: Vec<OptimizationSuggestion> = self
            .optimizers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|opt| opt.optimize(ctx))
            .filter(|s| s.estimated_improvement > 0.0)
            .collect();

        all.sort_by(|a, b| {
            b.estimated_improvement
                .partial_cmp(&a.estimated_improvement)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        all
    }

    /// Returns the names of all registered optimizers.
    pub fn list_optimizers(&self) -> Vec<String> {
        self.optimizers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|opt| opt.name().to_string())
            .collect()
    }

    /// Returns the number of registered optimizers.
    pub fn count(&self) -> usize {
        lock_guard(&self.optimizers).len()
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
    fn test_concurrency_optimizer_high_performance() {
        let opt = ConcurrencyOptimizer::default();
        let ctx = OptimizationContext {
            latency_ms: 1000,
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
        assert_eq!(suggestion.suggestion_type, "increase_concurrency");
    }

    #[test]
    fn test_concurrency_optimizer_high_latency() {
        let opt = ConcurrencyOptimizer::default();
        let ctx = OptimizationContext {
            latency_ms: 20_000,
            ..sample_context()
        };
        let suggestion = opt.optimize(&ctx);
        assert_eq!(suggestion.suggestion_type, "parallelize_steps");
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

    #[test]
    fn test_history_based_optimizer_agent_change() {
        let opt = HistoryBasedOptimizer;
        let ctx = sample_context();
        let suggestion = opt.optimize(&ctx);
        // "process" phase has poor success with agent_a and agent_b does better.
        assert_eq!(suggestion.suggestion_type, "agent_change");
        assert!(suggestion.description.contains("process"));
    }

    #[test]
    fn test_history_based_optimizer_budget_adjust() {
        let opt = HistoryBasedOptimizer;
        let mut ctx = sample_context();
        // Fix the process phase so phase_reorder doesn't trigger — give it 100% success.
        for rec in ctx.history.iter_mut() {
            if rec.phase_name == "process" {
                rec.success = true;
            }
        }
        // Add more history and set specific token costs to trigger budget adjustment.
        // Recent records (last 5) get much higher costs than early ones.
        let mut extra = vec![
            ExecutionRecord {
                phase_name: "fetch".into(),
                agent_name: "agent_a".into(),
                success: true,
                duration_ms: 200,
                token_cost: 10,
            },
            ExecutionRecord {
                phase_name: "process".into(),
                agent_name: "agent_a".into(),
                success: true,
                duration_ms: 200,
                token_cost: 10,
            },
            ExecutionRecord {
                phase_name: "output".into(),
                agent_name: "agent_b".into(),
                success: true,
                duration_ms: 200,
                token_cost: 10,
            },
            ExecutionRecord {
                phase_name: "fetch".into(),
                agent_name: "agent_a".into(),
                success: true,
                duration_ms: 200,
                token_cost: 10,
            },
        ];
        ctx.history = {
            let mut all = Vec::new();
            all.append(&mut extra);
            all.append(&mut ctx.history);
            all
        };
        // Now inflate the last 5 to be much larger than the early ones.
        for rec in ctx.history.iter_mut().rev().take(5) {
            rec.token_cost = (rec.token_cost as f64 * 50.0) as u64;
        }
        let suggestion = opt.optimize(&ctx);
        assert_eq!(suggestion.suggestion_type, "budget_adjust");
    }

    #[test]
    fn test_registry_aggregates_suggestions() {
        let reg = OptimizerRegistry::new();
        let ctx = sample_context();
        let suggestions = reg.optimize_all(&ctx);

        assert!(!suggestions.is_empty());
        // Suggestions should be sorted by estimated_improvement descending.
        for pair in suggestions.windows(2) {
            assert!(pair[0].estimated_improvement >= pair[1].estimated_improvement);
        }
    }

    #[test]
    fn test_registry_list_optimizers() {
        let reg = OptimizerRegistry::new();
        let names = reg.list_optimizers();
        assert!(names.contains(&"concurrency_optimizer".to_string()));
        assert!(names.contains(&"cost_optimizer".to_string()));
        assert!(names.contains(&"history_based_optimizer".to_string()));
    }

    #[test]
    fn test_registry_count() {
        let reg = OptimizerRegistry::new();
        assert_eq!(reg.count(), 3);
    }

    #[test]
    fn test_registry_custom_optimizer() {
        struct DummyOptimizer;
        impl WorkflowOptimizerPlugin for DummyOptimizer {
            fn name(&self) -> &str {
                "dummy"
            }

            fn optimize(&self, _ctx: &OptimizationContext) -> OptimizationSuggestion {
                OptimizationSuggestion {
                    optimizer_name: "dummy".into(),
                    suggestion_type: "custom".into(),
                    description: "A test suggestion.".into(),
                    confidence: 1.0,
                    estimated_improvement: 0.99,
                }
            }
        }

        let reg = OptimizerRegistry::new();
        reg.register(Box::new(DummyOptimizer));
        assert_eq!(reg.count(), 4);
        assert!(reg.list_optimizers().contains(&"dummy".to_string()));

        let suggestions = reg.optimize_all(&sample_context());
        assert!(suggestions.iter().any(|s| s.optimizer_name == "dummy"));
    }
}
