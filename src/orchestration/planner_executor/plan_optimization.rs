//! Plan optimization — adaptive DAG construction from task analysis
//!
//! Provides the `Planner` struct that decomposes a task envelope into an
//! `ExecutionPlan` using embedding-based classification and keyword heuristics.

use super::*;

/// Planner: decomposes a task into an execution plan
pub struct Planner;

impl Planner {
    /// Decompose a task envelope into an execution plan.
    ///
    /// Uses EmbeddingTaskClassifier for semantic task complexity detection,
    /// falling back to keyword heuristics (analyze_task) when embedding unavailable.
    pub fn plan(task: &AgentTaskEnvelope) -> ExecutionPlan {
        // Classify task via embedding-based classifier
        let classifier = EmbeddingTaskClassifier::default();
        let task_category = classifier.classify_task(&task.objective);
        info!(
            "EmbeddingTaskClassifier: task_category={:?}, complexity_score={:.2}",
            task_category,
            match task_category {
                TaskComplexity::Simple => 0.25,
                TaskComplexity::Medium => 0.50,
                TaskComplexity::Complex => 0.75,
            }
        );

        // Build planning context directly from classifier result.
        // analyze_task() provides keyword-based fallback context
        // (subtask_hints, has_code, has_research, has_multiple_subtasks),
        // while the primary complexity decision comes from the embedding classifier.
        let keyword_ctx = Planner::analyze_task(task);
        let context = PlanningContext {
            complexity: task_category,
            has_code: keyword_ctx.has_code,
            has_research: keyword_ctx.has_research,
            has_multiple_subtasks: keyword_ctx.has_multiple_subtasks,
            subtask_hints: keyword_ctx.subtask_hints,
        };
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
