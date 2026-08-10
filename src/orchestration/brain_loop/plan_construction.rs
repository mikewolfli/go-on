//! Plan construction — adaptive DAG decomposition of tasks into execution plans.
//!
//! Provides the `Planner` struct that decomposes a task envelope into an
//! `ExecutionPlan` using keyword + subtask-hint heuristics (`analyze_task`).
//! Moved from `planner_executor::plan_optimization` for direct use by BrainLoop.

use crate::agent::AgentTaskEnvelope;
use crate::orchestration::mode::ModeKind;
use crate::orchestration::planner_executor::{ExecutionPlan, PlanStep};
use serde::{Deserialize, Serialize};
use tracing::info;

/// DAG metrics for governance observability
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagMetrics {
    pub width: usize,
    pub depth: usize,
    pub parallel_group_count: usize,
    pub total_steps: usize,
    pub complexity_level: String,
}

impl Default for DagMetrics {
    fn default() -> Self {
        Self {
            width: 0,
            depth: 0,
            parallel_group_count: 0,
            total_steps: 0,
            complexity_level: "Unknown".into(),
        }
    }
}

/// Task complexity level for adaptive planning
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TaskComplexity {
    #[default]
    Simple,
    Medium,
    Complex,
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

/// Planner: decomposes a task into an execution plan
pub struct Planner;

impl Planner {
    /// Decompose a task envelope into an execution plan.
    ///
    /// Classifies the task exactly once via `Self::analyze_task` (keyword +
    /// subtask-hint heuristics) and feeds the resulting context into the
    /// adaptive DAG planner. The former `EmbeddingTaskClassifier` pass was
    /// removed: its rules had diverged from `analyze_task` (it ignored
    /// `subtask_hints`) and its complexity result was overwritten by the
    /// analyze_task context anyway, so the objective was classified twice for
    /// no observable effect.
    pub async fn plan(task: &AgentTaskEnvelope) -> ExecutionPlan {
        let context = Planner::analyze_task(task);
        info!(
            "Planner::analyze_task: complexity={:?}, has_code={}, has_research={}, has_multiple_subtasks={}, subtask_hints={}, complexity_score={:.2}",
            context.complexity,
            context.has_code,
            context.has_research,
            context.has_multiple_subtasks,
            context.subtask_hints.len(),
            match context.complexity {
                TaskComplexity::Simple => 0.25,
                TaskComplexity::Medium => 0.50,
                TaskComplexity::Complex => 0.75,
            }
        );
        Planner::plan_to_dag(task, &context)
    }

    /// Analyze task characteristics to determine adaptive planning context.
    fn analyze_task(task: &AgentTaskEnvelope) -> PlanningContext {
        // Single authoritative analysis: `TaskRouter::analyze_task` owns the
        // keyword-based classification (task type, complexity, capabilities,
        // multi-module involvement). The former independent keyword tables
        // (code/research/multiple detection) were deleted so no third
        // classifier can drift from the router.
        use crate::orchestration::task_router::{TaskRouter, TaskType};
        let characteristics = TaskRouter::analyze_task(&task.objective);

        let has_code = matches!(
            &characteristics.task_type,
            TaskType::BugFix
                | TaskType::FeatureImplementation
                | TaskType::Refactoring
                | TaskType::TestImplementation
                | TaskType::PerformanceOptimization
                | TaskType::CodeReview
        );

        // TaskRouter has no dedicated research task type, so the research
        // flag is approximated from the closest task-type signals (design/
        // documentation tasks require analysis). These structural flags are
        // informational (logged; `plan_to_dag` only consumes complexity and
        // subtask_hints).
        let has_research = matches!(
            &characteristics.task_type,
            TaskType::ArchitectureDesign | TaskType::Documentation
        );

        // Extract subtask hints from task input or objective (TaskRouter has
        // no equivalent capability, so this stays local).
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

        // Map TaskRouter's 1–5 complexity onto the planner's complexity bands.
        let complexity = match characteristics.complexity {
            1 | 2 => TaskComplexity::Simple,
            3 => TaskComplexity::Medium,
            _ => TaskComplexity::Complex,
        };

        PlanningContext {
            complexity,
            has_code,
            has_research,
            has_multiple_subtasks: characteristics.involves_multiple_modules,
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
                        mode: ModeKind::Edit,
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
                        mode: ModeKind::Edit,
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
                    mode: ModeKind::Edit,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentTaskEnvelope;

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
                "Fix the performance bug in the authentication module and verify everything works correctly"
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
            objective: "Research the authentication module, redesign the middleware chain to use JWT, and build comprehensive unit tests".to_string(),
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

    /// The planner's complexity must match the single authoritative
    /// classifier (`TaskRouter::estimate_complexity`) — the former independent
    /// keyword rules were removed when the three complexity classifiers were
    /// unified (A1).
    #[test]
    fn test_analyze_task_matches_unified_complexity_classifier() {
        let envelope = |objective: &str| AgentTaskEnvelope {
            task_id: "classify-1".into(),
            phase: "coding".into(),
            role: "coder".into(),
            objective: objective.to_string(),
            constraints: None,
            evidence: None,
            input: serde_json::json!({}),
        };

        // Simple: baseline score (2) with no complexity keywords.
        assert_eq!(
            Planner::analyze_task(&envelope("Greet the user")).complexity,
            TaskComplexity::Simple
        );
        assert_eq!(
            Planner::analyze_task(&envelope("Hello world")).complexity,
            TaskComplexity::Simple
        );
        assert_eq!(
            Planner::analyze_task(&envelope("Hi")).complexity,
            TaskComplexity::Simple
        );
        assert_eq!(
            Planner::analyze_task(&envelope("")).complexity,
            TaskComplexity::Simple
        );

        // Medium: baseline (2) + one complexity keyword (+1) -> 3.
        assert_eq!(
            Planner::analyze_task(&envelope(
                "Fix the performance bug in the authentication module"
            ))
            .complexity,
            TaskComplexity::Medium
        );

        // Complex: baseline (2) + strong keyword (+2) -> 4.
        assert_eq!(
            Planner::analyze_task(&envelope(
                "Redesign the authentication module and rewrite the middleware chain"
            ))
            .complexity,
            TaskComplexity::Complex
        );
    }

    /// `analyze_task` still extracts subtask hints into the planning context
    /// (they shape the DAG inside `plan_to_dag`), but complexity itself comes
    /// exclusively from the unified keyword classifier.
    #[test]
    fn test_analyze_task_extracts_subtask_hints() {
        let envelope_with_hints = |hints: Vec<&str>| AgentTaskEnvelope {
            task_id: "hints-1".into(),
            phase: "coding".into(),
            role: "coder".into(),
            objective: "Short objective".to_string(),
            constraints: None,
            evidence: None,
            input: serde_json::json!({ "subtasks": hints }),
        };

        // Hints are extracted, but a short hint-free objective stays Simple
        // under the unified keyword classifier.
        let ctx = Planner::analyze_task(&envelope_with_hints(vec!["a", "b", "c", "d"]));
        assert_eq!(ctx.complexity, TaskComplexity::Simple);
        assert_eq!(ctx.subtask_hints.len(), 4);

        // Bullet-style subtasks in the objective are still extracted.
        let bullet_task = AgentTaskEnvelope {
            task_id: "hints-2".into(),
            phase: "coding".into(),
            role: "coder".into(),
            objective: "- design the schema\n- implement the endpoint\n".to_string(),
            constraints: None,
            evidence: None,
            input: serde_json::json!({}),
        };
        let ctx = Planner::analyze_task(&bullet_task);
        assert_eq!(ctx.subtask_hints.len(), 2);
    }

    /// `Planner::plan` classifies exactly once: the DAG complexity level must
    /// equal `analyze_task`'s result (which is the unified keyword classifier).
    #[tokio::test]
    async fn test_plan_uses_analyze_task_complexity() {
        let task = AgentTaskEnvelope {
            task_id: "plan-hints-1".into(),
            phase: "coding".into(),
            role: "coder".into(),
            objective: "Redesign the ticket handling flow and rewrite the scheduler".to_string(),
            constraints: None,
            evidence: None,
            input: serde_json::json!({
                "subtasks": ["one", "two", "three", "four"],
            }),
        };
        let expected = Planner::analyze_task(&task).complexity;
        assert_eq!(expected, TaskComplexity::Complex);
        let plan = Planner::plan(&task).await;
        assert_eq!(
            plan.dag_metrics.unwrap().complexity_level,
            format!("{:?}", expected)
        );
        // Complex plan: research + parallel execution + review.
        assert!(plan.steps.len() >= 4);
    }

    /// The structural flags must be derived from TaskRouter's characteristics
    /// (the delegation target) — they are informational, but must agree with
    /// the router's task-type classification rather than a third keyword table.
    #[test]
    fn test_analyze_task_flags_derive_from_task_router() {
        let envelope = |objective: &str| AgentTaskEnvelope {
            task_id: "flags-1".into(),
            phase: "coding".into(),
            role: "coder".into(),
            objective: objective.to_string(),
            constraints: None,
            evidence: None,
            input: serde_json::json!({}),
        };

        let code_ctx = Planner::analyze_task(&envelope("Implement a new feature and write tests"));
        assert!(
            code_ctx.has_code,
            "code-oriented task type must set has_code"
        );
        assert!(!code_ctx.has_research);

        let design_ctx = Planner::analyze_task(&envelope("Design the API architecture"));
        assert!(
            design_ctx.has_research,
            "design task type must set has_research"
        );

        let plain_ctx = Planner::analyze_task(&envelope("Greet the user"));
        assert!(!plain_ctx.has_code);
        assert!(!plain_ctx.has_research);
        assert!(!plain_ctx.has_multiple_subtasks);
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
}
