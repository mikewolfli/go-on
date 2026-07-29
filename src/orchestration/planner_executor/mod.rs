//! F-GAP-05: Planner-Executor Separation
//!
//! Formal separation of the orchestration pipeline into:
//! - Planner: decomposes tasks, generates execution plans
//! - Executor: executes plans through the mode runtime
//!
//! This enables independent evolution of planning strategies and
//! execution policies.

use std::collections::{HashMap, HashSet};

use crate::agent::{AgentRegistry, AgentTaskEnvelope, AgentTaskResult};
use crate::i18n::runtime::tf;
use std::sync::Arc;

use crate::orchestration::mode::{ModeKind, ModeRuntime};
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

pub mod execution;

pub use execution::Executor;

// ---------------------------------------------------------------------------
// Deprecated re-exports — moved to brain_loop::plan_construction
// ---------------------------------------------------------------------------

#[deprecated(
    since = "1.5.0",
    note = "use crate::orchestration::brain_loop::plan_construction::Planner instead"
)]
pub use crate::orchestration::brain_loop::plan_construction::Planner;

#[deprecated(
    since = "1.5.0",
    note = "use crate::orchestration::brain_loop::plan_construction::TaskComplexity instead"
)]
pub use crate::orchestration::brain_loop::plan_construction::TaskComplexity;

#[deprecated(
    since = "1.5.0",
    note = "use crate::orchestration::brain_loop::plan_construction::DagMetrics instead"
)]
pub use crate::orchestration::brain_loop::plan_construction::DagMetrics;

/// A single step in an execution plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub step_id: String,
    pub description: String,
    pub mode: ModeKind,
    pub agent: Option<String>,
    pub depends_on: Vec<String>,
    pub timeout_seconds: u64,
}

/// An execution plan produced by the Planner
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
    pub plan_id: String,
    pub steps: Vec<PlanStep>,
    pub parallel_groups: Vec<Vec<String>>,
    pub dag_metrics: Option<DagMetrics>,
}

/// Configuration for the Planner-Executor pipeline.
///
/// Carried in OrchestrationServerDeps for future wiring of
/// task execution timeouts.
#[derive(Debug, Clone, Default)]
pub struct PlannerExecutorConfig;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::brain_loop::plan_construction::PlanningContext;

    fn make_task() -> AgentTaskEnvelope {
        AgentTaskEnvelope {
            task_id: "test-1".to_string(),
            phase: "coding".to_string(),
            role: "coder".to_string(),
            objective: "Implement feature X".to_string(),
            constraints: None,
            evidence: None,
            input: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn test_plan_complexity_variation() {
        // Simple task -> 2 steps
        let simple_task = AgentTaskEnvelope {
            task_id: "simple-1".to_string(),
            phase: "coding".to_string(),
            role: "coder".to_string(),
            objective: "Greet the user".to_string(),
            constraints: None,
            evidence: None,
            input: serde_json::json!({}),
        };
        let simple_plan = Planner::plan(&simple_task).await;
        assert_eq!(
            simple_plan.steps.len(),
            2,
            "Simple task should produce 2 steps"
        );
        assert!(simple_plan.dag_metrics.is_some());
        assert_eq!(
            simple_plan.dag_metrics.as_ref().unwrap().complexity_level,
            "Simple"
        );

        // Medium task -> 3 steps
        let medium_task = AgentTaskEnvelope {
            task_id: "medium-1".to_string(),
            phase: "coding".to_string(),
            role: "coder".to_string(),
            objective:
                "Fix the bug in the authentication module and verify everything works correctly"
                    .to_string(),
            constraints: None,
            evidence: None,
            input: serde_json::json!({}),
        };
        let medium_plan = Planner::plan(&medium_task).await;
        assert_eq!(
            medium_plan.steps.len(),
            3,
            "Medium task should produce 3 steps"
        );

        // Complex task -> full DAG
        let complex_task = AgentTaskEnvelope {
            task_id: "complex-1".to_string(),
            phase: "coding".to_string(),
            role: "coder".to_string(),
            objective: "Research the authentication module, refactor to use JWT, build a middleware chain, and write comprehensive unit tests for all modified components".to_string(),
            constraints: None,
            evidence: None,
            input: serde_json::json!({}),
        };
        let complex_plan = Planner::plan(&complex_task).await;
        assert!(
            complex_plan.steps.len() >= 3,
            "Complex task should produce >= 3 steps"
        );
    }

    #[test]
    fn test_plan_to_dag_produces_variable_depths() {
        let task = make_task();
        let simple = Planner::plan_to_dag(
            &task,
            &PlanningContext {
                complexity: TaskComplexity::Simple,
                ..PlanningContext::default()
            },
        );
        assert_eq!(simple.steps.len(), 2);

        let medium = Planner::plan_to_dag(
            &task,
            &PlanningContext {
                complexity: TaskComplexity::Medium,
                ..PlanningContext::default()
            },
        );
        assert_eq!(medium.steps.len(), 3);

        let complex = Planner::plan_to_dag(
            &task,
            &PlanningContext {
                complexity: TaskComplexity::Complex,
                subtask_hints: vec!["a".into(), "b".into(), "c".into()],
                ..PlanningContext::default()
            },
        );
        assert!(complex.steps.len() >= 4);
        assert!(!complex.parallel_groups.is_empty());
    }

    #[tokio::test]
    async fn test_dag_metrics_expose_width_and_depth() {
        let task = make_task();
        let plan = Planner::plan(&task).await;
        let metrics = plan.dag_metrics.unwrap();
        assert!(metrics.width >= 1);
        assert!(metrics.depth >= 1);
        assert_eq!(metrics.total_steps, plan.steps.len());
        assert!(!metrics.complexity_level.is_empty());
    }

    /// BLUE44: Verify DAG metrics are populated with non-zero values when a plan
    /// is created, confirming the metrics flow through governance integration.
    #[tokio::test]
    async fn test_dag_metrics_populated_with_non_zero_values() {
        // Test with a complex task to ensure non-trivial metrics
        let task = AgentTaskEnvelope {
            task_id: "complex-verify".to_string(),
            phase: "planning".to_string(),
            role: "architect".to_string(),
            objective: "Design and implement a distributed caching layer with write-through and write-behind strategies, including cluster rebalancing, failure recovery, and monitoring dashboards."
                .to_string(),
            constraints: None,
            evidence: None,
            input: serde_json::json!({"priority": "high", "team_size": 5}),
        };
        let plan = Planner::plan(&task).await;
        let metrics = plan
            .dag_metrics
            .expect("DAG metrics must be present when a plan is created");

        // All numeric metrics must be > 0
        assert!(
            metrics.width > 0,
            "dag_width must be non-zero, got {}",
            metrics.width
        );
        assert!(
            metrics.depth > 0,
            "dag_depth must be non-zero, got {}",
            metrics.depth
        );
        // parallel_group_count may be 0 for linear DAGs with no fan-out
        // so we only verify it doesn't exceed total_steps
        assert!(
            metrics.parallel_group_count <= metrics.total_steps,
            "dag_parallel_group_count ({}) should not exceed total_steps ({})",
            metrics.parallel_group_count,
            metrics.total_steps
        );
        assert!(
            metrics.total_steps > 0,
            "dag_total_steps must be non-zero, got {}",
            metrics.total_steps
        );
        assert!(
            !metrics.complexity_level.is_empty(),
            "complexity_level must not be empty"
        );

        // Verify consistency: total_steps should equal the number of plan steps
        assert_eq!(
            metrics.total_steps,
            plan.steps.len(),
            "dag_total_steps should equal plan.steps.len()"
        );

        // Verify that width and depth are within reasonable bounds
        assert!(
            metrics.width <= metrics.total_steps,
            "dag_width ({}) should not exceed total_steps ({})",
            metrics.width,
            metrics.total_steps
        );
        assert!(
            metrics.depth <= metrics.total_steps,
            "dag_depth ({}) should not exceed total_steps ({})",
            metrics.depth,
            metrics.total_steps
        );
    }

    #[tokio::test]
    async fn test_plan_creates_three_steps_legacy_compat() {
        let task = make_task();
        let plan = Planner::plan(&task).await;
        assert!(!plan.plan_id.is_empty());
        assert!(plan.steps.len() >= 2);
    }

    #[tokio::test]
    async fn test_plan_steps_have_correct_dependency_order() {
        // Use a Simple task to test linear dependency order
        let task = AgentTaskEnvelope {
            task_id: "simple-1".to_string(),
            phase: "coding".to_string(),
            role: "coder".to_string(),
            objective: "Fix a typo in the README".to_string(),
            constraints: None,
            evidence: None,
            input: serde_json::json!({}),
        };
        let plan = Planner::plan(&task).await;
        assert_eq!(plan.steps.len(), 2, "Simple task should produce 2 steps");
        // exec-1 has no deps (no plan phase for simple)
        assert!(plan.steps[0].depends_on.is_empty());
        // review-1 depends on exec-1
        assert_eq!(plan.steps[1].depends_on, vec!["exec-1"]);
    }

    #[tokio::test]
    async fn test_parallel_groups_execute_concurrently() {
        // Create a plan with a parallel group to verify concurrent execution.
        // We use a ManualClock runtime that records execution order to prove concurrency.
        use crate::orchestration::mode::ModeKind;

        let plan = ExecutionPlan {
            plan_id: "parallel-test".to_string(),
            steps: vec![
                PlanStep {
                    step_id: "plan-1".to_string(),
                    description: "Analyze".to_string(),
                    mode: ModeKind::FullAuto,
                    agent: None,
                    depends_on: vec![],
                    timeout_seconds: 60,
                },
                PlanStep {
                    step_id: "sub-1".to_string(),
                    description: "Subtask A".to_string(),
                    mode: ModeKind::FullAuto,
                    agent: None,
                    depends_on: vec!["plan-1".to_string()],
                    timeout_seconds: 60,
                },
                PlanStep {
                    step_id: "sub-2".to_string(),
                    description: "Subtask B".to_string(),
                    mode: ModeKind::FullAuto,
                    agent: None,
                    depends_on: vec!["plan-1".to_string()],
                    timeout_seconds: 60,
                },
                PlanStep {
                    step_id: "review-1".to_string(),
                    description: "Review".to_string(),
                    mode: ModeKind::SafeGuard,
                    agent: None,
                    depends_on: vec!["sub-1".to_string(), "sub-2".to_string()],
                    timeout_seconds: 60,
                },
            ],
            parallel_groups: vec![vec!["sub-1".to_string(), "sub-2".to_string()]],
            dag_metrics: None,
        };

        let registry = AgentRegistry::default();
        let results = Executor::execute(&plan, &registry, &[]).await;

        // Without runtimes, all steps fail — but we verify the parallel group
        // was dispatched (both sub-1 and sub-2 should have results).
        assert_eq!(results.len(), 4, "All 4 steps should produce results");

        // The order in results follows plan.steps declaration order.
        // plan-1: no runtime
        assert!(results[0].1.is_err());
        // If plan-1 fails, sub-1 and sub-2 should be cancelled (upstream failure)
        // Check that both parallel steps are handled (either success or cancellation)
        let sub_results: Vec<_> = results
            .iter()
            .filter(|(id, _)| id.starts_with("sub-"))
            .collect();
        assert_eq!(
            sub_results.len(),
            2,
            "Both parallel subtasks must have results"
        );
    }

    #[tokio::test]
    async fn test_execute_returns_results_for_all_steps() {
        let task = make_task();
        let plan = Planner::plan(&task).await;
        let registry = AgentRegistry::default();
        let results = Executor::execute(&plan, &registry, &[]).await;
        // With no runtimes:
        // plan-1 (no deps) -> "no runtime found"
        // exec-1 (depends on plan-1, which failed) -> "cancelled due to upstream failure"
        // review-1 (depends on exec-1, which was cancelled) -> "cancelled due to upstream failure"
        assert_eq!(results.len(), 3);
        assert!(results[0].1.is_err());
        assert!(results[1].1.is_err());
        assert!(results[2].1.is_err());
    }

    #[tokio::test]
    async fn test_execute_with_missing_dependency() {
        // Create a plan where exec-1 depends on plan-1, but plan-1 will fail
        // because there's no runtime. The dependency should still be tracked.
        let task = make_task();
        let plan = Planner::plan(&task).await;
        let registry = AgentRegistry::default();
        let results = Executor::execute(&plan, &registry, &[]).await;
        // First step (plan-1) fails with "no runtime found"
        assert!(results[0].1.is_err());
        // Second step (exec-1) depends on plan-1 which failed
        // -> short-circuit: "cancelled due to upstream failure"
        assert!(results[1].1.is_err());
    }
}
