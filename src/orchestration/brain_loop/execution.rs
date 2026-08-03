//! Execution-related methods for [`BrainLoop`](super::BrainLoop).
//!
//! Actively wired in production: HarnessBus, autonomy_loop_adapter, chat_phases.rs.
//! The cognitive loop in chat_phases.rs is an additional path; the brain loop
//! remains the primary iterative orchestration driver.

use super::{now_epoch_ms, tf, BrainLoop, BrainLoopPhase, StepStatus, TaskContext};

// ---------------------------------------------------------------------------
// Execution (async)
// ---------------------------------------------------------------------------

impl BrainLoop {
    /// Execute a specific step inside a plan.
    ///
    /// Marks the step as `InProgress`, records `output`, advances the plan
    /// phase to `Executing`, and bumps the cycle counter if this is the
    /// first step executed in a new iteration.
    pub async fn execute_step(
        &self,
        plan_id: &str,
        step_id: &str,
        output: &str,
    ) -> anyhow::Result<()> {
        let now = now_epoch_ms();
        let mut inner = self.inner.write().await;

        // Phase 1: validate and check iteration limit.
        let plan_failed = {
            let plan = inner.plans.get_mut(plan_id).ok_or_else(|| {
                anyhow::anyhow!("{}", tf("error.plan_not_found", &[("id", plan_id)]))
            })?;

            if plan.phase.is_terminal() {
                anyhow::bail!("{}", tf("error.plan_already_terminal", &[("id", plan_id)]));
            }

            let step_idx = plan
                .steps
                .iter()
                .position(|s| s.id == step_id)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "{}",
                        tf(
                            "error.step_not_found",
                            &[("id", step_id), ("plan_id", plan_id)]
                        )
                    )
                })?;

            if plan.steps[step_idx].status == StepStatus::Done {
                anyhow::bail!("{}", tf("error.step_already_done", &[("id", step_id)]));
            }

            // Iteration transition – check limit BEFORE incrementing.
            let was_planning =
                plan.phase == BrainLoopPhase::Planning || plan.phase == BrainLoopPhase::Replanning;
            if was_planning && plan.steps[step_idx].status == StepStatus::Pending {
                if plan.current_iteration >= plan.max_iterations {
                    plan.phase = BrainLoopPhase::Failed;
                    plan.fail_reason =
                        format!("exceeded maximum iterations ({})", plan.max_iterations);
                    true
                } else {
                    plan.current_iteration += 1;
                    inner.total_cycles += 1;
                    false
                }
            } else {
                false
            }
        };

        if plan_failed {
            inner.failed_plans_total += 1;
            BrainLoop::evict_oldest_terminal_plan(&mut inner.plans);
            return Ok(());
        }

        // Phase 2: mark step in-progress (separate scope to release plan borrow).
        {
            let plan = inner.plans.get_mut(plan_id).ok_or_else(|| {
                anyhow::anyhow!("{}", tf("error.plan_not_found", &[("id", plan_id)]))
            })?;

            let step_idx = plan
                .steps
                .iter()
                .position(|s| s.id == step_id)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "{}",
                        tf(
                            "error.step_not_found",
                            &[("id", step_id), ("plan_id", plan_id)]
                        )
                    )
                })?;

            plan.steps[step_idx].status = StepStatus::InProgress;
            plan.steps[step_idx].started_ms = now;
            plan.steps[step_idx].output = output.to_string();
            plan.phase = BrainLoopPhase::Executing;
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Execute with TaskContext (async)
// ---------------------------------------------------------------------------

impl BrainLoop {
    /// Execute a specific step with a [`TaskContext`], returning the updated
    /// context after execution.
    ///
    /// This is the chain-of-thought-aware version of [`execute_step`].  The
    /// caller provides the reasoning context before execution; this method
    /// attaches it to the step, then calls [`execute_step`] internally.
    /// The returned [`TaskContext`] can be passed to downstream steps for
    /// reasoning chain continuity.
    pub async fn execute_step_with_context(
        &self,
        plan_id: &str,
        step_id: &str,
        output: &str,
        context: TaskContext,
    ) -> anyhow::Result<TaskContext> {
        // First, attach the context to the step.
        {
            let mut inner = self.inner.write().await;
            let plan = inner.plans.get_mut(plan_id).ok_or_else(|| {
                anyhow::anyhow!("{}", tf("error.plan_not_found", &[("id", plan_id)]))
            })?;
            let step_idx = plan
                .steps
                .iter()
                .position(|s| s.id == step_id)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "{}",
                        tf(
                            "error.step_not_found",
                            &[("id", step_id), ("plan_id", plan_id)]
                        )
                    )
                })?;
            plan.steps[step_idx].context = Some(context);
        }

        // Execute the step normally.
        self.execute_step(plan_id, step_id, output).await?;

        // Retrieve the step's updated context to return.
        let inner = self.inner.read().await;
        let plan = inner
            .plans
            .get(plan_id)
            .ok_or_else(|| anyhow::anyhow!("{}", tf("error.plan_not_found", &[("id", plan_id)])))?;
        let step = plan.steps.iter().find(|s| s.id == step_id).ok_or_else(|| {
            anyhow::anyhow!(
                "{}",
                tf(
                    "error.step_not_found",
                    &[("id", step_id), ("plan_id", plan_id)]
                )
            )
        })?;
        Ok(step
            .context
            .as_ref()
            .cloned()
            .unwrap_or_else(|| TaskContext::new("empty-after-execute".to_string())))
    }
}
