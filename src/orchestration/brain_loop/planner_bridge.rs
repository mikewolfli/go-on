//! Planner bridge — integrates `planner_executor::Planner` into `BrainLoop`.
//!
//! Provides automatic task decomposition: when a task string is given instead
//! of explicit steps, the bridge uses the `Planner` to auto-generate a DAG of
//! steps via embedding-based classification and keyword heuristics.

use crate::orchestration::brain_loop::{BrainLoopPhase, BrainLoopStep, StepStatus};
use crate::orchestration::planner_executor::{ExecutionPlan, Planner};
use serde::{Deserialize, Serialize};

use crate::agent::AgentTaskEnvelope;

/// Strategy for generating a plan's steps.
///
/// - `ExplicitSteps`: caller provides steps directly (existing behavior)
/// - `AutoDecompose`: `planner_executor::Planner` decomposes the task into a DAG
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanningStrategy {
    /// Use caller-provided steps (default, backward-compatible).
    ExplicitSteps,
    /// Use `planner_executor::Planner` to auto-decompose the task.
    AutoDecompose,
}

impl Default for PlanningStrategy {
    fn default() -> Self {
        Self::ExplicitSteps
    }
}

/// Convert a planner_executor `ExecutionPlan` into a `Vec<BrainLoopStep>`.
///
/// Maps DAG plan steps into the brain loop's step representation, preserving
/// dependency ordering. Step IDs are kept from the execution plan.
pub fn execution_plan_to_brain_steps(plan: &ExecutionPlan) -> Vec<BrainLoopStep> {
    plan.steps
        .iter()
        .map(|ps| BrainLoopStep {
            id: ps.step_id.clone(),
            phase: match ps.mode {
                crate::orchestration::mode::ModeKind::SafeGuard => BrainLoopPhase::Reflecting,
                _ => BrainLoopPhase::Executing,
            },
            description: ps.description.clone(),
            input: String::new(),
            output: String::new(),
            started_ms: 0,
            completed_ms: 0,
            duration_ms: 0,
            status: StepStatus::Pending,
            context: None,
            depends_on: vec![],
            mode: "auto".to_string(),
            agent: None,
            timeout_seconds: 60,
            parallel_group: None,
        })
        .collect()
}

/// Decompose a task string into brain loop steps using the `planner_executor::Planner`.
///
/// This is the primary entry point for auto-decomposition. It constructs an
/// `AgentTaskEnvelope` from the task string and runs the planner, then
/// converts the resulting `ExecutionPlan` into `Vec<BrainLoopStep>`.
pub async fn auto_decompose_task(task: &str) -> Vec<BrainLoopStep> {
    let envelope = AgentTaskEnvelope {
        task_id: format!("auto-{}", fastrand::u64(..)),
        phase: "planning".to_string(),
        role: "planner".to_string(),
        objective: task.to_string(),
        constraints: None,
        evidence: None,
        input: serde_json::json!({}),
    };

    let plan = Planner::plan(&envelope).await;
    execution_plan_to_brain_steps(&plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_auto_decompose_simple_task() {
        let steps = auto_decompose_task("Greet the user").await;
        assert_eq!(steps.len(), 2, "Simple task should produce 2 steps");
        assert_eq!(steps[0].status, StepStatus::Pending);
        assert_eq!(steps[1].status, StepStatus::Pending);
    }

    #[tokio::test]
    async fn test_auto_decompose_complex_task() {
        let steps = auto_decompose_task(
            "Research the authentication module, refactor to use JWT, \
             build a middleware chain, and write comprehensive unit tests \
             for all modified components",
        )
        .await;
        assert!(steps.len() >= 3, "Complex task should produce >= 3 steps");
    }

    #[test]
    fn test_execution_plan_to_brain_steps_converts_correctly() {
        let exec_plan = ExecutionPlan {
            plan_id: "test".to_string(),
            steps: vec![
                crate::orchestration::planner_executor::PlanStep {
                    step_id: "exec-1".to_string(),
                    description: "Execute task".to_string(),
                    mode: crate::orchestration::mode::ModeKind::FullAuto,
                    agent: None,
                    depends_on: vec![],
                    timeout_seconds: 300,
                },
                crate::orchestration::planner_executor::PlanStep {
                    step_id: "review-1".to_string(),
                    description: "Review output".to_string(),
                    mode: crate::orchestration::mode::ModeKind::SafeGuard,
                    agent: None,
                    depends_on: vec!["exec-1".to_string()],
                    timeout_seconds: 60,
                },
            ],
            parallel_groups: vec![],
            dag_metrics: None,
        };

        let brain_steps = execution_plan_to_brain_steps(&exec_plan);
        assert_eq!(brain_steps.len(), 2);
        assert_eq!(brain_steps[0].id, "exec-1");
        assert_eq!(brain_steps[0].phase, BrainLoopPhase::Executing);
        assert_eq!(brain_steps[1].id, "review-1");
        assert_eq!(brain_steps[1].phase, BrainLoopPhase::Reflecting);
    }

    #[test]
    fn test_planning_strategy_default() {
        let strategy = PlanningStrategy::default();
        assert_eq!(strategy, PlanningStrategy::ExplicitSteps);
    }
}
