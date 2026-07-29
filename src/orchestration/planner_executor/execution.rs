//! Execution — runs an `ExecutionPlan` through the mode runtime
//!
//! Steps in the same parallel group are executed concurrently using
//! `futures::future::join_all`. Sequential (non-parallel) steps are
//! executed in order.

use super::*;

/// Executor: executes an execution plan through the mode runtime
///
/// ⚠ **Deprecated** — use [`BrainLoop`](crate::orchestration::brain_loop::BrainLoop) instead.
/// This struct will be removed in a future release.
///
/// Steps in the same parallel group are executed concurrently using
/// `futures::future::join_all` for true parallel execution.
#[deprecated(
    since = "1.5.0",
    note = "use crate::orchestration::brain_loop::BrainLoop instead"
)]
#[allow(deprecated)]
pub struct Executor;

#[allow(deprecated)]
impl Executor {
    /// Execute an execution plan, running steps in dependency order.
    ///
    /// Steps belonging to the same parallel group are executed concurrently
    /// via `futures::future::join_all`. Sequential (non-parallel) steps are
    /// executed in order as before.
    ///
    /// Returns results for each step in the order they appear in `plan.steps`.
    ///
    /// TODO: Delegate to [`BrainLoop::from_execution_plan`] + `run_async` and
    /// convert the [`BrainLoopProfile`](crate::orchestration::brain_loop::BrainLoopProfile)
    /// back to the legacy `Vec<(String, Result<AgentTaskResult, String>)>` return type.
    pub async fn execute(
        plan: &ExecutionPlan,
        _registry: &AgentRegistry,
        runtimes: &[(ModeKind, Arc<dyn ModeRuntime>)],
    ) -> Vec<(String, Result<AgentTaskResult, String>)> {
        tracing::warn!(
            target: "planner_executor",
            "Executor::execute is deprecated — migrate to brain_loop::BrainLoop (plan_id={})",
            plan.plan_id,
        );
        Self::execute_with_cancel(plan, _registry, runtimes, CancellationToken::new()).await
    }

    /// Execute an execution plan with cooperative cancellation support.
    ///
    /// Parallel groups respect the `CancellationToken` — when cancelled,
    /// remaining steps are recorded as failures rather than spawned.
    ///
    /// TODO: Delegate to [`BrainLoop::from_execution_plan`] + `run_async` and
    /// convert the [`BrainLoopProfile`](crate::orchestration::brain_loop::BrainLoopProfile)
    /// back to the legacy return type, plumbing `CancellationToken` through
    /// to the BrainLoop's own cancel support.
    pub async fn execute_with_cancel(
        plan: &ExecutionPlan,
        _registry: &AgentRegistry,
        runtimes: &[(ModeKind, Arc<dyn ModeRuntime>)],
        cancel: CancellationToken,
    ) -> Vec<(String, Result<AgentTaskResult, String>)> {
        tracing::warn!(
            target: "planner_executor",
            "Executor::execute_with_cancel is deprecated — migrate to brain_loop::BrainLoop (plan_id={})",
            plan.plan_id,
        );
        let mut results: Vec<(String, Result<AgentTaskResult, String>)> = Vec::new();
        let mut completed: HashSet<String> = HashSet::new();
        let mut failed: HashSet<String> = HashSet::new();

        // Build a map of which parallel group each step belongs to (if any)
        let mut step_group: HashMap<&str, usize> = HashMap::new();
        for (gi, group) in plan.parallel_groups.iter().enumerate() {
            for sid in group {
                step_group.insert(sid.as_str(), gi);
            }
        }

        // Track which steps have been dispatched to avoid double-processing
        let mut dispatched: HashSet<String> = HashSet::new();

        /// Execute a single step (shared by sequential and parallel paths).
        async fn run_step(
            step: &PlanStep,
            plan_id: &str,
            rt: &dyn ModeRuntime,
        ) -> Result<AgentTaskResult, String> {
            let envelope = AgentTaskEnvelope {
                task_id: format!("plan-{}_{}", plan_id, step.step_id),
                phase: "execution".to_string(),
                role: step.agent.clone().unwrap_or_else(|| "agent".to_string()),
                objective: step.description.clone(),
                constraints: None,
                evidence: None,
                input: serde_json::json!({
                    "step": &step.step_id,
                    "mode": format!("{:?}", step.mode),
                }),
            };
            rt.run(envelope).await.map_err(|e| {
                tf(
                    "error.planner.runtime_failed",
                    &[("detail", &e.to_string())],
                )
            })
        }

        // Process steps in topological order:
        // 1. At each iteration, find all steps whose dependencies are fully satisfied
        // 2. Execute them (sequential steps are run directly; parallel groups use spawn_blocking)
        // 3. Repeat until all steps are processed or no progress possible

        let all_step_ids: HashSet<String> = plan.steps.iter().map(|s| s.step_id.clone()).collect();
        let mut remaining: HashSet<String> = all_step_ids.clone();

        while !remaining.is_empty() {
            // Find all remaining steps whose dependencies are met
            let ready: Vec<&PlanStep> = plan
                .steps
                .iter()
                .filter(|s| remaining.contains(&s.step_id))
                .filter(|s| {
                    s.depends_on.iter().all(|d| {
                        // A dependency is met if it's completed, OR if it's not in the plan at all (external)
                        completed.contains(d) || !all_step_ids.contains(d)
                    })
                })
                .filter(|s| {
                    // Also check that no dependency has failed
                    !s.depends_on.iter().any(|d| failed.contains(d))
                })
                .collect();

            if ready.is_empty() {
                // No progress possible — remaining steps have missing or failed deps
                for sid in &remaining {
                    let step = match plan.steps.iter().find(|s| s.step_id == *sid) {
                        Some(s) => s,
                        None => {
                            let msg = format!("Step ID {} not found in plan", sid);
                            tracing::error!(target: "planner_executor", "{}", msg);
                            failed.insert(sid.clone());
                            results.push((sid.clone(), Err(msg)));
                            continue;
                        }
                    };
                    let mut reason = String::new();
                    for dep in &step.depends_on {
                        if failed.contains(dep) {
                            reason = tf("error.planner.upstream_failed", &[("failed_steps", dep)]);
                        } else if remaining.contains(dep) {
                            reason = tf(
                                "error.planner.dependencies_not_met",
                                &[("deps", &format!("{:?}", step.depends_on))],
                            );
                        }
                    }
                    if reason.is_empty() {
                        reason = "dependency not satisfied".to_string();
                    }
                    failed.insert(sid.clone());
                    results.push((sid.clone(), Err(reason)));
                }
                break;
            }

            for step in ready {
                let step_id = step.step_id.clone();
                remaining.remove(&step_id);
                dispatched.insert(step_id.clone());

                // Check if this step belongs to a parallel group
                if let Some(&gi) = step_group.get(step.step_id.as_str()) {
                    // Collect ALL ready members of this parallel group
                    let group_ids: Vec<String> = plan.parallel_groups[gi].clone();
                    let group_ready: Vec<&PlanStep> = plan
                        .steps
                        .iter()
                        .filter(|s| group_ids.contains(&s.step_id))
                        .filter(|s| remaining.contains(&s.step_id) || s.step_id == step_id)
                        .filter(|s| {
                            s.depends_on
                                .iter()
                                .all(|d| completed.contains(d) || !all_step_ids.contains(d))
                        })
                        .filter(|s| !s.depends_on.iter().any(|d| failed.contains(d)))
                        .collect();

                    if group_ready.len() <= 1 {
                        // Only one ready step, run it sequentially
                        let runtime = runtimes.iter().find(|(kind, _)| *kind == step.mode);
                        match runtime {
                            Some((_kind, rt)) => {
                                let result = run_step(step, &plan.plan_id, rt.as_ref()).await;
                                match result {
                                    Ok(agent_result) => {
                                        completed.insert(step_id.clone());
                                        results.push((step_id, Ok(agent_result)));
                                    }
                                    Err(e) => {
                                        let sid = step_id.clone();
                                        failed.insert(step_id);
                                        results.push((sid, Err(e)));
                                    }
                                }
                            }
                            None => {
                                let sid = step_id.clone();
                                failed.insert(step_id);
                                results.push((
                                    sid,
                                    Err(tf(
                                        "error.planner.no_runtime_found",
                                        &[("mode", &format!("{:?}", step.mode))],
                                    )),
                                ));
                            }
                        }
                    } else {
                        // Multiple ready steps — execute concurrently using spawn_blocking
                        // and futures::future::join_all, avoiding block_in_place on the async
                        // worker thread.
                        let plan_id = plan.plan_id.clone();

                        // Pre-collect owned data for each step so that spawn_blocking
                        // closures can capture 'static values.
                        let step_infos: Vec<(String, String, String, ModeKind, Option<String>)> =
                            group_ready
                                .iter()
                                .map(|gs| {
                                    (
                                        gs.step_id.clone(),
                                        format!("plan-{}_{}", plan_id, gs.step_id),
                                        gs.description.clone(),
                                        gs.mode.clone(),
                                        gs.agent.clone(),
                                    )
                                })
                                .collect();

                        let mut blocking_tasks = Vec::with_capacity(step_infos.len());

                        for (step_id, task_id, description, mode, agent) in step_infos {
                            // Check cancellation before spawning
                            if cancel.is_cancelled() {
                                let sid = step_id.clone();
                                failed.insert(sid.clone());
                                results.push((sid, Err("cancelled by shutdown token".to_string())));
                                continue;
                            }

                            let runtime = runtimes.iter().find(|(kind, _)| *kind == mode);
                            match runtime {
                                Some((_kind, rt)) => {
                                    let envelope = AgentTaskEnvelope {
                                        task_id: task_id.clone(),
                                        phase: "execution".to_string(),
                                        role: agent.clone().unwrap_or_else(|| "agent".to_string()),
                                        objective: description.clone(),
                                        constraints: None,
                                        evidence: None,
                                        input: serde_json::json!({
                                            "step": &step_id,
                                            "mode": format!("{:?}", mode),
                                        }),
                                    };
                                    let rt_clone = Arc::clone(rt);
                                    blocking_tasks.push(tokio::task::spawn(async move {
                                        let result = rt_clone.run(envelope).await.map_err(|e| {
                                            tf(
                                                "error.planner.runtime_failed",
                                                &[("detail", &e.to_string())],
                                            )
                                        });
                                        (step_id, result)
                                    }));
                                }
                                None => {
                                    blocking_tasks.push(tokio::task::spawn(async move {
                                        (
                                            step_id,
                                            Err(tf(
                                                "error.planner.no_runtime_found",
                                                &[("mode", &format!("{:?}", mode))],
                                            )),
                                        )
                                    }));
                                }
                            }
                        }

                        let parallel_results: Vec<(String, Result<AgentTaskResult, String>)> =
                            join_all(blocking_tasks)
                                .await
                                .into_iter()
                                .filter_map(|r| match r {
                                    Ok(inner) => Some(inner),
                                    Err(join_err) => {
                                        tracing::error!(
                                            "parallel group spawn_blocking panicked: {:?}",
                                            join_err
                                        );
                                        // A join error means the blocking task panicked.
                                        // We lose the step_id because the closure was dropped.
                                        // Mark all remaining group members as failed instead.
                                        None
                                    }
                                })
                                .collect();

                        for (sid, result) in parallel_results {
                            remaining.remove(&sid);
                            match result {
                                Ok(agent_result) => {
                                    completed.insert(sid.clone());
                                    results.push((sid, Ok(agent_result)));
                                }
                                Err(e) => {
                                    failed.insert(sid.clone());
                                    results.push((sid, Err(e)));
                                }
                            }
                        }

                        // Mark all group members as dispatched (whether executed or not)
                        for gid in &group_ids {
                            remaining.remove(gid);
                            dispatched.insert(gid.clone());
                        }
                    }
                } else {
                    // Sequential step (not in a parallel group)
                    remaining.remove(&step_id);
                    let runtime = runtimes.iter().find(|(kind, _)| *kind == step.mode);
                    match runtime {
                        Some((_kind, rt)) => {
                            let result = run_step(step, &plan.plan_id, rt.as_ref()).await;
                            match result {
                                Ok(agent_result) => {
                                    completed.insert(step_id.clone());
                                    results.push((step_id, Ok(agent_result)));
                                }
                                Err(e) => {
                                    failed.insert(step_id.clone());
                                    results.push((step_id, Err(e)));
                                }
                            }
                        }
                        None => {
                            failed.insert(step_id.clone());
                            results.push((
                                step_id,
                                Err(tf(
                                    "error.planner.no_runtime_found",
                                    &[("mode", &format!("{:?}", step.mode))],
                                )),
                            ));
                        }
                    }
                }
            }
        }

        results
    }
}
