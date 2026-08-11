//! F-GAP-05: Planner-Executor Separation
//!
//! Formal separation of the orchestration pipeline into:
//! - Planner: decomposes tasks, generates execution plans
//! - Executor: executes plans through the mode runtime
//!
//! This enables independent evolution of planning strategies and
//! execution policies.
//!
//! Note: The `ExecutionPlan` and `PlanStep` types are canonical and used
//! by `brain_loop::plan_construction::Planner`.  The `Executor` was replaced
//! by `plan_construction` (the `BrainLoop` struct itself was removed in the
//! round-23 cleanup; the live planning surface lives in
//! `brain_loop::plan_construction`).

use crate::orchestration::brain_loop::plan_construction::DagMetrics;
use crate::orchestration::mode::ModeKind;
use serde::{Deserialize, Serialize};

/// A single step in an execution plan
///
/// This is the canonical step type used by `Planner::plan()`.
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
///
/// This is the canonical plan type returned by `Planner::plan()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
    pub plan_id: String,
    pub steps: Vec<PlanStep>,
    pub parallel_groups: Vec<Vec<String>>,
    pub dag_metrics: Option<DagMetrics>,
}

#[cfg(test)]
mod tests {
    use crate::agent::AgentTaskEnvelope;
    use crate::orchestration::brain_loop::plan_construction::{
        Planner, PlanningContext, TaskComplexity,
    };

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
        let simple_plan = Planner::plan(&simple_task);
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

        // Medium task -> 3 steps (unified classifier: baseline + "performance")
        let medium_task = AgentTaskEnvelope {
            task_id: "medium-1".to_string(),
            phase: "coding".to_string(),
            role: "coder".to_string(),
            objective:
                "Fix the performance bug in the authentication module and verify everything works correctly"
                    .to_string(),
            constraints: None,
            evidence: None,
            input: serde_json::json!({}),
        };
        let medium_plan = Planner::plan(&medium_task);
        assert_eq!(
            medium_plan.steps.len(),
            3,
            "Medium task should produce 3 steps"
        );

        // Complex task -> full DAG (unified classifier: baseline + "redesign")
        let complex_task = AgentTaskEnvelope {
            task_id: "complex-1".to_string(),
            phase: "coding".to_string(),
            role: "coder".to_string(),
            objective: "Research the authentication module, redesign the middleware chain to use JWT, and build comprehensive unit tests".to_string(),
            constraints: None,
            evidence: None,
            input: serde_json::json!({}),
        };
        let complex_plan = Planner::plan(&complex_task);
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
        let plan = Planner::plan(&task);
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
        let plan = Planner::plan(&task);
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
        let plan = Planner::plan(&task);
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
        let plan = Planner::plan(&task);
        assert_eq!(plan.steps.len(), 2, "Simple task should produce 2 steps");
        // exec-1 has no deps (no plan phase for simple)
        assert!(plan.steps[0].depends_on.is_empty());
        // review-1 depends on exec-1
        assert_eq!(plan.steps[1].depends_on, vec!["exec-1"]);
    }
}
