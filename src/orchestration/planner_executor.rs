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

/// DAG metrics for governance observability
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagMetrics {
    pub width: usize,
    pub depth: usize,
    pub parallel_group_count: usize,
    pub total_steps: usize,
    pub complexity_level: String,
}

/// An execution plan produced by the Planner
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
    pub plan_id: String,
    pub steps: Vec<PlanStep>,
    pub parallel_groups: Vec<Vec<String>>,
    pub dag_metrics: Option<DagMetrics>,
}

/// Planner: decomposes a task into an execution plan
pub struct Planner;

/// Task complexity level for adaptive planning
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskComplexity {
    Simple,
    Medium,
    Complex,
}

impl Default for TaskComplexity {
    fn default() -> Self {
        TaskComplexity::Simple
    }
}

/// Planning context carrying task features for adaptive decomposition
#[derive(Debug, Clone, Default)]
pub struct PlanningContext {
    pub complexity: TaskComplexity,
    pub has_code: bool,
    pub has_research: bool,
    pub has_multiple_subtasks: bool,
    pub subtask_hints: Vec<String>,
}

impl Planner {
    /// Decompose a task envelope into an execution plan (legacy, fixed 3-step).
    ///
    /// Delegates to `plan_to_dag` with a default context for backward compatibility.
    pub fn plan(task: &AgentTaskEnvelope) -> ExecutionPlan {
        let context = Planner::analyze_task(task);
        Planner::plan_to_dag(task, &context)
    }

    /// Analyze task characteristics to determine adaptive planning context.
    fn analyze_task(task: &AgentTaskEnvelope) -> PlanningContext {
        let objective_lower = task.objective.to_ascii_lowercase();
        let objective_len = task.objective.len();

        // Detect complexity indicators from objective and input payload
        let has_code = objective_lower.contains("code")
            || objective_lower.contains("file")
            || objective_lower.contains("implement")
            || objective_lower.contains("function")
            || objective_lower.contains("refactor")
            || objective_lower.contains("class")
            || objective_lower.contains("module")
            || objective_lower.contains("build")
            || objective_lower.contains("test");

        let has_research = objective_lower.contains("research")
            || objective_lower.contains("search")
            || objective_lower.contains("find")
            || objective_lower.contains("analyze")
            || objective_lower.contains("explain")
            || objective_lower.contains("compare");

        let has_multiple = objective_lower.contains(" and ")
            || objective_lower.contains(",")
            || objective_lower.contains("first")
            || objective_lower.contains("then")
            || objective_lower.contains("also")
            || objective_lower.contains("both")
            || objective_lower.contains("multiple");

        // Extract subtask hints from task input or objective
        let mut subtask_hints: Vec<String> = Vec::new();
        if let Some(input_obj) = task.input.as_object() {
            if let Some(hints) = input_obj.get("subtasks").and_then(|v| v.as_array()) {
                for hint in hints {
                    if let Some(s) = hint.as_str() {
                        subtask_hints.push(s.to_string());
                    }
                }
            }
        }
        if subtask_hints.is_empty() {
            // Try to extract bullet-like subtasks from objective
            for line in task.objective.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with('-')
                    || trimmed.starts_with('*')
                    || trimmed.starts_with(|c: char| c.is_ascii_digit())
                {
                    let hint = trimmed
                        .trim_start_matches(|c: char| {
                            c == '-' || c == '*' || c.is_ascii_digit() || c == '.' || c == ')'
                        })
                        .trim()
                        .to_string();
                    if !hint.is_empty() && hint.len() > 5 {
                        subtask_hints.push(hint);
                    }
                }
            }
        }

        // Determine complexity level
        // Tasks with explicit multi-step decomposition signals are Complex
        let has_strong_code = has_code
            && (objective_lower.contains("refactor")
                || objective_lower.contains("build")
                || objective_lower.contains("write tests"))
            && objective_len > 60;
        let complexity = if (objective_len > 300 && (has_code || has_research))
            || (has_strong_code && has_multiple && has_research)
            || subtask_hints.len() >= 4
        {
            TaskComplexity::Complex
        } else if objective_len > 60
            || has_code
            || has_research
            || has_multiple
            || !subtask_hints.is_empty()
        {
            TaskComplexity::Medium
        } else {
            TaskComplexity::Simple
        };

        PlanningContext {
            complexity,
            has_code,
            has_research,
            has_multiple_subtasks: has_multiple,
            subtask_hints,
        }
    }

    /// Adaptive DAG planner: produces a task-dependent execution plan with
    /// proper dependency edges and parallel groups based on task characteristics.
    pub fn plan_to_dag(task: &AgentTaskEnvelope, context: &PlanningContext) -> ExecutionPlan {
        let plan_id = format!("plan-{}-dag", task.task_id);
        let mut steps: Vec<PlanStep> = Vec::new();
        let mut parallel_groups: Vec<Vec<String>> = Vec::new();

        match context.complexity {
            TaskComplexity::Simple => {
                // Simple task: 2-step linear plan (execute + review)
                let exec_id = "exec-1".to_string();
                steps.push(PlanStep {
                    step_id: exec_id.clone(),
                    description: format!("Execute: {}", task.objective),
                    mode: ModeKind::FullAuto,
                    agent: None,
                    depends_on: vec![],
                    timeout_seconds: task
                        .constraints
                        .as_ref()
                        .and_then(|c| c.parse::<u64>().ok())
                        .unwrap_or(300),
                });
                steps.push(PlanStep {
                    step_id: "review-1".to_string(),
                    description: "Review and verify output".to_string(),
                    mode: ModeKind::SafeGuard,
                    agent: None,
                    depends_on: vec![exec_id],
                    timeout_seconds: 60,
                });
            }
            TaskComplexity::Medium => {
                // Medium task: 3-step plan with optional parallel subtasks
                if context.subtask_hints.len() >= 2 {
                    // Multiple subtasks: fan-out parallel execution
                    let plan_step = PlanStep {
                        step_id: "plan-1".to_string(),
                        description: format!("Analyze objective: {}", task.objective),
                        mode: ModeKind::Agent,
                        agent: None,
                        depends_on: vec![],
                        timeout_seconds: 120,
                    };
                    steps.push(plan_step);

                    let mut parallel_ids: Vec<String> = Vec::new();
                    for (i, hint) in context.subtask_hints.iter().enumerate() {
                        let sub_id = format!("sub-{}", i + 1);
                        steps.push(PlanStep {
                            step_id: sub_id.clone(),
                            description: format!("Subtask {}: {}", i + 1, hint),
                            mode: ModeKind::FullAuto,
                            agent: None,
                            depends_on: vec!["plan-1".to_string()],
                            timeout_seconds: 300,
                        });
                        parallel_ids.push(sub_id);
                    }
                    if parallel_ids.len() >= 2 {
                        parallel_groups.push(parallel_ids.clone());
                    }

                    let join_id = "review-1".to_string();
                    steps.push(PlanStep {
                        step_id: join_id.clone(),
                        description: "Review and consolidate subtask outputs".to_string(),
                        mode: ModeKind::SafeGuard,
                        agent: None,
                        depends_on: parallel_ids,
                        timeout_seconds: 120,
                    });
                } else {
                    // Sequential research → execute → review
                    let plan_step = PlanStep {
                        step_id: "plan-1".to_string(),
                        description: format!("Analyze objective: {}", task.objective),
                        mode: ModeKind::Agent,
                        agent: None,
                        depends_on: vec![],
                        timeout_seconds: 120,
                    };
                    steps.push(plan_step);

                    let exec_id = "exec-1".to_string();
                    steps.push(PlanStep {
                        step_id: exec_id.clone(),
                        description: format!("Execute: {}", task.objective),
                        mode: ModeKind::FullAuto,
                        agent: None,
                        depends_on: vec!["plan-1".to_string()],
                        timeout_seconds: task
                            .constraints
                            .as_ref()
                            .and_then(|c| c.parse::<u64>().ok())
                            .unwrap_or(600),
                    });
                    steps.push(PlanStep {
                        step_id: "review-1".to_string(),
                        description: "Review and verify output".to_string(),
                        mode: ModeKind::SafeGuard,
                        agent: None,
                        depends_on: vec![exec_id],
                        timeout_seconds: 120,
                    });
                }
            }
            TaskComplexity::Complex => {
                // Complex task: full DAG with research → multi-execute → review
                let plan_step = PlanStep {
                    step_id: "plan-1".to_string(),
                    description: format!("Deep analysis: {}", task.objective),
                    mode: ModeKind::Agent,
                    agent: None,
                    depends_on: vec![],
                    timeout_seconds: 300,
                };
                steps.push(plan_step);

                let subtask_count = context.subtask_hints.len().max(3);
                let mut parallel_ids: Vec<String> = Vec::new();
                for i in 0..subtask_count {
                    let sub_id = format!("exec-{}", i + 1);
                    let description = if i < context.subtask_hints.len() {
                        format!("Subtask {}: {}", i + 1, context.subtask_hints[i])
                    } else {
                        format!("Execution component {}", i + 1)
                    };
                    steps.push(PlanStep {
                        step_id: sub_id.clone(),
                        description,
                        mode: ModeKind::FullAuto,
                        agent: None,
                        depends_on: vec!["plan-1".to_string()],
                        timeout_seconds: 600,
                    });
                    parallel_ids.push(sub_id);
                }
                if parallel_ids.len() >= 2 {
                    parallel_groups.push(parallel_ids.clone());
                }

                steps.push(PlanStep {
                    step_id: "review-1".to_string(),
                    description: "Review and verify consolidated output".to_string(),
                    mode: ModeKind::SafeGuard,
                    agent: None,
                    depends_on: parallel_ids,
                    timeout_seconds: 300,
                });
            }
        }

        let metrics = DagMetrics {
            width: if parallel_groups.is_empty() {
                1
            } else {
                parallel_groups.iter().map(|g| g.len()).max().unwrap_or(1)
            },
            depth: steps.len(),
            parallel_group_count: parallel_groups.len(),
            total_steps: steps.len(),
            complexity_level: format!("{:?}", context.complexity),
        };

        ExecutionPlan {
            plan_id,
            steps,
            parallel_groups,
            dag_metrics: Some(metrics),
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
        let mut failed: Vec<String> = Vec::new();

        for step in &plan.steps {
            // Check dependencies
            let deps_met = step.depends_on.iter().all(|d| completed.contains(d));
            // Check for upstream failures — short-circuit to avoid cascading "dependencies not met" errors
            let upstream_failed: Vec<&String> = step
                .depends_on
                .iter()
                .filter(|d| failed.contains(d))
                .collect();
            if !upstream_failed.is_empty() {
                failed.push(step.step_id.clone());
                results.push((
                    step.step_id.clone(),
                    Err(format!(
                        "cancelled due to upstream failure: {:?}",
                        upstream_failed
                    )),
                ));
                continue;
            }
            if !deps_met {
                results.push((
                    step.step_id.clone(),
                    Err(format!("dependencies not met: {:?}", step.depends_on)),
                ));
                continue;
            }

            // Find the runtime for this mode
            let runtime = _runtimes.iter().find(|(kind, _)| *kind == step.mode);

            match runtime {
                Some((_kind, rt)) => {
                    // Build a task envelope for this step
                    let envelope = AgentTaskEnvelope {
                        task_id: format!("plan-{}_{}", plan.plan_id, step.step_id),
                        phase: "execution".to_string(),
                        role: step.agent.clone().unwrap_or_else(|| "agent".to_string()),
                        objective: step.description.clone(),
                        constraints: None,
                        evidence: None,
                        input: serde_json::json!({
                            "step": &step.step_id,
                            "mode": format!("{:?}", _kind),
                        }),
                    };
                    match rt.run(envelope) {
                        Ok(result) => {
                            completed.push(step.step_id.clone());
                            results.push((step.step_id.clone(), Ok(result)));
                        }
                        Err(e) => {
                            failed.push(step.step_id.clone());
                            results.push((
                                step.step_id.clone(),
                                Err(format!("runtime execution failed: {}", e)),
                            ));
                        }
                    }
                }
                None => {
                    failed.push(step.step_id.clone());
                    results.push((
                        step.step_id.clone(),
                        Err(format!("no runtime found for mode {:?}", step.mode)),
                    ));
                }
            }
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
    fn test_plan_creates_variable_steps_by_complexity() {
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
        let medium_plan = Planner::plan(&medium_task);
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

    #[test]
    fn test_dag_metrics_expose_width_and_depth() {
        let task = make_task();
        let plan = Planner::plan(&task);
        let metrics = plan.dag_metrics.unwrap();
        assert!(metrics.width >= 1);
        assert!(metrics.depth >= 1);
        assert_eq!(metrics.total_steps, plan.steps.len());
        assert!(!metrics.complexity_level.is_empty());
    }

    #[test]
    fn test_plan_creates_three_steps_legacy_compat() {
        let task = make_task();
        let plan = Planner::plan(&task);
        assert!(!plan.plan_id.is_empty());
        assert!(plan.steps.len() >= 2);
    }

    #[test]
    fn test_plan_steps_have_correct_dependency_order() {
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

    #[test]
    fn test_execute_returns_results_for_all_steps() {
        let task = make_task();
        let plan = Planner::plan(&task);
        let registry = AgentRegistry::default();
        let results = Executor::execute(&plan, &registry, &[]);
        // With no runtimes:
        // plan-1 (no deps) -> "no runtime found"
        // exec-1 (depends on plan-1, which failed) -> "cancelled due to upstream failure"
        // review-1 (depends on exec-1, which was cancelled) -> "cancelled due to upstream failure"
        assert_eq!(results.len(), 3);
        assert!(results[0].1.is_err());
        assert!(results[1].1.is_err());
        assert!(results[2].1.is_err());
    }

    #[test]
    fn test_execute_with_missing_dependency() {
        // Create a plan where exec-1 depends on plan-1, but plan-1 will fail
        // because there's no runtime. The dependency should still be tracked.
        let task = make_task();
        let plan = Planner::plan(&task);
        let registry = AgentRegistry::default();
        let results = Executor::execute(&plan, &registry, &[]);
        // First step (plan-1) fails with "no runtime found"
        assert!(results[0].1.is_err());
        // Second step (exec-1) depends on plan-1 which failed
        // -> short-circuit: "cancelled due to upstream failure"
        assert!(results[1].1.is_err());
    }
}
