//! # Brain Loop — Plan → Execute → Reflect → Replan (F-GAP-17)
//!
//! Implements an iterative cognitive cycle that:
//!   1. **Plan** — produces a structured plan for a given task.
//!   2. **Execute** — runs the plan and captures the result.
//!   3. **Reflect** — analyses the outcome (score, issues, improvements).
//!   4. **Replan** — improves the plan based on reflection.
//!
//! The cycle repeats until the task converges (score ≥ `min_score`) or the
//! maximum number of iterations is reached.
//!
//! ## Thread safety
//!
//! [`BrainLoop`] holds its mutable state behind `Arc<Mutex<…>>` so it can be
//! cloned and shared across threads safely.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// The phase of the brain loop at a given point in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum BrainLoopState {
    #[default]
    Planning,
    Executing,
    Reflecting,
    Replanning,
    Completed,
    Failed,
}

impl BrainLoopState {
    /// Returns `true` for terminal (non-recoverable) states.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

// ---------------------------------------------------------------------------
// Core data structures
// ---------------------------------------------------------------------------

/// A single atomic step in the brain loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainLoopStep {
    pub id: String,
    pub phase: BrainLoopState,
    pub input: String,
    pub output: Option<String>,
    pub reflection: Option<String>,
    pub score: Option<f64>,
    pub created_ms: u64,
    pub duration_ms: u64,
}

/// Configuration that tunes the behaviour of the brain loop.
#[derive(Debug, Clone)]
pub struct BrainLoopConfig {
    /// Maximum number of Plan→Execute→Reflect→Replan iterations.
    /// Default: `5`
    pub max_iterations: u32,
    /// Minimum score required to consider a task converged (0.0 – 1.0).
    /// Default: `0.7`
    pub min_score: f64,
    /// If the score difference between two consecutive reflections is
    /// below this threshold, the system considers the loop converged.
    /// Default: `0.05`
    pub convergence_threshold: f64,
}

impl Default for BrainLoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 5,
            min_score: 0.7,
            convergence_threshold: 0.05,
        }
    }
}

/// Reflection produced after analysing a plan + result pair.
#[derive(Debug, Clone)]
pub struct Reflection {
    pub score: f64,
    pub issues: Vec<String>,
    pub improvements: Vec<String>,
    pub converged: bool,
}

/// Summary report returned by [`BrainLoop::run`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainLoopReport {
    pub iterations: usize,
    pub final_score: f64,
    pub converged: bool,
    pub history: Vec<BrainLoopStep>,
}

/// Runtime metrics snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BrainLoopProfile {
    pub state: BrainLoopState,
    pub iteration_count: u32,
    pub total_steps: u64,
    pub avg_score: f64,
    pub convergence_info: String,
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

struct BrainLoopInner {
    config: BrainLoopConfig,
    state: BrainLoopState,
    iteration_count: u32,
    steps: Vec<BrainLoopStep>,
    previous_score: Option<f64>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// The brain loop orchestrator.
///
/// All mutable state is behind `Arc<Mutex<…>>` so the struct can be
/// cloned and shared across threads.
#[derive(Clone)]
pub struct BrainLoop {
    inner: Arc<Mutex<BrainLoopInner>>,
    next_step_id: Arc<AtomicU64>,
}

impl BrainLoop {
    /// Create a new brain loop with the given configuration.
    pub fn new(config: BrainLoopConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BrainLoopInner {
                config,
                state: BrainLoopState::Planning,
                iteration_count: 0,
                steps: Vec::new(),
                previous_score: None,
            })),
            next_step_id: Arc::new(AtomicU64::new(1)),
        }
    }

    // ── Plan ──────────────────────────────────────────────────────────────

    /// Produce a plan for the given `task`.
    ///
    /// The current implementation uses simple string-based logic. A production
    /// system would delegate to an LLM or planner service.
    pub fn plan(&self, task: &str) -> anyhow::Result<String> {
        let mut inner = self.inner.lock().unwrap();
        inner.state = BrainLoopState::Planning;
        inner.iteration_count += 1;

        let step_id = self.next_step_id.fetch_add(1, Ordering::AcqRel);
        let now = now_epoch_ms();
        let step = BrainLoopStep {
            id: format!("plan-{step_id}"),
            phase: BrainLoopState::Planning,
            input: task.to_string(),
            output: None,
            reflection: None,
            score: None,
            created_ms: now,
            duration_ms: 0,
        };
        inner.steps.push(step);

        // Simple string-based planning logic.
        let plan = format!(
            "Plan for '{}' (iteration {}):\n\
             ─────────────────────────────\n\
             1. Analyse the requirements\n\
             2. Design a solution\n\
             3. Implement the solution\n\
             4. Verify correctness\n\
             5. Review and improve",
            task, inner.iteration_count
        );

        // Update the step output.
        if let Some(last) = inner.steps.last_mut() {
            last.output = Some(plan.clone());
            last.duration_ms = now_epoch_ms().saturating_sub(now);
        }

        Ok(plan)
    }

    // ── Execute ───────────────────────────────────────────────────────────

    /// Execute the given `plan` and return the result.
    ///
    /// The current implementation simulates execution by producing a
    /// deterministic result string. A production system would actually
    /// run the plan steps.
    pub fn execute(&self, plan: &str) -> anyhow::Result<String> {
        let mut inner = self.inner.lock().unwrap();
        inner.state = BrainLoopState::Executing;

        let step_id = self.next_step_id.fetch_add(1, Ordering::AcqRel);
        let now = now_epoch_ms();
        let step = BrainLoopStep {
            id: format!("exec-{step_id}"),
            phase: BrainLoopState::Executing,
            input: plan.to_string(),
            output: None,
            reflection: None,
            score: None,
            created_ms: now,
            duration_ms: 0,
        };
        inner.steps.push(step);

        // Simulate execution result.
        let result = "Execution result for plan:\n\
             ─────────────────────────\n\
             - All steps completed successfully.\n\
             - Output meets the stated requirements.\n\
             - No critical errors detected."
            .to_string();

        if let Some(last) = inner.steps.last_mut() {
            last.output = Some(result.clone());
            last.duration_ms = now_epoch_ms().saturating_sub(now);
        }

        Ok(result)
    }

    // ── Reflect ───────────────────────────────────────────────────────────

    /// Analyse the outcome of executing a plan.
    ///
    /// Returns a [`Reflection`] that contains a score, identified issues,
    /// suggested improvements, and a convergence flag.
    pub fn reflect(&self, plan: &str, result: &str) -> anyhow::Result<Reflection> {
        let mut inner = self.inner.lock().unwrap();
        inner.state = BrainLoopState::Reflecting;

        let step_id = self.next_step_id.fetch_add(1, Ordering::AcqRel);
        let now = now_epoch_ms();

        // Simple heuristic scoring based on the plan/result length.
        let plan_len = plan.len() as f64;
        let result_len = result.len() as f64;
        let raw_score = if plan_len > 0.0 {
            (result_len / plan_len).min(1.5) / 1.5
        } else {
            0.5
        };
        let score = raw_score.clamp(0.0, 1.0);

        // Detect issues / improvements from the result content.
        let mut issues: Vec<String> = Vec::new();
        let mut improvements: Vec<String> = Vec::new();

        if result.contains("error") || result.contains("Error") {
            issues.push("Errors detected in execution result.".to_string());
            improvements.push("Investigate and fix reported errors.".to_string());
        }
        if result.len() < 20 {
            issues.push("Result is too short — may be incomplete.".to_string());
            improvements.push("Provide more detailed output.".to_string());
        }
        if score < 0.4 {
            issues.push("Low score — plan may be misaligned with task.".to_string());
            improvements.push("Reassess the plan structure and try again.".to_string());
        }

        // If no specific issues were found, add a generic positive note.
        if issues.is_empty() {
            improvements.push("Continue with the current approach.".to_string());
        }

        // Detect convergence.
        let converged = self.check_convergence(&inner, score);

        let reflection = Reflection {
            score,
            issues,
            improvements,
            converged,
        };

        let reflection_str = format!(
            "Score: {:.3}, issues: {}, improvements: {}, converged: {}",
            reflection.score,
            reflection.issues.len(),
            reflection.improvements.len(),
            reflection.converged
        );

        let step = BrainLoopStep {
            id: format!("refl-{step_id}"),
            phase: BrainLoopState::Reflecting,
            input: format!("plan:\n{plan}\n\nresult:\n{result}"),
            output: None,
            reflection: Some(reflection_str),
            score: Some(score),
            created_ms: now,
            duration_ms: now_epoch_ms().saturating_sub(now),
        };
        inner.steps.push(step);
        inner.previous_score = Some(score);

        Ok(reflection)
    }

    // ── Replan ────────────────────────────────────────────────────────────

    /// Produce an improved plan based on the reflection and the original plan.
    pub fn replan(&self, reflection: &Reflection, original_plan: &str) -> anyhow::Result<String> {
        let mut inner = self.inner.lock().unwrap();
        inner.state = BrainLoopState::Replanning;

        let step_id = self.next_step_id.fetch_add(1, Ordering::AcqRel);
        let now = now_epoch_ms();

        // Build an improved plan incorporating reflection feedback.
        let mut plan = format!(
            "Revised Plan (based on reflection):\n\
             ────────────────────────────────────\n\
             Original score: {:.3}\n\
             Issues addressed:\n",
            reflection.score
        );

        for issue in &reflection.issues {
            plan.push_str(&format!("  ! {issue}\n"));
        }

        plan.push_str("\nImprovements incorporated:\n");
        for improvement in &reflection.improvements {
            plan.push_str(&format!("  + {improvement}\n"));
        }

        if reflection.converged {
            plan.push_str("\n[Converged — no further changes needed]\n");
        } else {
            plan.push_str(&format!(
                "\nNext iteration aims to raise score from {:.3} to ≥ {:.3}\n",
                reflection.score, inner.config.min_score,
            ));
        }

        let step = BrainLoopStep {
            id: format!("replan-{step_id}"),
            phase: BrainLoopState::Replanning,
            input: original_plan.to_string(),
            output: Some(plan.clone()),
            reflection: None,
            score: Some(reflection.score),
            created_ms: now,
            duration_ms: now_epoch_ms().saturating_sub(now),
        };
        inner.steps.push(step);

        Ok(plan)
    }

    // ── Full loop ─────────────────────────────────────────────────────────

    /// Run the full Plan → Execute → Reflect → Replan cycle.
    ///
    /// The loop continues until either:
    /// - Convergence is detected (score ≥ `min_score` or score stabilises).
    /// - The maximum number of iterations is reached.
    ///
    /// Returns a [`BrainLoopReport`] summarising the run.
    pub fn run(&self, task: &str) -> anyhow::Result<BrainLoopReport> {
        let config = {
            let inner = self.inner.lock().unwrap();
            inner.config.clone()
        };

        let mut iterations = 0usize;
        let mut final_score = 0.0;
        let mut converged = false;

        let mut plan = self.plan(task)?;

        for iter in 0..config.max_iterations as usize {
            iterations = iter + 1;

            // Execute.
            let result = self.execute(&plan)?;

            // Reflect.
            let reflection = self.reflect(&plan, &result)?;
            final_score = reflection.score;

            if reflection.converged {
                converged = true;
                break;
            }

            // Replan (if score is still below min_score).
            if reflection.score < config.min_score {
                plan = self.replan(&reflection, &plan)?;
            } else {
                converged = true;
                break;
            }
        }

        // Mark the final state.
        {
            let mut inner = self.inner.lock().unwrap();
            if converged {
                inner.state = BrainLoopState::Completed;
            } else {
                inner.state = BrainLoopState::Failed;
            }
        }

        let history = {
            let inner = self.inner.lock().unwrap();
            inner.steps.clone()
        };

        // Verify terminal state is set correctly.
        let state = {
            let inner = self.inner.lock().unwrap();
            inner.state
        };
        debug_assert!(state.is_terminal(), "state must be terminal after run");

        Ok(BrainLoopReport {
            iterations,
            final_score,
            converged,
            history,
        })
    }

    // ── Queries ───────────────────────────────────────────────────────────

    /// Return a snapshot of the current runtime profile.
    pub fn profile(&self) -> BrainLoopProfile {
        let inner = self.inner.lock().unwrap();
        let total_steps = inner.steps.len() as u64;

        let scores: Vec<f64> = inner.steps.iter().filter_map(|s| s.score).collect();
        let avg_score = if scores.is_empty() {
            0.0
        } else {
            scores.iter().sum::<f64>() / scores.len() as f64
        };

        // Summarise the last completed step for diagnostic info.
        let last_step_info: String = inner
            .steps
            .last()
            .map(|s| {
                let reflection_note = s.reflection.as_deref().unwrap_or("-");
                format!(
                    "step {} [phase={:?}, input_len={}, created_ms={}, reflection={}]",
                    s.id,
                    s.phase,
                    s.input.len(),
                    s.created_ms,
                    reflection_note
                )
            })
            .unwrap_or_default();

        let convergence_info = match inner.state {
            BrainLoopState::Completed => {
                format!(
                    "Converged after {} iterations (avg score: {:.3}); last: {}",
                    inner.iteration_count, avg_score, last_step_info
                )
            }
            BrainLoopState::Failed => {
                format!(
                    "Failed after {} iterations (avg score: {:.3}, min required: {:.3}); last: {}",
                    inner.iteration_count, avg_score, inner.config.min_score, last_step_info
                )
            }
            BrainLoopState::Planning => format!("Planning phase; last: {}", last_step_info),
            BrainLoopState::Executing => format!("Executing phase; last: {}", last_step_info),
            BrainLoopState::Reflecting => format!("Reflecting phase; last: {}", last_step_info),
            BrainLoopState::Replanning => format!("Replanning phase; last: {}", last_step_info),
        };

        BrainLoopProfile {
            state: inner.state,
            iteration_count: inner.iteration_count,
            total_steps,
            avg_score,
            convergence_info,
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────────

    /// Check whether the loop has converged.
    ///
    /// Convergence is detected when either:
    /// - The current score meets or exceeds `min_score`.
    /// - The score difference from the previous reflection is below
    ///   `convergence_threshold`.
    fn check_convergence(&self, inner: &BrainLoopInner, current_score: f64) -> bool {
        // Score meets the target.
        if current_score >= inner.config.min_score {
            return true;
        }

        // Score has stabilised (no significant improvement).
        if let Some(prev) = inner.previous_score {
            if (current_score - prev).abs() < inner.config.convergence_threshold {
                return true;
            }
        }

        false
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

    fn default_config() -> BrainLoopConfig {
        BrainLoopConfig {
            max_iterations: 5,
            min_score: 0.7,
            convergence_threshold: 0.05,
        }
    }

    // -----------------------------------------------------------------------
    // test_new_loop_empty
    // -----------------------------------------------------------------------

    #[test]
    fn test_new_loop_empty() {
        let bl = BrainLoop::new(default_config());

        let profile = bl.profile();
        assert_eq!(profile.state, BrainLoopState::Planning);
        assert_eq!(profile.iteration_count, 0);
        assert_eq!(profile.total_steps, 0);
        assert_eq!(profile.avg_score, 0.0);
        assert!(profile.convergence_info.contains("Planning"));
    }

    // -----------------------------------------------------------------------
    // test_plan_creates_output
    // -----------------------------------------------------------------------

    #[test]
    fn test_plan_creates_output() {
        let bl = BrainLoop::new(default_config());
        let plan = bl.plan("Write a sorting algorithm").unwrap();

        assert!(!plan.is_empty(), "plan should produce non-empty output");
        assert!(
            plan.contains("Plan for"),
            "plan should contain the task description"
        );
        assert!(
            plan.contains("iteration"),
            "plan should track iteration count"
        );

        let profile = bl.profile();
        assert_eq!(profile.iteration_count, 1);
        assert_eq!(profile.total_steps, 1);
    }

    // -----------------------------------------------------------------------
    // test_execute_produces_result
    // -----------------------------------------------------------------------

    #[test]
    fn test_execute_produces_result() {
        let bl = BrainLoop::new(default_config());
        let plan = bl.plan("Test task").unwrap();
        let result = bl.execute(&plan).unwrap();

        assert!(!result.is_empty(), "execution should produce a result");
        assert!(
            result.contains("completed successfully"),
            "result should indicate success"
        );

        let profile = bl.profile();
        assert_eq!(profile.total_steps, 2); // plan + execute
    }

    // -----------------------------------------------------------------------
    // test_reflect_analyzes_outcome
    // -----------------------------------------------------------------------

    #[test]
    fn test_reflect_analyzes_outcome() {
        let bl = BrainLoop::new(default_config());
        let plan = bl.plan("Test reflection").unwrap();
        let result = bl.execute(&plan).unwrap();
        let reflection = bl.reflect(&plan, &result).unwrap();

        assert!(
            (0.0..=1.0).contains(&reflection.score),
            "score should be between 0 and 1, got {}",
            reflection.score
        );
        assert!(
            !reflection.improvements.is_empty(),
            "reflection should produce improvements"
        );
        assert!(
            !reflection.converged || reflection.score >= 0.7,
            "converged requires score >= min_score"
        );

        let profile = bl.profile();
        assert_eq!(profile.state, BrainLoopState::Reflecting);
    }

    // -----------------------------------------------------------------------
    // test_replan_improves_plan
    // -----------------------------------------------------------------------

    #[test]
    fn test_replan_improves_plan() {
        let bl = BrainLoop::new(default_config());
        let original_plan = bl.plan("Improve this").unwrap();
        let result = bl.execute(&original_plan).unwrap();
        let reflection = bl.reflect(&original_plan, &result).unwrap();
        let revised_plan = bl.replan(&reflection, &original_plan).unwrap();

        assert!(!revised_plan.is_empty(), "revised plan should not be empty");
        assert_ne!(
            revised_plan, original_plan,
            "revised plan should differ from original"
        );
        assert!(
            revised_plan.contains("Revised Plan"),
            "revised plan should indicate it is a revision"
        );

        let profile = bl.profile();
        assert_eq!(profile.state, BrainLoopState::Replanning);
    }

    // -----------------------------------------------------------------------
    // test_run_full_loop_converges
    // -----------------------------------------------------------------------

    #[test]
    fn test_run_full_loop_converges() {
        // Use a config with a low min_score so the loop converges quickly.
        let config = BrainLoopConfig {
            max_iterations: 10,
            min_score: 0.3,
            convergence_threshold: 0.05,
        };
        let bl = BrainLoop::new(config);
        let report = bl.run("Simple task").unwrap();

        assert!(report.converged, "loop should converge with low min_score");
        assert!(
            report.final_score >= 0.0,
            "final score should be non-negative"
        );
        assert!(report.iterations >= 1, "should have at least 1 iteration");
        assert!(
            !report.history.is_empty(),
            "report should contain step history"
        );

        let profile = bl.profile();
        assert_eq!(
            profile.state,
            BrainLoopState::Completed,
            "profile should indicate completion"
        );
    }

    // -----------------------------------------------------------------------
    // test_run_full_loop_fails_on_low_score
    // -----------------------------------------------------------------------

    #[test]
    fn test_run_full_loop_fails_on_low_score() {
        // Use a high min_score that the simple heuristic can't reach.
        let config = BrainLoopConfig {
            max_iterations: 3,
            min_score: 0.99,
            convergence_threshold: 0.001,
        };
        let bl = BrainLoop::new(config);
        let report = bl.run("Impossible task").unwrap();

        assert!(
            !report.converged,
            "loop should NOT converge with unattainable min_score"
        );
        assert_eq!(
            report.iterations, 3,
            "should exhaust max_iterations without converging"
        );

        let profile = bl.profile();
        assert_eq!(
            profile.state,
            BrainLoopState::Failed,
            "profile should indicate failure"
        );
    }

    // -----------------------------------------------------------------------
    // test_profile_reflects_state
    // -----------------------------------------------------------------------

    #[test]
    fn test_profile_reflects_state() {
        let bl = BrainLoop::new(default_config());

        // Before: Planning (idle).
        let p0 = bl.profile();
        assert_eq!(p0.state, BrainLoopState::Planning);
        assert_eq!(p0.iteration_count, 0);

        // After plan.
        let _ = bl.plan("Task");
        let p1 = bl.profile();
        assert_eq!(p1.state, BrainLoopState::Planning);
        assert_eq!(p1.iteration_count, 1);
        assert_eq!(p1.total_steps, 1);

        // After execute.
        let plan = bl.plan("Task 2").unwrap();
        let _ = bl.execute(&plan);
        let p2 = bl.profile();
        assert_eq!(p2.state, BrainLoopState::Executing);
        assert_eq!(p2.total_steps, 3); // plan + plan2 + exec
    }

    // -----------------------------------------------------------------------
    // test_config_defaults
    // -----------------------------------------------------------------------

    #[test]
    fn test_config_defaults() {
        let config = BrainLoopConfig::default();
        assert_eq!(config.max_iterations, 5);
        assert_eq!(config.min_score, 0.7);
        assert!((config.convergence_threshold - 0.05).abs() < f64::EPSILON);

        // Verify you can create a BrainLoop with default config.
        let bl = BrainLoop::new(BrainLoopConfig::default());
        let profile = bl.profile();
        assert_eq!(profile.state, BrainLoopState::Planning);
    }

    // -----------------------------------------------------------------------
    // test_convergence_detection
    // -----------------------------------------------------------------------

    #[test]
    fn test_convergence_detection() {
        // Convergence via score threshold.
        let config = BrainLoopConfig {
            max_iterations: 10,
            min_score: 0.3,
            convergence_threshold: 0.05,
        };
        let bl = BrainLoop::new(config);
        let report = bl.run("Converge by score").unwrap();
        assert!(report.converged, "should converge by exceeding min_score");
        assert!(
            report.final_score >= 0.3,
            "final score should meet min_score threshold"
        );
        assert!(report.iterations <= 10, "should converge before max");

        // Convergence via stability (the run method replans only when score < min_score,
        // and after replanning the loop continues. With our heuristic, scores from
        // successive iterations with similar-length plans/results should converge).
        let config2 = BrainLoopConfig {
            max_iterations: 3,
            min_score: 0.5,             // reachable by the heuristic
            convergence_threshold: 0.5, // very wide — will trigger stability convergence
        };
        let bl2 = BrainLoop::new(config2);
        let report2 = bl2.run("Stable task").unwrap();
        assert!(
            report2.converged,
            "should converge via stability (wide threshold)"
        );

        // No convergence when score never reaches min and threshold is tiny.
        let config3 = BrainLoopConfig {
            max_iterations: 2,
            min_score: 0.99,             // unreachable
            convergence_threshold: 1e-9, // essentially never stable
        };
        let bl3 = BrainLoop::new(config3);
        let report3 = bl3.run("No convergence").unwrap();
        assert!(
            !report3.converged,
            "should NOT converge when score is too low and no stability"
        );
        assert_eq!(report3.iterations, 2, "should exhaust all iterations");
    }
}
