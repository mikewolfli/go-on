//! # Brain Loop — Plan → Execute → Reflect → Replan
//!
//! ⚠️ **DEPRECATED** (non-test): Use cognitive loop in chat_phases.rs instead.
//! Test code is excluded to avoid false positives from the test harness.
//!
//! Implements FUTURE5.MD M5 "脑回路（Plan→Execute→Reflect→Replan）",
//! an iterative orchestration cycle that drives a plan forward by executing
//! individual steps, reflecting on the outcome, and optionally replanning
//! the remaining work.  The loop continues until the plan completes, fails,
//! is cancelled, or reaches the configured maximum number of iterations.
//!
//! ⚠️ **RETENTION NOTE**: This module (~1700 lines) is currently **held back**
//!    as a future-extension. The Plan→Execute→Reflect→Replan loop requires
//!    the ACP autonomy runtime (`autonomy_runtime.rs`) and the DAG executor
//!    (`dag_executor.rs`) to be stabilized first. Once those components are
//!    production-ready, the BrainLoop should be wired into `process_chat_request`
//!    as a post-fallback reflection stage — after the agent responds, BrainLoop
//!    evaluates the result, replans if needed, and feeds back into execution.
//!
//! ## Wiring TODO (when activated)
//!
//! 1. In `process_chat_request` (chat.rs), after the agent selection & execution
//!    pipeline completes, call `BrainLoop::new(…)` with the response context.
//! 2. Use `BrainLoop::execute_step()` to run a single plan→execute→reflect cycle.
//! 3. Wire `BrainLoop::is_complete()` to skip further iteration when the goal is met.
//! 4. Connect `ProgressReporter` to SSE stream for real-time loop status.
//!
//! ## Thread safety
//!
//! The top-level [`BrainLoop`] struct holds interior mutability behind
//! `Arc<RwLock<…>>` so it can be shared across tasks.  Reads and writes
//! use `tokio::sync::RwLock` for async-safe concurrency.  Individual
//! snapshot types (`BrainLoopPlan`, `BrainLoopStep`, …) derive `Clone`
//! so callers obtain a consistent view without holding the lock.
//!
//! # Architecture
//!
//! This module is split into sub-modules for clarity:
//!
//! - [`planning`] — plan lifecycle, reflection, replanning, persistence
//! - [`execution`] — step execution with and without context
//! - [`reflection`] — [`DeepReasoningEngine`], report types

// ---------------------------------------------------------------------------
// Sub-modules
// ---------------------------------------------------------------------------

pub mod execution;
pub mod planning;
pub mod reflection;

#[allow(unused_imports)]
pub use reflection::{BrainLoopReport, DeepReasoningEngine, Reflection};

// ---------------------------------------------------------------------------
// Imports
// ---------------------------------------------------------------------------

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use tokio::sync::RwLock;
use tokio::task::JoinHandle;

use crate::agent::AgentRegistry;
use crate::agents::progress_reporter::ProgressReporter;
use crate::intelligence::metacognitive::MetacognitiveController;
use crate::orchestration::core_dag::TaskContext;

use std::time::{SystemTime, UNIX_EPOCH};

use crate::i18n::runtime::tf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// RwLock guard helpers
// ---------------------------------------------------------------------------

/// Acquire a read guard on the inner RwLock.
pub(crate) async fn read_guard<T>(rw: &RwLock<T>) -> tokio::sync::RwLockReadGuard<'_, T> {
    rw.read().await
}

/// Acquire a write guard on the inner RwLock.
pub(crate) async fn write_guard<T>(rw: &RwLock<T>) -> tokio::sync::RwLockWriteGuard<'_, T> {
    rw.write().await
}

// ---------------------------------------------------------------------------
// Public data types
// ---------------------------------------------------------------------------

/// The phase a plan is currently in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrainLoopPhase {
    Planning,
    Executing,
    Reflecting,
    Replanning,
    /// Deep-reasoning mode — the loop performs additional analysis
    /// before proceeding.  Prepared for GAP-B50-06.
    DeepReasoning,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Chain-of-Thought context associated with this step.
    pub context: Option<TaskContext>,
}

/// A plan being tracked by the brain loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainLoopPlan {
    pub id: String,
    pub goal: String,
    pub steps: Vec<BrainLoopStep>,
    pub max_iterations: u32,
    pub current_iteration: u32,
    pub created_ms: u64,
    pub phase: BrainLoopPhase,
    pub fail_reason: String,
    /// Deep-reasoning chain produced by the [`DeepReasoningEngine`]
    /// when `enable_deep_reasoning` is true (GAP-B50-06).
    pub reasoning: Option<String>,
    /// World-model entity data queried during planning when
    /// `world_model_integration` is true (GAP-B50-06).
    pub world_model_data: Option<HashMap<String, Value>>,
}

/// A hint produced by the metacognitive feedback loop, carrying preventive
/// measures or warnings for the planner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannerHint {
    /// Category of the hint: "Warning", "Info", or "Blocking".
    pub hint_type: String,
    /// Human-readable message describing the hint.
    pub message: String,
    /// Source component that produced the hint,
    /// e.g. "metacognitive", "world_model".
    pub source: String,
    /// Preventive measures recommended to avoid recurrence of the issue.
    pub preventive_measures: Vec<String>,
}

/// Reflection data recorded after executing a step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainLoopReflection {
    pub step_id: String,
    pub observations: Vec<String>,
    pub issues: Vec<String>,
    pub improvements: Vec<String>,
    pub confidence: f64,
    pub reflection_ms: u64,
    /// Snapshot of the TaskContext at reflection time.
    pub context_snapshot: Option<TaskContext>,
    /// Reasoning chain gathered from upstream contexts.
    pub reasoning_chain: Vec<String>,
}

/// Configuration that tunes the behaviour of a [`BrainLoop`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainLoopConfig {
    pub max_iterations: u32,
    pub max_steps_per_iteration: u32,
    pub reflection_required: bool,
    pub auto_replan: bool,
    /// Minimum score required to consider a task converged (0.0 – 1.0).
    /// Default: `0.7`
    pub min_score: f64,
    /// If the score difference between two consecutive reflections is
    /// below this threshold, the system considers the loop converged.
    /// Default: `0.05`
    pub convergence_threshold: f64,
    /// Optional directory for persisting plans as JSON files.
    pub plans_directory: Option<PathBuf>,
    /// Enable deep-reasoning mode (GAP-B50-06).
    /// When `true`, the loop may enter the `DeepReasoning` phase
    /// for additional analysis before completing a plan.
    /// Default: `false`
    pub enable_deep_reasoning: bool,
    /// Maximum tokens allowed for a deep-reasoning chain (GAP-B50-06).
    /// Only used when `enable_deep_reasoning` is true.
    /// Default: `4096`
    pub max_deep_reasoning_tokens: usize,
    /// Optional model name override for deep-reasoning calls (GAP-B50-06).
    /// When `None`, the default planner model is used.
    pub deep_reasoning_model: Option<String>,
    /// Whether to query the world model for environment entities during
    /// planning (GAP-B50-06).
    /// Default: `true`
    pub world_model_integration: bool,
    /// Maximum time (in milliseconds) that `sync_write`/`sync_read` will
    /// spin-wait before panicking. This bounds the worst-case wait when
    /// the async holder is stalled.
    /// Default: `5000` (5 seconds)
    pub max_spin_ms: u64,
}

impl Default for BrainLoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 5,
            max_steps_per_iteration: 10,
            reflection_required: true,
            auto_replan: true,
            min_score: 0.7,
            convergence_threshold: 0.05,
            plans_directory: None,
            enable_deep_reasoning: false,
            max_deep_reasoning_tokens: 4096,
            deep_reasoning_model: None,
            world_model_integration: true,
            max_spin_ms: 5000,
        }
    }
}

// ---------------------------------------------------------------------------
// Profile / Report types (kept for backward compatibility)
// ---------------------------------------------------------------------------

/// Runtime metrics snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BrainLoopProfile {
    pub total_plans: u64,
    pub active_plans: u64,
    pub completed_plans: u64,
    pub failed_plans: u64,
    pub total_cycles: u64,
    pub avg_cycles_per_plan: f64,
    /// Convergence status info (e.g. "converged after 3 iterations", "not converged").
    pub convergence_info: String,
    /// Average step score across all plans (0.0 – 1.0).
    pub avg_step_score: f64,
    /// Total steps across all plans.
    pub total_steps: u64,
}

// ---------------------------------------------------------------------------
// Internal runtime state
// ---------------------------------------------------------------------------

pub(crate) struct BrainLoopInner {
    pub(crate) plans: HashMap<String, BrainLoopPlan>,
    pub(crate) reflections: Vec<BrainLoopReflection>,
    pub(crate) config: BrainLoopConfig,
    pub(crate) total_cycles: u64,
    pub(crate) total_plans_started: u64,
    pub(crate) completed_plans_total: u64,
    pub(crate) failed_plans_total: u64,
    pub(crate) cancelled_plans_total: u64,
    /// Optional progress reporter for streaming status hints.
    pub(crate) progress_reporter: Option<ProgressReporter>,
    /// Running async tasks spawned by the brain loop, keyed by plan id.
    /// Reserved for GAP-B50-06 deep-reasoning integration.
    #[allow(dead_code)]
    pub(crate) brain_loop_tasks: HashMap<String, JoinHandle<()>>,
    /// Optional metacognitive controller for self-correction feedback.
    pub(crate) metacognitive: Option<MetacognitiveController>,
    /// Planner hints accumulated during loop execution.
    pub(crate) planner_hints: Vec<PlannerHint>,
    /// Tracks per-error-type occurrence counts for detecting repeated
    /// failures (3+ → PlannerHint warning).
    pub(crate) error_counts: HashMap<String, u32>,
    /// B51-08: Optional agent registry for LLM-backed deep reasoning.
    pub(crate) agent_registry: Option<Arc<AgentRegistry>>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// The brain loop orchestrator.
///
/// All mutable state lives behind `Arc<RwLock<…>>` so the struct can be
/// cloned and shared across tasks.  Read-heavy methods use a read lock;
/// mutation methods use a write lock.
#[derive(Clone)]
pub struct BrainLoop {
    pub(crate) inner: Arc<RwLock<BrainLoopInner>>,
    pub(crate) next_plan_id: Arc<AtomicU64>,
}

impl BrainLoop {
    /// Create a new brain loop with the given configuration.
    pub fn new(config: BrainLoopConfig) -> Self {
        Self {
            inner: Arc::new(RwLock::new(BrainLoopInner {
                plans: HashMap::new(),
                reflections: Vec::new(),
                config,
                total_cycles: 0,
                total_plans_started: 0,
                completed_plans_total: 0,
                failed_plans_total: 0,
                cancelled_plans_total: 0,
                progress_reporter: None,
                brain_loop_tasks: HashMap::new(),
                metacognitive: None,
                planner_hints: Vec::new(),
                error_counts: HashMap::new(),
                agent_registry: None,
            })),
            next_plan_id: Arc::new(AtomicU64::new(1)),
        }
    }

    // ── Plan lifecycle (sync fast paths) ────────────────────────────────

    /// Acquire a write guard from a sync context via try-write + yield loop.
    ///
    /// TODO: This module is deprecated (use cognitive loop in chat_phases.rs instead).
    /// The busy-spin is replaced with a small sleep to avoid CPU burning.
    /// Will panic after `max_spin_ms` to avoid unbounded blocking.
    #[allow(clippy::needless_continue)]
    pub(crate) fn sync_write(&self) -> tokio::sync::RwLockWriteGuard<'_, BrainLoopInner> {
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_millis(
                self.inner
                    .try_read()
                    .map(|g| g.config.max_spin_ms)
                    .unwrap_or(5000),
            );
        loop {
            match self.inner.try_write() {
                Ok(guard) => return guard,
                Err(_) => {
                    if std::time::Instant::now() > deadline {
                        panic!(
                            "sync_write timed out after {} ms",
                            self.inner
                                .try_read()
                                .map(|g| g.config.max_spin_ms)
                                .unwrap_or(5000)
                        );
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
        }
    }

    /// Acquire a read guard from a sync context via try-read + yield loop.
    ///
    /// TODO: This module is deprecated (use cognitive loop in chat_phases.rs instead).
    /// The busy-spin is replaced with a small sleep to avoid CPU burning.
    /// Will panic after `max_spin_ms` to avoid unbounded blocking.
    #[allow(clippy::needless_continue)]
    pub(crate) fn sync_read(&self) -> tokio::sync::RwLockReadGuard<'_, BrainLoopInner> {
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_millis(
                self.inner
                    .try_read()
                    .map(|g| g.config.max_spin_ms)
                    .unwrap_or(5000),
            );
        loop {
            match self.inner.try_read() {
                Ok(guard) => return guard,
                Err(_) => {
                    if std::time::Instant::now() > deadline {
                        panic!(
                            "sync_read timed out after {} ms",
                            self.inner
                                .try_read()
                                .map(|g| g.config.max_spin_ms)
                                .unwrap_or(5000)
                        );
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Return the current Unix time in milliseconds.
pub(crate) fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|e| {
            tracing::warn!("system time is before UNIX_EPOCH: {}", e);
            Default::default()
        })
        .as_millis() as u64
}

/// Extract a coarse error type from an error message.
///
/// Splits on the first `:` to capture the error kind prefix (e.g.
/// "network error", "timeout", "validation failure"), falling back
/// to the full message when no delimiter is present.
pub(crate) fn extract_error_type(msg: &str) -> String {
    msg.split(':').next().unwrap_or(msg).trim().to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(deprecated)]
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
            context: None,
        }
    }

    fn default_config() -> BrainLoopConfig {
        BrainLoopConfig {
            max_iterations: 5,
            max_steps_per_iteration: 10,
            reflection_required: true,
            auto_replan: true,
            min_score: 0.7,
            convergence_threshold: 0.05,
            plans_directory: None,
            enable_deep_reasoning: false,
            max_deep_reasoning_tokens: 4096,
            deep_reasoning_model: None,
            world_model_integration: true,
            max_spin_ms: 5000,
        }
    }

    // -----------------------------------------------------------------------
    // test_new_brain_loop_empty
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_new_brain_loop_empty() {
        let bl = BrainLoop::new(default_config());
        let plans = bl.list_plans();
        assert!(plans.is_empty(), "new brain loop should have no plans");

        let profile = bl.profile().await;
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

    #[tokio::test]
    async fn test_start_plan() {
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

    #[tokio::test]
    async fn test_execute_step() {
        let bl = BrainLoop::new(default_config());
        let steps = vec![make_step("s1", "Step one")];
        let plan_id = bl.start_plan("Goal", steps).unwrap();

        bl.execute_step(&plan_id, "s1", "output from step 1")
            .await
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

    #[tokio::test]
    async fn test_execute_nonexistent_step_fails() {
        let bl = BrainLoop::new(default_config());
        let plan_id = bl
            .start_plan("Goal", vec![make_step("s1", "Real step")])
            .unwrap();

        let err = bl.execute_step(&plan_id, "s999", "data").await.unwrap_err();
        assert!(
            err.to_string().contains("error.step_not_found"),
            "error should mention the missing step id: {err}"
        );

        // Executing on a non-existent plan should also fail.
        let err2 = bl
            .execute_step("plan-nonexistent", "s1", "data")
            .await
            .unwrap_err();
        assert!(
            err2.to_string().contains("error.plan_not_found"),
            "error should mention the missing plan id: {err2}"
        );
    }

    // -----------------------------------------------------------------------
    // test_reflect
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_reflect() {
        let bl = BrainLoop::new(default_config());
        let plan_id = bl
            .start_plan("Goal", vec![make_step("s1", "Step A")])
            .unwrap();

        bl.execute_step(&plan_id, "s1", "done").await.unwrap();

        let reflection = bl
            .reflect(
                &plan_id,
                "s1",
                vec!["observed X".to_string()],
                vec!["issue Y".to_string()],
                vec!["improve Z".to_string()],
            )
            .await
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

    #[tokio::test]
    async fn test_replan_adds_new_steps() {
        let bl = BrainLoop::new(default_config());
        let plan_id = bl
            .start_plan("Goal", vec![make_step("s1", "Old step")])
            .unwrap();

        // Execute and reflect.
        bl.execute_step(&plan_id, "s1", "result").await.unwrap();
        bl.reflect(&plan_id, "s1", vec!["ok".to_string()], vec![], vec![])
            .await
            .unwrap();

        // Replan with two new steps.
        let new_steps = vec![
            make_step("s2", "Revised step 1"),
            make_step("s3", "Revised step 2"),
        ];
        bl.replan(&plan_id, new_steps).await.unwrap();

        let plan = bl.get_plan(&plan_id).unwrap();
        assert_eq!(plan.phase, BrainLoopPhase::Replanning);
        // The old step s1 remains (completed), plus two new ones.
        assert_eq!(plan.steps.len(), 3);
        assert_eq!(plan.steps[0].id, "s1");
        assert_eq!(plan.steps[1].id, "s2");
        assert_eq!(plan.steps[2].id, "s3");
    }

    // -----------------------------------------------------------------------
    // test_execute_step_with_context (GAP-B50-05)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_execute_step_with_context() {
        let bl = BrainLoop::new(default_config());
        let plan_id = bl
            .start_plan("Context test", vec![make_step("c1", "Context step")])
            .unwrap();

        let ctx = TaskContext::new("ctx-1".to_string());
        let returned_ctx = bl
            .execute_step_with_context(&plan_id, "c1", "output with context", ctx)
            .await
            .unwrap();

        assert_eq!(returned_ctx.id, "ctx-1");
        assert!(returned_ctx.reasoning_trace.is_empty());
        assert!((returned_ctx.confidence - 1.0).abs() < f64::EPSILON);

        // The step should have the context attached.
        let plan = bl.get_plan(&plan_id).unwrap();
        let step = &plan.steps[0];
        assert!(step.context.is_some(), "step should have context attached");
        let step_ctx = step.context.as_ref().unwrap();
        assert_eq!(step_ctx.id, "ctx-1");
    }

    // -----------------------------------------------------------------------
    // test_reflect_includes_context_snapshot (GAP-B50-05)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_reflect_includes_context_snapshot() {
        let bl = BrainLoop::new(default_config());
        let plan_id = bl
            .start_plan(
                "Reflect context",
                vec![make_step("rct1", "Reflect with ctx")],
            )
            .unwrap();

        // Execute with context.
        let ctx = TaskContext::new("ctx-reflect-1".to_string());
        bl.execute_step_with_context(&plan_id, "rct1", "executed", ctx)
            .await
            .unwrap();

        // Reflect — should capture context_snapshot and reasoning_chain.
        let reflection = bl
            .reflect(
                &plan_id,
                "rct1",
                vec!["observed".to_string()],
                vec![],
                vec!["improve".to_string()],
            )
            .await
            .unwrap();

        assert_eq!(reflection.step_id, "rct1");
        assert!(
            reflection.context_snapshot.is_some(),
            "reflection should capture context snapshot"
        );
        let snap = reflection.context_snapshot.as_ref().unwrap();
        assert_eq!(snap.id, "ctx-reflect-1");
        // The reasoning_chain should be empty because no reasoning_trace
        // was added to the context before execution.
        assert!(
            reflection.reasoning_chain.is_empty(),
            "reasoning_chain should be empty when context has no reasoning_trace"
        );
    }

    // -----------------------------------------------------------------------
    // test_replan_merges_contexts (GAP-B50-05)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_replan_merges_contexts() {
        let bl = BrainLoop::new(default_config());
        let plan_id = bl
            .start_plan(
                "Replan context merge",
                vec![
                    make_step("m1", "Merge step 1"),
                    make_step("m2", "Merge step 2"),
                ],
            )
            .unwrap();

        // Execute both steps with different contexts.
        let ctx1 = TaskContext::new("ctx-m1".to_string());
        bl.execute_step_with_context(&plan_id, "m1", "out1", ctx1)
            .await
            .unwrap();

        let ctx2 = TaskContext::new("ctx-m2".to_string());
        bl.execute_step_with_context(&plan_id, "m2", "out2", ctx2)
            .await
            .unwrap();

        // Reflect on both so they become Done.
        bl.reflect(&plan_id, "m1", vec![], vec![], vec![])
            .await
            .unwrap();
        bl.reflect(&plan_id, "m2", vec![], vec![], vec![])
            .await
            .unwrap();

        // Replan — new steps should receive merged context.
        let new_steps = vec![
            make_step("m3", "Merged step 1"),
            make_step("m4", "Merged step 2"),
        ];
        bl.replan(&plan_id, new_steps).await.unwrap();

        let plan = bl.get_plan(&plan_id).unwrap();
        // m1 and m2 are Done, m3 and m4 are new.
        assert_eq!(plan.steps.len(), 4);

        // New steps should have a merged context.
        let step3 = &plan.steps[2];
        assert!(
            step3.context.is_some(),
            "new step should have merged context"
        );
        let merged = step3.context.as_ref().unwrap();
        // Merged context should have a new UUID-based id.
        assert_ne!(merged.id, "ctx-m1");
        assert_ne!(merged.id, "ctx-m2");
        // parent_context_id should point to first parent.
        assert_eq!(merged.parent_context_id.as_deref(), Some("ctx-m1"));

        // Step 4 should share the same merged context.
        let step4 = &plan.steps[3];
        assert!(
            step4.context.is_some(),
            "step 4 should also have merged context"
        );
        assert_eq!(
            step4.context.as_ref().unwrap().id,
            merged.id,
            "both new steps should share the same merged context id"
        );
    }

    // -----------------------------------------------------------------------
    // test_context_propagation_chain (GAP-B50-05)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_context_propagation_chain() {
        let bl = BrainLoop::new(default_config());
        let plan_id = bl
            .start_plan(
                "Chain propagation",
                vec![make_step("a", "Step A"), make_step("b", "Step B")],
            )
            .unwrap();

        // Step A: execute with context containing a reasoning trace.
        let mut ctx_a = TaskContext::new("ctx-a".to_string());
        ctx_a
            .reasoning_trace
            .push("Step A: initial analysis".to_string());
        ctx_a.confidence = 0.8;
        let ctx_a_returned = bl
            .execute_step_with_context(&plan_id, "a", "result_a", ctx_a)
            .await
            .unwrap();

        // Step B: pass Step A's context downstream.
        let mut ctx_b = ctx_a_returned.clone();
        ctx_b.id = "ctx-b".to_string();
        ctx_b
            .reasoning_trace
            .push("Step B: refined analysis".to_string());
        ctx_b.parent_context_id = Some(ctx_a_returned.id.clone());
        let _ctx_b_returned = bl
            .execute_step_with_context(&plan_id, "b", "result_b", ctx_b)
            .await
            .unwrap();

        // Reflect on step B to verify reasoning chain is captured.
        let reflection = bl
            .reflect(&plan_id, "b", vec!["final".to_string()], vec![], vec![])
            .await
            .unwrap();

        // The reasoning chain should include traces from both A and B.
        assert!(
            reflection.reasoning_chain.len() >= 2,
            "reasoning chain should contain traces from upstream steps"
        );
        assert!(
            reflection
                .reasoning_chain
                .iter()
                .any(|t| t.contains("Step A")),
            "reasoning chain should include Step A's trace"
        );
        assert!(
            reflection
                .reasoning_chain
                .iter()
                .any(|t| t.contains("Step B")),
            "reasoning chain should include Step B's trace"
        );

        // context_snapshot should hold step B's final context.
        assert!(reflection.context_snapshot.is_some());
        let snap = reflection.context_snapshot.as_ref().unwrap();
        assert_eq!(snap.id, "ctx-b");
    }

    // -----------------------------------------------------------------------
    // test_complete_plan
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_complete_plan() {
        let bl = BrainLoop::new(default_config());
        let plan_id = bl
            .start_plan("Goal", vec![make_step("s1", "Step")])
            .unwrap();

        bl.complete_plan(&plan_id).await.unwrap();

        let plan = bl.get_plan(&plan_id).unwrap();
        assert_eq!(plan.phase, BrainLoopPhase::Completed);
        assert!(plan.phase.is_terminal());

        // Completing an already completed plan should fail.
        let err = bl.complete_plan(&plan_id).await.unwrap_err();
        assert!(err.to_string().contains("error.plan_already_terminal"));
    }

    // -----------------------------------------------------------------------
    // test_fail_plan
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_fail_plan() {
        let bl = BrainLoop::new(default_config());
        let plan_id = bl
            .start_plan("Goal", vec![make_step("s1", "Step")])
            .unwrap();

        bl.fail_plan(&plan_id, "Something went wrong")
            .await
            .unwrap();

        let plan = bl.get_plan(&plan_id).unwrap();
        assert_eq!(plan.phase, BrainLoopPhase::Failed);
        assert!(plan.phase.is_terminal());
        assert_eq!(plan.fail_reason, "Something went wrong");

        // Failing an already failed plan should fail.
        let err = bl.fail_plan(&plan_id, "again").await.unwrap_err();
        assert!(err.to_string().contains("error.plan_already_terminal"));
    }

    // -----------------------------------------------------------------------
    // test_cancel_plan
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_cancel_plan() {
        let bl = BrainLoop::new(default_config());
        let plan_id = bl
            .start_plan("Goal", vec![make_step("s1", "Step")])
            .unwrap();

        bl.cancel_plan(&plan_id).await.unwrap();

        let plan = bl.get_plan(&plan_id).unwrap();
        assert_eq!(plan.phase, BrainLoopPhase::Cancelled);
        assert!(plan.phase.is_terminal());

        // Cancelling an already cancelled plan should fail.
        let err = bl.cancel_plan(&plan_id).await.unwrap_err();
        assert!(err.to_string().contains("error.plan_already_terminal"));
    }

    // -----------------------------------------------------------------------
    // test_max_iterations_enforced
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_max_iterations_enforced() {
        let config = BrainLoopConfig {
            max_iterations: 2,
            ..default_config()
        };
        let bl = BrainLoop::new(config);

        // Start a plan with a single step.
        let plan_id = bl
            .start_plan("Iteration test", vec![make_step("s1", "Step")])
            .unwrap();

        // Iteration 1: execute step.
        bl.execute_step(&plan_id, "s1", "iter 1").await.unwrap();
        {
            let plan = bl.get_plan(&plan_id).unwrap();
            assert_eq!(plan.current_iteration, 1);
            assert!(!plan.phase.is_terminal());
        }

        // Reflect so the step is done, then replan with a new step for the next iteration.
        bl.reflect(&plan_id, "s1", vec![], vec![], vec![])
            .await
            .unwrap();
        bl.replan(&plan_id, vec![make_step("s2", "Iter 2 step")])
            .await
            .unwrap();

        // Iteration 2: execute new step.
        bl.execute_step(&plan_id, "s2", "iter 2").await.unwrap();
        {
            let plan = bl.get_plan(&plan_id).unwrap();
            assert_eq!(plan.current_iteration, 2);
            assert!(!plan.phase.is_terminal());
        }

        // Reflect and replan for iteration 3 (over limit).
        bl.reflect(&plan_id, "s2", vec![], vec![], vec![])
            .await
            .unwrap();
        bl.replan(&plan_id, vec![make_step("s3", "Iter 3 step")])
            .await
            .unwrap();

        // Executing s3 pushes iteration to 3, which exceeds max_iterations.
        bl.execute_step(&plan_id, "s3", "iter 3").await.unwrap();

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

    #[tokio::test]
    async fn test_profile_reflects_state() {
        let bl = BrainLoop::new(default_config());

        // Profile before any plans.
        let p0 = bl.profile().await;
        assert_eq!(p0.total_plans, 0);
        assert_eq!(p0.active_plans, 0);

        // Start two plans.
        let pid_a = bl
            .start_plan("Plan A", vec![make_step("a1", "A1")])
            .unwrap();
        let pid_b = bl
            .start_plan("Plan B", vec![make_step("b1", "B1")])
            .unwrap();

        let p1 = bl.profile().await;
        assert_eq!(p1.total_plans, 2);
        assert_eq!(p1.active_plans, 2);

        // Execute a step on plan A → cycles = 1.
        bl.execute_step(&pid_a, "a1", "out").await.unwrap();

        let p2 = bl.profile().await;
        assert_eq!(p2.total_cycles, 1);
        assert!(p2.avg_cycles_per_plan > 0.0);

        // Complete plan A.
        bl.complete_plan(&pid_a).await.unwrap();

        let p3 = bl.profile().await;
        assert_eq!(p3.completed_plans, 1);
        assert_eq!(p3.active_plans, 1);
        assert_eq!(p3.total_plans, 2);

        // Fail plan B.
        bl.fail_plan(&pid_b, "Timeout").await.unwrap();

        let p4 = bl.profile().await;
        assert_eq!(p4.failed_plans, 1);
        assert_eq!(p4.active_plans, 0);
        assert_eq!(p4.total_plans, 2);
    }

    // -----------------------------------------------------------------------
    // test_get_nonexistent_plan_fails
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_nonexistent_plan_fails() {
        let bl = BrainLoop::new(default_config());

        let err = bl.get_plan("does-not-exist").unwrap_err();
        assert!(err.to_string().contains("not found"));

        let err = bl.current_phase("phantom-plan").await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    // -----------------------------------------------------------------------
    // test_deep_reasoning_config (GAP-B50-03)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_deep_reasoning_config() {
        let config = BrainLoopConfig {
            enable_deep_reasoning: true,
            ..default_config()
        };
        let bl = BrainLoop::new(config);

        // Verify the config is stored correctly by checking profile with a plan.
        let plan_id = bl
            .start_plan("Deep reasoning test", vec![make_step("d1", "Deep step")])
            .unwrap();
        assert!(bl.get_plan(&plan_id).is_ok());

        // The phase variant should exist and not be terminal.
        assert!(!BrainLoopPhase::DeepReasoning.is_terminal());

        // New fields from GAP-B50-06 should default correctly.
        let cfg = BrainLoopConfig::default();
        assert_eq!(cfg.max_deep_reasoning_tokens, 4096);
        assert!(cfg.deep_reasoning_model.is_none());
        assert!(cfg.world_model_integration);
    }

    // -----------------------------------------------------------------------
    // test_run_async (GAP-B50-03)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_run_async() {
        let bl = BrainLoop::new(default_config());
        let steps = vec![make_step("r1", "Run step")];

        let profile = bl.run_async("Async run test", steps).await.unwrap();
        assert_eq!(profile.total_plans, 1);
        assert_eq!(
            profile.completed_plans, 1,
            "run_async should complete the plan"
        );
        assert_eq!(profile.active_plans, 0);
    }

    // -----------------------------------------------------------------------
    // test_run_sync_compat (GAP-B50-03)
    // -----------------------------------------------------------------------

    /// Note: uses a regular `#[test]` because `run()` creates its own
    /// temporary tokio runtime internally.
    #[test]
    #[allow(deprecated)]
    fn test_run_sync_compat() {
        let bl = BrainLoop::new(default_config());
        let steps = vec![make_step("rs1", "Sync compat step")];

        let profile = bl.run("Sync compat test", steps).unwrap();
        assert_eq!(profile.total_plans, 1);
        assert_eq!(profile.completed_plans, 1);
    }

    // -----------------------------------------------------------------------
    // test_deep_reasoning_engine_noop_when_disabled (GAP-B50-06)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_deep_reasoning_engine_noop_when_disabled() {
        let config = BrainLoopConfig {
            enable_deep_reasoning: false,
            max_deep_reasoning_tokens: 0,
            ..default_config()
        };
        let engine = DeepReasoningEngine::new(&config);
        assert_eq!(engine.max_reasoning_tokens, 0);
        assert!(engine.model.is_none());

        // plan_with_reasoning should return plan unchanged (no reasoning).
        let context = TaskContext {
            id: "ctx-1".to_string(),
            reasoning_trace: vec![],
            intermediate_findings: HashMap::new(),
            confidence: 0.5,
            open_questions: vec![],
            assumptions: vec![],
            parent_context_id: None,
        };
        let plan = BrainLoopPlan {
            id: "p-1".to_string(),
            goal: "test".to_string(),
            steps: vec![make_step("s1", "step 1")],
            max_iterations: 5,
            current_iteration: 0,
            created_ms: 0,
            phase: BrainLoopPhase::Planning,
            fail_reason: String::new(),
            reasoning: None,
            world_model_data: None,
        };
        let enriched = engine.plan_with_reasoning(&context, &plan).await;
        assert!(enriched.reasoning.is_none());
        assert_eq!(enriched.id, plan.id);

        // reflect_with_reasoning should return basic reflection.
        let reflection = engine
            .reflect_with_reasoning("output", &[], &plan, "s1")
            .await;
        assert_eq!(reflection.step_id, "s1");
        assert_eq!(reflection.confidence, 1.0);

        // replan_with_reasoning should return empty.
        let steps = engine.replan_with_reasoning(&reflection, &plan).await;
        assert!(steps.is_empty());

        // quality_validate should return 1.0.
        let score = engine.quality_validate(&plan).await;
        assert!((score - 1.0).abs() < f64::EPSILON);
    }

    // -----------------------------------------------------------------------
    // test_deep_reasoning_engine_enabled (GAP-B50-06)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_deep_reasoning_engine_enabled() {
        let config = BrainLoopConfig {
            enable_deep_reasoning: true,
            max_deep_reasoning_tokens: 4096,
            deep_reasoning_model: Some("gpt-4".to_string()),
            ..default_config()
        };
        let engine = DeepReasoningEngine::new(&config);
        assert_eq!(engine.max_reasoning_tokens, 4096);
        assert_eq!(engine.model.as_deref(), Some("gpt-4"));

        let context = TaskContext {
            id: "ctx-deep".to_string(),
            reasoning_trace: vec!["step 1 analysis".to_string()],
            intermediate_findings: HashMap::new(),
            confidence: 0.75,
            open_questions: vec!["what if?".to_string()],
            assumptions: vec!["assume X".to_string()],
            parent_context_id: None,
        };
        let plan = BrainLoopPlan {
            id: "p-deep".to_string(),
            goal: "deep goal".to_string(),
            steps: vec![make_step("s1", "step 1"), make_step("s2", "step 2")],
            max_iterations: 5,
            current_iteration: 0,
            created_ms: 0,
            phase: BrainLoopPhase::Planning,
            fail_reason: String::new(),
            reasoning: None,
            world_model_data: None,
        };

        // plan_with_reasoning should enrich the plan.
        let enriched = engine.plan_with_reasoning(&context, &plan).await;
        assert!(
            enriched.reasoning.is_some(),
            "reasoning should be populated"
        );
        let reasoning = enriched.reasoning.as_deref().unwrap_or("");
        assert!(reasoning.contains("ctx-deep"));
        assert!(reasoning.contains("deep goal"));
        assert!(reasoning.contains("4096"));

        // reflect_with_reasoning should produce deeper analysis.
        let reflection = engine
            .reflect_with_reasoning("execution output", &[], &plan, "s1")
            .await;
        assert_eq!(reflection.step_id, "s1");
        assert!(!reflection.improvements.is_empty());
        assert!(reflection.confidence <= 1.0);

        // replan_with_reasoning should generate steps from improvements.
        let new_steps = engine.replan_with_reasoning(&reflection, &plan).await;
        assert!(
            !new_steps.is_empty(),
            "should generate steps from improvements"
        );
        assert!(new_steps[0].id.contains("reasoned"));

        // quality_validate should produce a reasonable score.
        let score = engine.quality_validate(&enriched).await;
        assert!(score > 0.0, "quality score should be > 0.0");
        assert!(score <= 1.0, "quality score should be <= 1.0");
    }

    // -----------------------------------------------------------------------
    // test_query_world_model_stub (GAP-B50-06)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_query_world_model_stub() {
        // With world model disabled, no data should be set.
        let config = BrainLoopConfig {
            world_model_integration: false,
            ..default_config()
        };
        let bl = BrainLoop::new(config);
        let plan_id = bl
            .start_plan("WM test", vec![make_step("w1", "World step")])
            .unwrap();
        bl.query_world_model(&plan_id).await;
        let plan = bl.get_plan(&plan_id).unwrap();
        assert!(
            plan.world_model_data.is_none(),
            "world_model_data should be None when integration is disabled"
        );

        // With world model enabled, stub data should be populated.
        let config = BrainLoopConfig {
            world_model_integration: true,
            ..default_config()
        };
        let bl2 = BrainLoop::new(config);
        let plan_id2 = bl2
            .start_plan("WM test 2", vec![make_step("w2", "World step 2")])
            .unwrap();
        bl2.query_world_model(&plan_id2).await;
        let plan2 = bl2.get_plan(&plan_id2).unwrap();
        assert!(
            plan2.world_model_data.is_some(),
            "world_model_data should be populated when integration is enabled"
        );
        let data = plan2.world_model_data.unwrap();
        assert_eq!(
            data.get("environment").and_then(|v| v.as_str()),
            Some("world-model-v1")
        );
        assert!(data.contains_key("query_timestamp_ms"));
    }

    // -----------------------------------------------------------------------
    // test_run_async_with_deep_reasoning (GAP-B50-06)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_run_async_with_deep_reasoning() {
        // Disable auto_replan to prevent infinite re-looping from
        // replan_with_reasoning generating steps from reflection improvements.
        // Use auto_replan: false so the plan completes after the first
        // execute-reflect cycle without entering deep reasoning replanning.
        let config = BrainLoopConfig {
            enable_deep_reasoning: true,
            auto_replan: false,
            ..default_config()
        };
        let bl = BrainLoop::new(config);
        let steps = vec![make_step("dr1", "Deep run step")];

        let profile = bl.run_async("Deep reasoning run", steps).await.unwrap();
        assert_eq!(profile.total_plans, 1);
        assert_eq!(
            profile.completed_plans, 1,
            "run_async with deep reasoning should complete the plan"
        );
        assert_eq!(profile.active_plans, 0);
    }

    // -----------------------------------------------------------------------
    // test_deep_reasoning_plan_reasoning_field (GAP-B50-06)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_deep_reasoning_plan_reasoning_field() {
        let config = BrainLoopConfig {
            enable_deep_reasoning: true,
            ..default_config()
        };
        let bl = BrainLoop::new(config);
        let plan_id = bl
            .start_plan("Reasoning field test", vec![make_step("rf1", "RF step")])
            .unwrap();

        // Manually set reasoning on the plan.
        {
            let mut inner = write_guard(&bl.inner).await;
            if let Some(p) = inner.plans.get_mut(&plan_id) {
                p.reasoning = Some("manual reasoning chain".to_string());
                let mut wm = HashMap::new();
                wm.insert("entity".to_string(), Value::String("test".to_string()));
                p.world_model_data = Some(wm);
            }
        }

        let plan = bl.get_plan(&plan_id).unwrap();
        assert_eq!(plan.reasoning.as_deref(), Some("manual reasoning chain"));
        assert!(plan.world_model_data.is_some());
        let wm = plan.world_model_data.unwrap();
        assert_eq!(wm.get("entity").and_then(|v| v.as_str()), Some("test"));
    }

    // -----------------------------------------------------------------------
    // test_enable_deep_reasoning_default_false
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_enable_deep_reasoning_default_false() {
        let config = BrainLoopConfig::default();
        assert!(
            !config.enable_deep_reasoning,
            "enable_deep_reasoning should default to false"
        );
        assert_eq!(
            config.max_deep_reasoning_tokens, 4096,
            "max_deep_reasoning_tokens should default to 4096"
        );
        assert!(
            config.deep_reasoning_model.is_none(),
            "deep_reasoning_model should default to None"
        );
        assert!(
            config.world_model_integration,
            "world_model_integration should default to true"
        );
    }
}
