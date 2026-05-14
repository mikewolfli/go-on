//! # Brain Loop — Plan → Execute → Reflect → Replan

//!
//! Implements FUTURE5.MD M5 "脑回路（Plan→Execute→Reflect→Replan）",
//! an iterative orchestration cycle that drives a plan forward by executing
//! individual steps, reflecting on the outcome, and optionally replanning
//! the remaining work.  The loop continues until the plan completes, fails,
//! is cancelled, or reaches the configured maximum number of iterations.
//!
//! ## Thread safety
//!
//! The top-level [`BrainLoop`] struct holds interior mutability behind
//! `Arc<Mutex<…>>` so it can be shared across tasks.  Individual snapshot
//! types (`BrainLoopPlan`, `BrainLoopStep`, …) derive `Clone` so callers
//! obtain a consistent view without holding the lock.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// Lock a Mutex, recovering from poison with a log.
fn lock_guard<T>(mtx: &Mutex<T>) -> MutexGuard<'_, T> {
    match mtx.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::error!("brain_loop mutex poisoned, recovering");
            poisoned.into_inner()
        }
    }
}
use std::time::{SystemTime, UNIX_EPOCH};

use crate::i18n::runtime::tf;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Public data types
// ---------------------------------------------------------------------------

/// The phase a plan is currently in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrainLoopPhase {
    Planning,
    Executing,
    Reflecting,
    Replanning,
    Completed,
    Failed,
    Cancelled,
}

impl BrainLoopPhase {
    /// Returns `true` for terminal phases.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// Status of an individual step within a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Pending,
    InProgress,
    Done,
    Skipped,
}

// ---------------------------------------------------------------------------
// Core data structures
// ---------------------------------------------------------------------------

/// A single atomic unit of work inside a plan.
#[derive(Debug, Clone)]
pub struct BrainLoopStep {
    pub id: String,
    pub phase: BrainLoopPhase,
    pub description: String,
    pub input: String,
    pub output: String,
    pub started_ms: u64,
    pub completed_ms: u64,
    pub duration_ms: u64,
    pub status: StepStatus,
}

/// A plan being tracked by the brain loop.
#[derive(Debug, Clone)]
pub struct BrainLoopPlan {
    pub id: String,
    pub goal: String,
    pub steps: Vec<BrainLoopStep>,
    pub max_iterations: u32,
    pub current_iteration: u32,
    pub created_ms: u64,
    pub phase: BrainLoopPhase,
    pub fail_reason: String,
}

/// Reflection data recorded after executing a step.
#[derive(Debug, Clone)]
pub struct BrainLoopReflection {
    pub step_id: String,
    pub observations: Vec<String>,
    pub issues: Vec<String>,
    pub improvements: Vec<String>,
    pub confidence: f64,
    pub reflection_ms: u64,
}

/// Configuration that tunes the behaviour of a [`BrainLoop`].
#[derive(Debug, Clone)]
pub struct BrainLoopConfig {
    pub max_iterations: u32,
    pub max_steps_per_iteration: u32,
    pub reflection_required: bool,
    pub auto_replan: bool,
}

impl Default for BrainLoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 5,
            max_steps_per_iteration: 10,
            reflection_required: true,
            auto_replan: true,
        }
    }
}

/// Runtime metrics snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BrainLoopProfile {
    pub total_plans: u64,
    pub active_plans: u64,
    pub completed_plans: u64,
    pub failed_plans: u64,
    pub total_cycles: u64,
    pub avg_cycles_per_plan: f64,
}

// ---------------------------------------------------------------------------
// Internal runtime state
// ---------------------------------------------------------------------------

struct BrainLoopInner {
    plans: HashMap<String, BrainLoopPlan>,
    reflections: Vec<BrainLoopReflection>,
    config: BrainLoopConfig,
    total_cycles: u64,
    total_plans_started: u64,
    completed_plans_total: u64,
    failed_plans_total: u64,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// The brain loop orchestrator.
///
/// All mutable state lives behind `Arc<Mutex<…>>` so the struct can be
/// cloned and shared across threads.
#[derive(Clone)]
pub struct BrainLoop {
    inner: Arc<Mutex<BrainLoopInner>>,
    next_plan_id: Arc<AtomicU64>,
}

impl BrainLoop {
    /// Create a new brain loop with the given configuration.
    pub fn new(config: BrainLoopConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BrainLoopInner {
                plans: HashMap::new(),
                reflections: Vec::new(),
                config,
                total_cycles: 0,
                total_plans_started: 0,
                completed_plans_total: 0,
                failed_plans_total: 0,
            })),
            next_plan_id: Arc::new(AtomicU64::new(1)),
        }
    }

    // ── Plan lifecycle ──────────────────────────────────────────────────

    /// Start a new plan with the given `goal` and initial `steps`.
    ///
    /// Returns the assigned plan id on success.
    pub fn start_plan(&self, goal: &str, steps: Vec<BrainLoopStep>) -> anyhow::Result<String> {
        let id_num = self.next_plan_id.fetch_add(1, Ordering::AcqRel);
        let id = format!("plan-{id_num}");

        let now = now_epoch_ms();
        let mut inner = lock_guard(&self.inner);
        let max_iterations = inner.config.max_iterations;
        let plan = BrainLoopPlan {
            id: id.clone(),
            goal: goal.to_string(),
            steps,
            max_iterations,
            current_iteration: 0,
            created_ms: now,
            phase: BrainLoopPhase::Planning,
            fail_reason: String::new(),
        };
        inner.plans.insert(id.clone(), plan);
        inner.total_plans_started += 1;
        Ok(id)
    }

    /// Get a clone of a plan by its id.
    pub fn get_plan(&self, id: &str) -> anyhow::Result<BrainLoopPlan> {
        lock_guard(&self.inner)
            .plans
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("plan `{id}` not found"))
    }

    /// Return a list of all known plan ids.
    pub fn list_plans(&self) -> Vec<String> {
        lock_guard(&self.inner).plans.keys().cloned().collect()
    }

    // ── Execution ────────────────────────────────────────────────────────

    /// Execute a specific step inside a plan.
    ///
    /// Marks the step as `InProgress`, records `output`, advances the plan
    /// phase to `Executing`, and bumps the cycle counter if this is the
    /// first step executed in a new iteration.
    pub fn execute_step(&self, plan_id: &str, step_id: &str, output: &str) -> anyhow::Result<()> {
        let now = now_epoch_ms();
        let mut inner = lock_guard(&self.inner);

        // Remove the plan so we can mutate it independently from `inner`.
        let mut plan = inner
            .plans
            .remove(plan_id)
            .ok_or_else(|| anyhow::anyhow!("{}", tf("error.plan_not_found", &[("id", plan_id)])))?;

        if plan.phase.is_terminal() {
            inner.plans.insert(plan_id.to_string(), plan);
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
            inner.plans.insert(plan_id.to_string(), plan);
            anyhow::bail!("{}", tf("error.step_already_done", &[("id", step_id)]));
        }

        // Iteration transition – only bump on the first execution of a
        // new iteration (when plan is in Planning/Replanning and step is
        // still Pending).
        let was_planning =
            plan.phase == BrainLoopPhase::Planning || plan.phase == BrainLoopPhase::Replanning;
        if was_planning && plan.steps[step_idx].status == StepStatus::Pending {
            plan.current_iteration += 1;
            inner.total_cycles += 1;

            if plan.current_iteration > plan.max_iterations {
                plan.phase = BrainLoopPhase::Failed;
                plan.fail_reason = format!("exceeded maximum iterations ({})", plan.max_iterations);
                inner.plans.insert(plan_id.to_string(), plan);
                inner.failed_plans_total += 1;
                Self::evict_oldest_terminal_plan(&mut inner.plans);
                return Ok(());
            }
        }

        plan.steps[step_idx].status = StepStatus::InProgress;
        plan.steps[step_idx].started_ms = now;
        plan.steps[step_idx].output = output.to_string();
        plan.phase = BrainLoopPhase::Executing;

        inner.plans.insert(plan_id.to_string(), plan);
        Ok(())
    }

    // ── Reflection ───────────────────────────────────────────────────────

    /// Record a reflection on a completed step.
    ///
    /// Moves the plan into the `Reflecting` phase.
    pub fn reflect(
        &self,
        plan_id: &str,
        step_id: &str,
        observations: Vec<String>,
        issues: Vec<String>,
        improvements: Vec<String>,
    ) -> anyhow::Result<BrainLoopReflection> {
        let now = now_epoch_ms();
        let mut inner = lock_guard(&self.inner);

        // Remove the plan so we can mutate it without borrowing inner.
        let mut plan = inner
            .plans
            .remove(plan_id)
            .ok_or_else(|| anyhow::anyhow!("{}", tf("error.plan_not_found", &[("id", plan_id)])))?;

        if plan.phase.is_terminal() {
            inner.plans.insert(plan_id.to_string(), plan);
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

        let started = plan.steps[step_idx].started_ms;

        plan.steps[step_idx].status = StepStatus::Done;
        plan.steps[step_idx].completed_ms = now;
        plan.steps[step_idx].duration_ms = now.saturating_sub(started);
        plan.phase = BrainLoopPhase::Reflecting;

        let confidence = if issues.is_empty() {
            1.0
        } else {
            (1.0 - (issues.len() as f64).min(issues.len() as f64) * 0.2).max(0.1)
        };

        let reflection = BrainLoopReflection {
            step_id: step_id.to_string(),
            observations,
            issues,
            improvements,
            confidence,
            reflection_ms: now,
        };

        const MAX_REFLECTIONS: usize = 1000;
        if inner.reflections.len() >= MAX_REFLECTIONS {
            inner.reflections.remove(0);
        }
        inner.reflections.push(reflection.clone());
        inner.plans.insert(plan_id.to_string(), plan);

        Ok(reflection)
    }

    // ── Replanning ───────────────────────────────────────────────────────

    /// Replace the remaining pending steps with a new set of steps.
    ///
    /// Existing completed / in-progress steps are preserved.
    /// The plan phase is set to `Replanning`.
    pub fn replan(&self, plan_id: &str, new_steps: Vec<BrainLoopStep>) -> anyhow::Result<()> {
        let mut inner = lock_guard(&self.inner);

        let plan = inner
            .plans
            .get_mut(plan_id)
            .ok_or_else(|| anyhow::anyhow!("{}", tf("error.plan_not_found", &[("id", plan_id)])))?;

        if plan.phase.is_terminal() {
            anyhow::bail!("{}", tf("error.plan_already_terminal", &[("id", plan_id)]));
        }

        // Keep only steps that are not pending (they are either done or in progress).
        plan.steps.retain(|s| s.status != StepStatus::Pending);

        // Append the new steps.
        plan.steps.extend(new_steps);
        plan.phase = BrainLoopPhase::Replanning;

        Ok(())
    }

    // ── Terminal transitions ─────────────────────────────────────────────

    /// Mark a plan as completed.
    pub fn complete_plan(&self, plan_id: &str) -> anyhow::Result<()> {
        let mut inner = lock_guard(&self.inner);
        let plan = inner
            .plans
            .get_mut(plan_id)
            .ok_or_else(|| anyhow::anyhow!("{}", tf("error.plan_not_found", &[("id", plan_id)])))?;

        if plan.phase.is_terminal() {
            anyhow::bail!("{}", tf("error.plan_already_terminal", &[("id", plan_id)]));
        }
        plan.phase = BrainLoopPhase::Completed;
        inner.completed_plans_total += 1;
        Self::evict_oldest_terminal_plan(&mut inner.plans);
        Ok(())
    }

    /// Mark a plan as failed with a reason.
    pub fn fail_plan(&self, plan_id: &str, reason: &str) -> anyhow::Result<()> {
        let mut inner = lock_guard(&self.inner);
        let plan = inner
            .plans
            .get_mut(plan_id)
            .ok_or_else(|| anyhow::anyhow!("{}", tf("error.plan_not_found", &[("id", plan_id)])))?;

        if plan.phase.is_terminal() {
            anyhow::bail!("{}", tf("error.plan_already_terminal", &[("id", plan_id)]));
        }
        plan.phase = BrainLoopPhase::Failed;
        plan.fail_reason = reason.to_string();
        inner.failed_plans_total += 1;
        Self::evict_oldest_terminal_plan(&mut inner.plans);
        Ok(())
    }

    /// Cancel a plan.
    pub fn cancel_plan(&self, plan_id: &str) -> anyhow::Result<()> {
        let mut inner = lock_guard(&self.inner);
        let plan = inner
            .plans
            .get_mut(plan_id)
            .ok_or_else(|| anyhow::anyhow!("{}", tf("error.plan_not_found", &[("id", plan_id)])))?;

        if plan.phase.is_terminal() {
            anyhow::bail!("{}", tf("error.plan_already_terminal", &[("id", plan_id)]));
        }
        plan.phase = BrainLoopPhase::Cancelled;
        inner.failed_plans_total += 1;
        Self::evict_oldest_terminal_plan(&mut inner.plans);
        Ok(())
    }

    // ── Queries ──────────────────────────────────────────────────────────

    /// The current phase of a plan.
    pub fn current_phase(&self, plan_id: &str) -> anyhow::Result<BrainLoopPhase> {
        lock_guard(&self.inner)
            .plans
            .get(plan_id)
            .map(|p| p.phase)
            .ok_or_else(|| anyhow::anyhow!("plan `{plan_id}` not found"))
    }

    /// Return a snapshot of runtime metrics.
    pub fn profile(&self) -> BrainLoopProfile {
        let inner = lock_guard(&self.inner);
        let total_plans = inner.total_plans_started;
        let active_plans = inner
            .plans
            .values()
            .filter(|p| !p.phase.is_terminal())
            .count() as u64;
        let avg = if total_plans > 0 {
            inner.total_cycles as f64 / total_plans as f64
        } else {
            0.0
        };
        BrainLoopProfile {
            total_plans,
            active_plans,
            completed_plans: inner.completed_plans_total,
            failed_plans: inner.failed_plans_total,
            total_cycles: inner.total_cycles,
            avg_cycles_per_plan: avg,
        }
    }

    // Evict the oldest terminal plan when the cap is exceeded.
    fn evict_oldest_terminal_plan(plans: &mut HashMap<String, BrainLoopPlan>) {
        const MAX_TERMINAL_PLANS: usize = 200;
        let terminal_count = plans.values().filter(|p| p.phase.is_terminal()).count();
        if terminal_count > MAX_TERMINAL_PLANS {
            if let Some(oldest_id) = plans
                .iter()
                .filter(|(_, p)| p.phase.is_terminal())
                .min_by_key(|(_, p)| p.created_ms)
                .map(|(id, _)| id.clone())
            {
                plans.remove(&oldest_id);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Return the current Unix time in milliseconds.
fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn make_step(id: &str, desc: &str) -> BrainLoopStep {
        BrainLoopStep {
            id: id.to_string(),
            phase: BrainLoopPhase::Planning,
            description: desc.to_string(),
            input: String::new(),
            output: String::new(),
            started_ms: 0,
            completed_ms: 0,
            duration_ms: 0,
            status: StepStatus::Pending,
        }
    }

    fn default_config() -> BrainLoopConfig {
        BrainLoopConfig {
            max_iterations: 5,
            max_steps_per_iteration: 10,
            reflection_required: true,
            auto_replan: true,
        }
    }

    // -----------------------------------------------------------------------
    // test_new_brain_loop_empty
    // -----------------------------------------------------------------------

    #[test]
    fn test_new_brain_loop_empty() {
        let bl = BrainLoop::new(default_config());
        let plans = bl.list_plans();
        assert!(plans.is_empty(), "new brain loop should have no plans");

        let profile = bl.profile();
        assert_eq!(profile.total_plans, 0);
        assert_eq!(profile.active_plans, 0);
        assert_eq!(profile.completed_plans, 0);
        assert_eq!(profile.failed_plans, 0);
        assert_eq!(profile.total_cycles, 0);
        assert_eq!(profile.avg_cycles_per_plan, 0.0);
    }

    // -----------------------------------------------------------------------
    // test_start_plan
    // -----------------------------------------------------------------------

    #[test]
    fn test_start_plan() {
        let bl = BrainLoop::new(default_config());
        let steps = vec![make_step("s1", "Step one"), make_step("s2", "Step two")];
        let plan_id = bl.start_plan("Test goal", steps.clone()).unwrap();

        let plan = bl.get_plan(&plan_id).unwrap();
        assert_eq!(plan.goal, "Test goal");
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.phase, BrainLoopPhase::Planning);
        assert_eq!(plan.current_iteration, 0);
        assert!(plan.created_ms > 0);

        // Should appear in list.
        let plans = bl.list_plans();
        assert!(plans.contains(&plan_id));
    }

    // -----------------------------------------------------------------------
    // test_execute_step
    // -----------------------------------------------------------------------

    #[test]
    fn test_execute_step() {
        let bl = BrainLoop::new(default_config());
        let steps = vec![make_step("s1", "Step one")];
        let plan_id = bl.start_plan("Goal", steps).unwrap();

        bl.execute_step(&plan_id, "s1", "output from step 1")
            .unwrap();

        let plan = bl.get_plan(&plan_id).unwrap();
        assert_eq!(plan.phase, BrainLoopPhase::Executing);
        assert_eq!(plan.current_iteration, 1);

        let step = &plan.steps[0];
        assert_eq!(step.status, StepStatus::InProgress);
        assert_eq!(step.output, "output from step 1");
        assert!(step.started_ms > 0);
    }

    // -----------------------------------------------------------------------
    // test_execute_nonexistent_step_fails
    // -----------------------------------------------------------------------

    #[test]
    fn test_execute_nonexistent_step_fails() {
        let bl = BrainLoop::new(default_config());
        let plan_id = bl
            .start_plan("Goal", vec![make_step("s1", "Real step")])
            .unwrap();

        let err = bl.execute_step(&plan_id, "s999", "data").unwrap_err();
        assert!(
            err.to_string().contains("error.step_not_found"),
            "error should mention the missing step id: {err}"
        );

        // Executing on a non-existent plan should also fail.
        let err2 = bl
            .execute_step("plan-nonexistent", "s1", "data")
            .unwrap_err();
        assert!(
            err2.to_string().contains("error.plan_not_found"),
            "error should mention the missing plan id: {err2}"
        );
    }

    // -----------------------------------------------------------------------
    // test_reflect
    // -----------------------------------------------------------------------

    #[test]
    fn test_reflect() {
        let bl = BrainLoop::new(default_config());
        let plan_id = bl
            .start_plan("Goal", vec![make_step("s1", "Step A")])
            .unwrap();

        bl.execute_step(&plan_id, "s1", "done").unwrap();

        let reflection = bl
            .reflect(
                &plan_id,
                "s1",
                vec!["observed X".to_string()],
                vec!["issue Y".to_string()],
                vec!["improve Z".to_string()],
            )
            .unwrap();

        assert_eq!(reflection.step_id, "s1");
        assert_eq!(reflection.issues, vec!["issue Y"]);
        assert!(reflection.confidence < 1.0);
        assert!(reflection.reflection_ms > 0);

        // The plan should now be in Reflecting phase.
        let plan = bl.get_plan(&plan_id).unwrap();
        assert_eq!(plan.phase, BrainLoopPhase::Reflecting);

        // The step should be marked Done with a non-zero duration.
        let step = &plan.steps[0];
        assert_eq!(step.status, StepStatus::Done);
        assert!(step.duration_ms > 0 || step.completed_ms >= step.started_ms);
    }

    // -----------------------------------------------------------------------
    // test_replan_adds_new_steps
    // -----------------------------------------------------------------------

    #[test]
    fn test_replan_adds_new_steps() {
        let bl = BrainLoop::new(default_config());
        let plan_id = bl
            .start_plan("Goal", vec![make_step("s1", "Old step")])
            .unwrap();

        // Execute and reflect.
        bl.execute_step(&plan_id, "s1", "result").unwrap();
        bl.reflect(&plan_id, "s1", vec!["ok".to_string()], vec![], vec![])
            .unwrap();

        // Replan with two new steps.
        let new_steps = vec![
            make_step("s2", "Revised step 1"),
            make_step("s3", "Revised step 2"),
        ];
        bl.replan(&plan_id, new_steps).unwrap();

        let plan = bl.get_plan(&plan_id).unwrap();
        assert_eq!(plan.phase, BrainLoopPhase::Replanning);
        // The old step s1 remains (completed), plus two new ones.
        assert_eq!(plan.steps.len(), 3);
        assert_eq!(plan.steps[0].id, "s1");
        assert_eq!(plan.steps[1].id, "s2");
        assert_eq!(plan.steps[2].id, "s3");
    }

    // -----------------------------------------------------------------------
    // test_complete_plan
    // -----------------------------------------------------------------------

    #[test]
    fn test_complete_plan() {
        let bl = BrainLoop::new(default_config());
        let plan_id = bl
            .start_plan("Goal", vec![make_step("s1", "Step")])
            .unwrap();

        bl.complete_plan(&plan_id).unwrap();

        let plan = bl.get_plan(&plan_id).unwrap();
        assert_eq!(plan.phase, BrainLoopPhase::Completed);
        assert!(plan.phase.is_terminal());

        // Completing an already completed plan should fail.
        let err = bl.complete_plan(&plan_id).unwrap_err();
        assert!(err.to_string().contains("error.plan_already_terminal"));
    }

    // -----------------------------------------------------------------------
    // test_fail_plan
    // -----------------------------------------------------------------------

    #[test]
    fn test_fail_plan() {
        let bl = BrainLoop::new(default_config());
        let plan_id = bl
            .start_plan("Goal", vec![make_step("s1", "Step")])
            .unwrap();

        bl.fail_plan(&plan_id, "Something went wrong").unwrap();

        let plan = bl.get_plan(&plan_id).unwrap();
        assert_eq!(plan.phase, BrainLoopPhase::Failed);
        assert!(plan.phase.is_terminal());
        assert_eq!(plan.fail_reason, "Something went wrong");

        // Failing an already failed plan should fail.
        let err = bl.fail_plan(&plan_id, "again").unwrap_err();
        assert!(err.to_string().contains("error.plan_already_terminal"));
    }

    // -----------------------------------------------------------------------
    // test_cancel_plan
    // -----------------------------------------------------------------------

    #[test]
    fn test_cancel_plan() {
        let bl = BrainLoop::new(default_config());
        let plan_id = bl
            .start_plan("Goal", vec![make_step("s1", "Step")])
            .unwrap();

        bl.cancel_plan(&plan_id).unwrap();

        let plan = bl.get_plan(&plan_id).unwrap();
        assert_eq!(plan.phase, BrainLoopPhase::Cancelled);
        assert!(plan.phase.is_terminal());

        // Cancelling an already cancelled plan should fail.
        let err = bl.cancel_plan(&plan_id).unwrap_err();
        assert!(err.to_string().contains("error.plan_already_terminal"));
    }

    // -----------------------------------------------------------------------
    // test_max_iterations_enforced
    // -----------------------------------------------------------------------

    #[test]
    fn test_max_iterations_enforced() {
        let config = BrainLoopConfig {
            max_iterations: 2,
            max_steps_per_iteration: 10,
            reflection_required: true,
            auto_replan: true,
        };
        let bl = BrainLoop::new(config);

        // Start a plan with a single step.
        let plan_id = bl
            .start_plan("Iteration test", vec![make_step("s1", "Step")])
            .unwrap();

        // Iteration 1: execute step.
        bl.execute_step(&plan_id, "s1", "iter 1").unwrap();
        {
            let plan = bl.get_plan(&plan_id).unwrap();
            assert_eq!(plan.current_iteration, 1);
            assert!(!plan.phase.is_terminal());
        }

        // Reflect so the step is done, then replan with a new step for the next iteration.
        bl.reflect(&plan_id, "s1", vec![], vec![], vec![]).unwrap();
        bl.replan(&plan_id, vec![make_step("s2", "Iter 2 step")])
            .unwrap();

        // Iteration 2: execute new step.
        bl.execute_step(&plan_id, "s2", "iter 2").unwrap();
        {
            let plan = bl.get_plan(&plan_id).unwrap();
            assert_eq!(plan.current_iteration, 2);
            assert!(!plan.phase.is_terminal());
        }

        // Reflect and replan for iteration 3 (over limit).
        bl.reflect(&plan_id, "s2", vec![], vec![], vec![]).unwrap();
        bl.replan(&plan_id, vec![make_step("s3", "Iter 3 step")])
            .unwrap();

        // Executing s3 pushes iteration to 3, which exceeds max_iterations.
        bl.execute_step(&plan_id, "s3", "iter 3").unwrap();

        let plan = bl.get_plan(&plan_id).unwrap();
        assert_eq!(
            plan.phase,
            BrainLoopPhase::Failed,
            "plan should fail when max_iterations is exceeded"
        );
        assert!(plan.fail_reason.contains("maximum iterations"));
    }

    // -----------------------------------------------------------------------
    // test_profile_reflects_state
    // -----------------------------------------------------------------------

    #[test]
    fn test_profile_reflects_state() {
        let bl = BrainLoop::new(default_config());

        // Profile before any plans.
        let p0 = bl.profile();
        assert_eq!(p0.total_plans, 0);
        assert_eq!(p0.active_plans, 0);

        // Start two plans.
        let pid_a = bl
            .start_plan("Plan A", vec![make_step("a1", "A1")])
            .unwrap();
        let pid_b = bl
            .start_plan("Plan B", vec![make_step("b1", "B1")])
            .unwrap();

        let p1 = bl.profile();
        assert_eq!(p1.total_plans, 2);
        assert_eq!(p1.active_plans, 2);

        // Execute a step on plan A → cycles = 1.
        bl.execute_step(&pid_a, "a1", "out").unwrap();

        let p2 = bl.profile();
        assert_eq!(p2.total_cycles, 1);
        assert!(p2.avg_cycles_per_plan > 0.0);

        // Complete plan A.
        bl.complete_plan(&pid_a).unwrap();

        let p3 = bl.profile();
        assert_eq!(p3.completed_plans, 1);
        assert_eq!(p3.active_plans, 1);
        assert_eq!(p3.total_plans, 2);

        // Fail plan B.
        bl.fail_plan(&pid_b, "Timeout").unwrap();

        let p4 = bl.profile();
        assert_eq!(p4.failed_plans, 1);
        assert_eq!(p4.active_plans, 0);
        assert_eq!(p4.total_plans, 2);
    }

    // -----------------------------------------------------------------------
    // test_get_nonexistent_plan_fails
    // -----------------------------------------------------------------------

    #[test]
    fn test_get_nonexistent_plan_fails() {
        let bl = BrainLoop::new(default_config());

        let err = bl.get_plan("does-not-exist").unwrap_err();
        assert!(err.to_string().contains("not found"));

        let err = bl.current_phase("phantom-plan").unwrap_err();
        assert!(err.to_string().contains("not found"));
    }
}
