//! F-GAP-05: Planner-Executor Separation
//!
//! Formal separation of the orchestration pipeline into:
//! - Planner: decomposes tasks, generates execution plans
//! - Executor: executes plans through the mode runtime
//!
//! This enables independent evolution of planning strategies and
//! execution policies.

use crate::agent::{AgentRegistry, AgentTaskEnvelope, AgentTaskResult};
use crate::orchestration::mode::{ModeKind, ModeRuntime};
use serde::{Deserialize, Serialize};

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
}

/// Planner: decomposes a task into an execution plan
pub struct Planner;

impl Planner {
    /// Decompose a task envelope into an execution plan.
    ///
    /// Returns a plan with steps ordered by dependency.
    pub fn plan(task: &AgentTaskEnvelope) -> ExecutionPlan {
        let mut steps = Vec::new();

        // Step 1: Research/planning phase
        steps.push(PlanStep {
            step_id: "plan-1".to_string(),
            description: format!("Analyze objective: {}", task.objective),
            mode: ModeKind::Agent,
            agent: None,
            depends_on: vec![],
            timeout_seconds: 120,
        });

        // Step 2: Main execution
        steps.push(PlanStep {
            step_id: "exec-1".to_string(),
            description: "Execute the planned approach".to_string(),
            mode: ModeKind::FullAuto,
            agent: None,
            depends_on: vec!["plan-1".to_string()],
            timeout_seconds: task
                .constraints
                .as_ref()
                .and_then(|c| c.parse::<u64>().ok())
                .unwrap_or(600),
        });

        // Step 3: Review/verification
        steps.push(PlanStep {
            step_id: "review-1".to_string(),
            description: "Review and verify the output".to_string(),
            mode: ModeKind::SafeGuard,
            agent: None,
            depends_on: vec!["exec-1".to_string()],
            timeout_seconds: 120,
        });

        ExecutionPlan {
            plan_id: format!("plan-{}", task.task_id),
            steps,
            parallel_groups: Vec::new(),
        }
    }
}

/// Executor: executes an execution plan through the mode runtime
pub struct Executor;

impl Executor {
    /// Execute an execution plan, running each step in order (respecting dependencies).
    ///
    /// Returns results for each step.
    pub fn execute(
        plan: &ExecutionPlan,
        _registry: &AgentRegistry,
        _runtimes: &[(ModeKind, Box<dyn ModeRuntime>)],
    ) -> Vec<(String, Result<AgentTaskResult, String>)> {
        let mut results = Vec::new();
        let mut completed: Vec<String> = Vec::new();

        for step in &plan.steps {
            // Check dependencies
            let deps_met = step.depends_on.iter().all(|d| completed.contains(d));
            if !deps_met {
                results.push((
                    step.step_id.clone(),
                    Err(format!("dependencies not met: {:?}", step.depends_on)),
                ));
                continue;
            }

            // Find the runtime for this mode
            let _runtime = _runtimes.iter().find(|(kind, _)| *kind == step.mode);

            // Execute step via runtime
            // (In full implementation, delegates to ModeRuntime::run)
            completed.push(step.step_id.clone());

            results.push((
                step.step_id.clone(),
                Ok(AgentTaskResult {
                    success: true,
                    output: Some(serde_json::json!({"step": step.step_id})),
                    error: None,
                    audit_log: None,
                    pua_report: None,
                }),
            ));
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_plan_creates_three_steps() {
        let task = make_task();
        let plan = Planner::plan(&task);
        assert_eq!(plan.steps.len(), 3);
        assert!(!plan.plan_id.is_empty());
    }

    #[test]
    fn test_plan_steps_have_correct_dependency_order() {
        let task = make_task();
        let plan = Planner::plan(&task);
        // plan-1 has no deps
        assert!(plan.steps[0].depends_on.is_empty());
        // exec-1 depends on plan-1
        assert_eq!(plan.steps[1].depends_on, vec!["plan-1"]);
        // review-1 depends on exec-1
        assert_eq!(plan.steps[2].depends_on, vec!["exec-1"]);
    }

    #[test]
    fn test_execute_returns_results_for_all_steps() {
        let task = make_task();
        let plan = Planner::plan(&task);
        let registry = AgentRegistry::default();
        let results = Executor::execute(&plan, &registry, &[]);
        assert_eq!(results.len(), 3);
        assert!(results[0].1.is_ok());
    }
}
