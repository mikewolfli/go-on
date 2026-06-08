#![deprecated(note = "Use cognitive loop in chat_phases.rs instead")]

//! # Brain Loop — Plan → Execute → Reflect → Replan
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

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::RwLock;
use tokio::task::JoinHandle;

use crate::agent::{Agent, AgentRegistry, Message, StreamingSender};
use crate::agents::progress_reporter::ProgressReporter;
use crate::intelligence::metacognitive::{CorrectiveStatus, MetacognitiveController};
use crate::intelligence::world_model::{EntityType, WorldModel, WorldModelConfig};
use crate::orchestration::core_dag::TaskContext;

use std::time::{SystemTime, UNIX_EPOCH};

use crate::i18n::runtime::tf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// RwLock guard helpers
// ---------------------------------------------------------------------------

/// Acquire a read guard on the inner RwLock.
async fn read_guard<T>(rw: &RwLock<T>) -> tokio::sync::RwLockReadGuard<'_, T> {
    rw.read().await
}

/// Acquire a write guard on the inner RwLock.
async fn write_guard<T>(rw: &RwLock<T>) -> tokio::sync::RwLockWriteGuard<'_, T> {
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
// DeepReasoningEngine (GAP-B50-06)
// ---------------------------------------------------------------------------

/// Engine that provides LLM-level reasoning augmentation for the brain loop.
///
/// When `enable_deep_reasoning` is disabled, all methods act as no-ops.
/// When enabled, the engine enriches plans with reasoning chains, produces
/// richer reflections, adjusts plans based on reflection content, and
/// validates outputs using a MultiModelVoter-style consensus approach.
#[derive(Clone)]
pub struct DeepReasoningEngine {
    /// Maximum tokens for a single reasoning chain.
    pub max_reasoning_tokens: usize,
    /// Optional model name override for deep-reasoning calls.
    #[allow(dead_code)] // F-GAP-51 — reserved for future use
    pub model: Option<String>,
    /// Agent registry for LLM calls (B51-08).
    /// When `Some`, `plan_with_reasoning` and `reflect_with_reasoning` call
    /// the configured LLM agent for real reasoning chains instead of stubs.
    pub agent_registry: Option<Arc<AgentRegistry>>,
}

impl DeepReasoningEngine {
    /// Create a new engine from configuration.
    pub fn new(config: &BrainLoopConfig) -> Self {
        Self {
            max_reasoning_tokens: config.max_deep_reasoning_tokens,
            model: config.deep_reasoning_model.clone(),
            agent_registry: None,
        }
    }

    /// Set the agent registry for LLM-backed reasoning.
    pub fn with_agent_registry(mut self, registry: Arc<AgentRegistry>) -> Self {
        self.agent_registry = Some(registry);
        self
    }

    /// Produce a structured plan enhanced with LLM-level reasoning.
    ///
    /// Takes a [`TaskContext`] (chain-of-thought state from the DAG executor)
    /// and the current plan, and returns a plan with the `reasoning` field
    /// populated with the analysis chain.
    ///
    /// When deep reasoning is disabled (`max_reasoning_tokens == 0`),
    /// returns the plan unchanged.
    ///
    /// When an `agent_registry` is available (B51-08), calls the configured
    /// LLM agent to generate a real reasoning chain. Otherwise falls back to
    /// a structured summary.
    pub async fn plan_with_reasoning(
        &self,
        context: &TaskContext,
        plan: &BrainLoopPlan,
    ) -> BrainLoopPlan {
        if self.max_reasoning_tokens == 0 {
            return plan.clone();
        }

        let mut enriched = plan.clone();

        // ── Attempt LLM-backed reasoning if agent registry is available ──
        if let Some(ref registry) = self.agent_registry {
            if let Some(agent) = registry.get("primary") {
                let prompt = format!(
                    "You are a deep reasoning engine analyzing a plan.\n\
                     Task \"{}\" with {} steps ({} pending).\n\
                     Context: confidence={:.2}, reasoning_trace={:?}, \
                     open_questions={:?}, assumptions={:?}.\n\
                     Provide a concise reasoning analysis identifying gaps, \
                     risks, and improvement suggestions (max {} tokens).",
                    plan.goal,
                    plan.steps.len(),
                    plan.steps
                        .iter()
                        .filter(|s| s.status == StepStatus::Pending)
                        .count(),
                    context.confidence,
                    context.reasoning_trace,
                    context.open_questions,
                    context.assumptions,
                    self.max_reasoning_tokens,
                );
                let reasoning = Self::call_llm_and_collect(&agent, &prompt).await;
                enriched.reasoning = Some(reasoning);
                return enriched;
            }
        }

        // ── Fallback: structured summary ───────────────────────────────
        enriched.reasoning = Some(format!(
            "Deep reasoning analysis (max_tokens={}):\n\
             - Context id: {}\n\
             - Confidence: {:.2}\n\
             - Reasoning trace ({} steps): {:?}\n\
             - Open questions: {:?}\n\
             - Assumptions: {:?}\n\
             - Plan goal: {}\n\
             - Steps: {} pending / {} total",
            self.max_reasoning_tokens,
            context.id,
            context.confidence,
            context.reasoning_trace.len(),
            context.reasoning_trace,
            context.open_questions,
            context.assumptions,
            plan.goal,
            plan.steps
                .iter()
                .filter(|s| s.status == StepStatus::Pending)
                .count(),
            plan.steps.len(),
        ));
        enriched
    }

    /// Produce a reflection enhanced with LLM-level improvement suggestions.
    ///
    /// Takes the step execution result and reflection history, and returns
    /// a [`BrainLoopReflection`] with deeper analysis.
    ///
    /// When deep reasoning is disabled, returns a basic empty reflection.
    ///
    /// When an `agent_registry` is available (B51-08), calls the configured
    /// LLM agent to generate real analysis. Otherwise falls back to a
    /// structured summary.
    pub async fn reflect_with_reasoning(
        &self,
        result: &str,
        history: &[BrainLoopReflection],
        plan: &BrainLoopPlan,
        step_id: &str,
    ) -> BrainLoopReflection {
        if self.max_reasoning_tokens == 0 {
            return BrainLoopReflection {
                step_id: step_id.to_string(),
                observations: vec![result.to_string()],
                issues: vec![],
                improvements: vec![],
                confidence: 1.0,
                reflection_ms: now_epoch_ms(),
                context_snapshot: None,
                reasoning_chain: vec![],
            };
        }

        let now = now_epoch_ms();
        let prev_confidence = history.last().map(|r| r.confidence).unwrap_or(1.0);

        // ── Attempt LLM-backed reflection if agent registry is available ──
        if let Some(ref registry) = self.agent_registry {
            if let Some(agent) = registry.get("primary") {
                let prompt = format!(
                    "You are a deep reflection engine analyzing execution results.\n\
                     Step \"{}\" of plan \"{}\" (iteration {}/{}).\n\
                     Execution result: {}\n\
                     Prior reflections: {}\n\
                     Previous confidence: {:.2}\n\
                     \n\
                     Provide a structured reflection with:\n\
                     1. Key observations\n\
                     2. Issues identified\n\
                     3. Concrete improvements\n\
                     4. Confidence score (0.0-1.0)\n\
                     Keep the analysis concise (max {} tokens).",
                    step_id,
                    plan.goal,
                    plan.current_iteration,
                    plan.max_iterations,
                    result,
                    history.len(),
                    prev_confidence,
                    self.max_reasoning_tokens,
                );
                let analysis = Self::call_llm_and_collect(&agent, &prompt).await;
                return BrainLoopReflection {
                    step_id: step_id.to_string(),
                    observations: vec![result.to_string(), analysis.clone()],
                    issues: vec![],
                    improvements: vec![],
                    confidence: (prev_confidence + 0.9) / 2.0,
                    reflection_ms: now,
                    context_snapshot: None,
                    reasoning_chain: vec![analysis],
                };
            }
        }

        // ── Fallback: structured summary ───────────────────────────────
        let analysis = format!(
            "Deep reflection (max_tokens={}):\n\
             - Step result: {}\n\
             - Previous confidence: {:.2}\n\
             - Prior reflections: {}\n\
             - Plan iteration: {}/{}\n\
             Suggested improvements based on reasoning analysis.",
            self.max_reasoning_tokens,
            result,
            prev_confidence,
            history.len(),
            plan.current_iteration,
            plan.max_iterations,
        );

        BrainLoopReflection {
            step_id: step_id.to_string(),
            observations: vec![result.to_string(), analysis],
            issues: vec![],
            improvements: vec![
                "Review reasoning trace for gaps in logic".to_string(),
                "Cross-check open questions against execution output".to_string(),
            ],
            confidence: (prev_confidence + 0.9) / 2.0,
            reflection_ms: now,
            context_snapshot: None,
            reasoning_chain: vec![],
        }
    }

    /// Generate new steps using reflection content (not just confidence scores).
    ///
    /// When deep reasoning is disabled, returns an empty vector (no replanning).
    pub async fn replan_with_reasoning(
        &self,
        reflection: &BrainLoopReflection,
        plan: &BrainLoopPlan,
    ) -> Vec<BrainLoopStep> {
        if self.max_reasoning_tokens == 0 {
            return vec![];
        }

        let mut new_steps = Vec::new();
        for (i, improvement) in reflection.improvements.iter().enumerate() {
            let step_id = format!("{}-reasoned-{}", plan.id, i + 1);
            new_steps.push(BrainLoopStep {
                id: step_id,
                phase: BrainLoopPhase::Planning,
                description: improvement.clone(),
                input: String::new(),
                output: String::new(),
                started_ms: 0,
                completed_ms: 0,
                duration_ms: 0,
                status: StepStatus::Pending,
                context: None,
            });
        }
        new_steps
    }

    /// Validate a plan or reflection using LLM-backed consensus when
    /// an agent registry is available, or heuristic rules otherwise.
    ///
    /// Returns a quality score between 0.0 and 1.0.
    pub async fn quality_validate<T: Serialize>(&self, item: &T) -> f64 {
        if self.max_reasoning_tokens == 0 {
            return 1.0;
        }

        // ── LLM-backed validation if agent registry is available ───────
        if let Some(ref registry) = self.agent_registry {
            if let Some(agent) = registry.get("primary") {
                let json_str = serde_json::to_string(item).unwrap_or_default();
                let prompt = format!(
                    "You are a quality validator. Rate the quality of the following \
                     plan/reflection on a scale of 0.0 to 1.0. \
                     Consider completeness, clarity, reasoning depth, and feasibility.\n\
                     \n\
                     Item:\n{}\n\
                     \n\
                     Respond with ONLY a floating point number between 0.0 and 1.0.",
                    if json_str.len() > 2000 {
                        &json_str[..2000]
                    } else {
                        &json_str
                    }
                );
                let response = Self::call_llm_and_collect(&agent, &prompt).await;
                // Parse floating point from response
                let score: f64 = response
                    .trim()
                    .chars()
                    .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0.5);
                return score.clamp(0.0, 1.0);
            }
        }

        // ── Heuristic fallback ─────────────────────────────────────────
        let json = serde_json::to_value(item).unwrap_or_default();
        let score = if json.is_null() {
            0.0
        } else if let Some(obj) = json.as_object() {
            let has_reasoning = obj.contains_key("reasoning");
            let has_steps = obj.contains_key("steps") || obj.contains_key("observations");
            let has_confidence = obj.contains_key("confidence");
            let fields_score = (if has_reasoning { 0.3 } else { 0.0 })
                + (if has_steps { 0.4 } else { 0.0 })
                + (if has_confidence { 0.3 } else { 0.0 });
            let content_score =
                if let Some(reasoning) = obj.get("reasoning").and_then(|v| v.as_str()) {
                    let len = reasoning.len() as f64;
                    (len / 500.0).min(0.2)
                } else {
                    0.0
                };
            (fields_score + content_score).min(1.0)
        } else {
            0.5
        };
        score.clamp(0.0, 1.0)
    }

    // ── Private helpers ────────────────────────────────────────────────

    /// Call an LLM agent with a prompt and collect the full text response.
    async fn call_llm_and_collect(agent: &Arc<dyn Agent>, prompt: &str) -> String {
        let msg = Message {
            role: "user".to_string(),
            content: prompt.to_string(),
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(1024);
        let sender = StreamingSender::from(tx);
        let agent_clone = Arc::clone(agent);
        let msg_clone = msg.clone();

        let task =
            tokio::spawn(
                async move { agent_clone.chat(vec![msg_clone], None, None, sender).await },
            );

        let mut response = String::new();
        while let Some(token) = rx.recv().await {
            // Skip control tokens
            if token.starts_with("__model_used__:") {
                continue;
            }
            if token.starts_with("__tool_call__:") {
                continue;
            }
            if let Some(reasoning) = token.strip_prefix("__thinking__") {
                response.push_str(reasoning);
            } else {
                response.push_str(&token);
            }
        }

        match task.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::warn!("DeepReasoningEngine: LLM call failed: {e}");
            }
            Err(join_err) => {
                tracing::warn!("DeepReasoningEngine: LLM task panicked: {join_err}");
            }
        }

        response
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
    /// Convergence status info (e.g. "converged after 3 iterations", "not converged").
    pub convergence_info: String,
    /// Average step score across all plans (0.0 – 1.0).
    pub avg_step_score: f64,
    /// Total steps across all plans.
    pub total_steps: u64,
}

/// Summary report produced by a full Plan → Execute → Reflect → Replan cycle.
// Reserved for future BrainLoop integration.
#[allow(dead_code)]
// F-GAP-49 — reserved for future use
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainLoopReport {
    /// Number of iterations executed.
    pub iterations: usize,
    /// Final composite score.
    pub final_score: f64,
    /// Whether the loop converged.
    pub converged: bool,
    /// Full history of steps across iterations.
    pub history: Vec<BrainLoopStep>,
}

/// A reflection produced after analysing a plan + result pair.
// Reserved for future BrainLoop integration.
#[allow(dead_code)]
// F-GAP-49 — reserved for future use
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reflection {
    pub score: f64,
    pub issues: Vec<String>,
    pub improvements: Vec<String>,
    pub converged: bool,
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
    cancelled_plans_total: u64,
    /// Optional progress reporter for streaming status hints.
    progress_reporter: Option<ProgressReporter>,
    /// Running async tasks spawned by the brain loop, keyed by plan id.
    /// Reserved for GAP-B50-06 deep-reasoning integration.
    #[allow(dead_code)]
    brain_loop_tasks: HashMap<String, JoinHandle<()>>,
    /// Optional metacognitive controller for self-correction feedback.
    metacognitive: Option<MetacognitiveController>,
    /// Planner hints accumulated during loop execution.
    planner_hints: Vec<PlannerHint>,
    /// Tracks per-error-type occurrence counts for detecting repeated
    /// failures (3+ → PlannerHint warning).
    error_counts: HashMap<String, u32>,
    /// B51-08: Optional agent registry for LLM-backed deep reasoning.
    agent_registry: Option<Arc<AgentRegistry>>,
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
    inner: Arc<RwLock<BrainLoopInner>>,
    next_plan_id: Arc<AtomicU64>,
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
    fn sync_write(&self) -> tokio::sync::RwLockWriteGuard<'_, BrainLoopInner> {
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
    fn sync_read(&self) -> tokio::sync::RwLockReadGuard<'_, BrainLoopInner> {
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

    /// Start a new plan with the given `goal` and initial `steps`.
    ///
    /// Returns the assigned plan id on success.
    pub fn start_plan(&self, goal: &str, steps: Vec<BrainLoopStep>) -> anyhow::Result<String> {
        let id_num = self.next_plan_id.fetch_add(1, Ordering::AcqRel);
        let id = format!("plan-{id_num}");

        let now = now_epoch_ms();
        let mut inner = self.sync_write();
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
            reasoning: None,
            world_model_data: None,
        };
        inner.plans.insert(id.clone(), plan);
        inner.total_plans_started += 1;
        Ok(id)
    }

    /// Get a clone of a plan by its id.
    pub fn get_plan(&self, id: &str) -> anyhow::Result<BrainLoopPlan> {
        self.sync_read()
            .plans
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("plan `{id}` not found"))
    }

    /// Attach a progress reporter for streaming status hints.
    ///
    /// When set, the brain loop will emit phase and progress tokens
    /// through the reporter during its Think-Act-Observe cycle.
    pub fn set_progress_reporter(&self, reporter: ProgressReporter) {
        let mut inner = self.sync_write();
        inner.progress_reporter = Some(reporter);
    }

    /// Attach a metacognitive controller for self-correction feedback.
    ///
    /// When set, `run_async` will query the controller for historical
    /// corrective actions and inject preventive measures as constraints
    /// into the planning loop.
    pub fn set_metacognitive(&self, mc: MetacognitiveController) {
        let mut inner = self.sync_write();
        inner.metacognitive = Some(mc);
    }

    /// Set the agent registry for LLM-backed deep reasoning (B51-08).
    pub fn set_agent_registry(&self, registry: Arc<AgentRegistry>) {
        let mut inner = self.sync_write();
        inner.agent_registry = Some(registry);
    }

    /// Return accumulated planner hints (e.g. from metacognitive feedback).
    pub fn get_planner_hints(&self) -> Vec<PlannerHint> {
        self.sync_read().planner_hints.clone()
    }

    /// Return a list of all known plan ids.
    pub fn list_plans(&self) -> Vec<String> {
        self.sync_read().plans.keys().cloned().collect()
    }

    // ── Execution (async) ──────────────────────────────────────────────

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
        let mut inner = write_guard(&self.inner).await;

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
            Self::evict_oldest_terminal_plan(&mut inner.plans);
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

        // Emit phase hint for streaming consumers.
        if let Some(ref mut reporter) = inner.progress_reporter {
            reporter.report_phase(crate::agents::progress_reporter::TOKEN_PHASE_EXECUTING);
        }

        Ok(())
    }

    // ── Execute with TaskContext (async) ────────────────────────────────

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
            let mut inner = write_guard(&self.inner).await;
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
        let inner = read_guard(&self.inner).await;
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
            .clone()
            .unwrap_or_else(|| TaskContext::new("empty-after-execute".to_string())))
    }

    // ── Reflection (async) ─────────────────────────────────────────────

    /// Record a reflection on a completed step.
    ///
    /// Moves the plan into the `Reflecting` phase.
    pub async fn reflect(
        &self,
        plan_id: &str,
        step_id: &str,
        observations: Vec<String>,
        issues: Vec<String>,
        improvements: Vec<String>,
    ) -> anyhow::Result<BrainLoopReflection> {
        let now = now_epoch_ms();
        let mut inner = write_guard(&self.inner).await;

        // Compute reflection inside a scope so the mutable plan borrow is
        // dropped before we push to `inner.reflections`.
        let reflection = {
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

            let started = plan.steps[step_idx].started_ms;
            // Capture context from the step before marking it done.
            let step_context = plan.steps[step_idx].context.clone();
            let accumulated_reasoning = step_context
                .as_ref()
                .map(|c| c.reasoning_trace.clone())
                .unwrap_or_default();

            plan.steps[step_idx].status = StepStatus::Done;
            plan.steps[step_idx].completed_ms = now;
            plan.steps[step_idx].duration_ms = now.saturating_sub(started);
            plan.phase = BrainLoopPhase::Reflecting;

            let confidence = if issues.is_empty() {
                1.0
            } else {
                // Each issue reduces confidence by 0.2, with a max penalty cap of 5 issues
                let penalty = (issues.len() as f64 * 0.2).min(1.0);
                (1.0 - penalty).max(0.1)
            };

            BrainLoopReflection {
                step_id: step_id.to_string(),
                observations,
                issues,
                improvements,
                confidence,
                reflection_ms: now,
                context_snapshot: step_context,
                reasoning_chain: accumulated_reasoning,
            }
        };

        // Emit phase hint for streaming consumers.
        if let Some(ref mut reporter) = inner.progress_reporter {
            reporter.report_phase(crate::agents::progress_reporter::TOKEN_PHASE_REFLECTING);
        }

        const MAX_REFLECTIONS: usize = 1000;
        if inner.reflections.len() >= MAX_REFLECTIONS {
            inner.reflections.remove(0);
        }
        inner.reflections.push(reflection.clone());

        Ok(reflection)
    }

    // ── Replanning (async) ─────────────────────────────────────────────

    /// Replace the remaining pending steps with a new set of steps.
    ///
    /// Existing completed / in-progress steps are preserved.
    /// The plan phase is set to `Replanning`.
    ///
    /// When TaskContexts exist on completed steps, they are merged and
    /// assigned to new steps for reasoning chain continuity.
    pub async fn replan(&self, plan_id: &str, new_steps: Vec<BrainLoopStep>) -> anyhow::Result<()> {
        let mut inner = write_guard(&self.inner).await;

        let plan = inner
            .plans
            .get_mut(plan_id)
            .ok_or_else(|| anyhow::anyhow!("{}", tf("error.plan_not_found", &[("id", plan_id)])))?;

        if plan.phase.is_terminal() {
            anyhow::bail!("{}", tf("error.plan_already_terminal", &[("id", plan_id)]));
        }

        // Collect parent TaskContexts from completed steps for merging.
        let parent_contexts: Vec<TaskContext> = plan
            .steps
            .iter()
            .filter(|s| s.status == StepStatus::Done)
            .filter_map(|s| s.context.clone())
            .collect();

        // Keep only steps that are not pending (they are either done or in progress).
        plan.steps.retain(|s| s.status != StepStatus::Pending);

        // Merge parent contexts into a single merged context for new steps.
        let merged_context = if !parent_contexts.is_empty() {
            Some(TaskContext::merge(&parent_contexts))
        } else {
            None
        };

        // Append the new steps, each receiving the merged context.
        for mut step in new_steps {
            if merged_context.is_some() {
                step.context = merged_context.clone();
            }
            plan.steps.push(step);
        }
        plan.phase = BrainLoopPhase::Replanning;

        // Emit phase hint for streaming consumers.
        if let Some(ref mut reporter) = inner.progress_reporter {
            reporter.report_phase(crate::agents::progress_reporter::TOKEN_PHASE_PLANNING);
        }

        Ok(())
    }

    // ── Terminal transitions (async) ───────────────────────────────────

    /// Mark a plan as completed.
    pub async fn complete_plan(&self, plan_id: &str) -> anyhow::Result<()> {
        let mut inner = write_guard(&self.inner).await;
        let plan = inner
            .plans
            .get_mut(plan_id)
            .ok_or_else(|| anyhow::anyhow!("{}", tf("error.plan_not_found", &[("id", plan_id)])))?;

        if plan.phase.is_terminal() {
            anyhow::bail!("{}", tf("error.plan_already_terminal", &[("id", plan_id)]));
        }
        plan.phase = BrainLoopPhase::Completed;
        inner.completed_plans_total += 1;

        // Emit completion hint for streaming consumers.
        if let Some(ref mut reporter) = inner.progress_reporter {
            reporter.report_complete();
        }

        Self::evict_oldest_terminal_plan(&mut inner.plans);
        Ok(())
    }

    /// Mark a plan as failed with a reason.
    pub async fn fail_plan(&self, plan_id: &str, reason: &str) -> anyhow::Result<()> {
        let mut inner = write_guard(&self.inner).await;
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

        // Emit completion hint on terminal state.
        if let Some(ref mut reporter) = inner.progress_reporter {
            reporter.report_complete();
        }

        Self::evict_oldest_terminal_plan(&mut inner.plans);
        Ok(())
    }

    /// Cancel a plan.
    pub async fn cancel_plan(&self, plan_id: &str) -> anyhow::Result<()> {
        let mut inner = write_guard(&self.inner).await;
        let plan = inner
            .plans
            .get_mut(plan_id)
            .ok_or_else(|| anyhow::anyhow!("{}", tf("error.plan_not_found", &[("id", plan_id)])))?;

        if plan.phase.is_terminal() {
            anyhow::bail!("{}", tf("error.plan_already_terminal", &[("id", plan_id)]));
        }
        plan.phase = BrainLoopPhase::Cancelled;
        inner.cancelled_plans_total += 1;

        // Emit completion hint on terminal state.
        if let Some(ref mut reporter) = inner.progress_reporter {
            reporter.report_complete();
        }

        Self::evict_oldest_terminal_plan(&mut inner.plans);
        Ok(())
    }

    // ── Queries (async) ────────────────────────────────────────────────

    /// The current phase of a plan.
    pub async fn current_phase(&self, plan_id: &str) -> anyhow::Result<BrainLoopPhase> {
        read_guard(&self.inner)
            .await
            .plans
            .get(plan_id)
            .map(|p| p.phase)
            .ok_or_else(|| anyhow::anyhow!("plan `{plan_id}` not found"))
    }

    /// Return a snapshot of runtime metrics.
    pub async fn profile(&self) -> BrainLoopProfile {
        let inner = read_guard(&self.inner).await;
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

        // Compute convergence info and avg step score from reflections.
        let total_steps: u64 = inner.plans.values().map(|p| p.steps.len() as u64).sum();
        let avg_step_score = if inner.reflections.is_empty() {
            0.0
        } else {
            inner.reflections.iter().map(|r| r.confidence).sum::<f64>()
                / inner.reflections.len() as f64
        };

        let convergence_info = if active_plans == 0 && total_plans > 0 {
            let converged = self.check_convergence(&inner);
            if converged {
                format!("converged after {} plans", total_plans)
            } else {
                "not converged".to_string()
            }
        } else {
            "in progress".to_string()
        };

        BrainLoopProfile {
            total_plans,
            active_plans,
            completed_plans: inner.completed_plans_total,
            failed_plans: inner.failed_plans_total,
            total_cycles: inner.total_cycles,
            avg_cycles_per_plan: avg,
            convergence_info,
            avg_step_score,
            total_steps,
        }
    }

    /// Check whether the loop has converged based on recent reflection confidence scores.
    ///
    /// Convergence is detected when:
    /// - At least two reflections exist, AND
    /// - The latest confidence score is >= `min_score`, OR
    /// - The score delta between the last two reflections is <= `convergence_threshold`.
    fn check_convergence(&self, inner: &BrainLoopInner) -> bool {
        let config = &inner.config;
        let reflections = &inner.reflections;

        if reflections.len() < 2 {
            return false;
        }

        let latest = &reflections[reflections.len() - 1];
        let previous = &reflections[reflections.len() - 2];

        if latest.confidence >= config.min_score {
            return true;
        }

        let delta = (latest.confidence - previous.confidence).abs();
        delta <= config.convergence_threshold && latest.confidence > 0.3
    }

    // ── Persistence (async) ────────────────────────────────────────────

    /// Serialize and write a plan to a JSON file in the configured `plans_directory`.
    ///
    /// Returns `Ok(())` if the plan exists and serialization succeeds, or if no
    /// directory is configured (silent no-op).
    pub async fn persist_plan(&self, plan_id: &str) -> anyhow::Result<()> {
        let (plan, dir) = {
            let inner = read_guard(&self.inner).await;
            let plan = inner
                .plans
                .get(plan_id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("plan `{plan_id}` not found"))?;
            let dir = inner.config.plans_directory.clone();
            (plan, dir)
        };

        let dir = match dir {
            Some(d) => d,
            None => return Ok(()), // no directory configured, skip
        };

        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| anyhow::anyhow!("failed to create plans directory {:?}: {e}", dir))?;

        let path = dir.join(format!("{plan_id}.json"));
        let json = serde_json::to_string_pretty(&plan)
            .map_err(|e| anyhow::anyhow!("failed to serialize plan `{plan_id}`: {e}"))?;
        tokio::fs::write(&path, &json)
            .await
            .map_err(|e| anyhow::anyhow!("failed to write plan `{plan_id}` to {:?}: {e}", path))?;
        tracing::debug!("persisted plan `{plan_id}` to {:?}", path);
        Ok(())
    }

    /// Load a plan from a JSON file in the configured `plans_directory`.
    ///
    /// Returns `None` if no directory is configured or the file does not exist.
    pub async fn load_plan(&self, plan_id: &str) -> Option<BrainLoopPlan> {
        let dir = {
            let inner = read_guard(&self.inner).await;
            inner.config.plans_directory.clone()
        };

        let dir = dir?;
        let path = dir.join(format!("{plan_id}.json"));
        if !path.exists() {
            return None;
        }
        match tokio::fs::read_to_string(&path).await {
            Ok(content) => match serde_json::from_str::<BrainLoopPlan>(&content) {
                Ok(plan) => Some(plan),
                Err(e) => {
                    tracing::warn!(
                        "failed to deserialize plan `{plan_id}` from {:?}: {e}",
                        path
                    );
                    None
                }
            },
            Err(e) => {
                tracing::warn!("failed to read plan `{plan_id}` from {:?}: {e}", path);
                None
            }
        }
    }

    // ── World model integration (GAP-B50-06, B51-08) ──────────────────

    /// Query the world model for environment entities relevant to the plan.
    ///
    /// When `world_model_integration` is enabled in the config, this queries
    /// the [`WorldModel`] for real entity data and populates the plan's
    /// `world_model_data` field.
    pub async fn query_world_model(&self, plan_id: &str) {
        let world_model_enabled = {
            let inner = read_guard(&self.inner).await;
            inner.config.world_model_integration
        };

        if !world_model_enabled {
            return;
        }

        // ── Query real world model data (B51-08) ───────────────────────
        let wm = WorldModel::new(WorldModelConfig::default());

        // Register the current plan goal as a tracked entity.
        let goal = {
            let inner = read_guard(&self.inner).await;
            inner
                .plans
                .get(plan_id)
                .map(|p| p.goal.clone())
                .unwrap_or_default()
        };

        if let Err(e) =
            wm.register_entity(&format!("brain-loop-plan-{plan_id}"), EntityType::System)
        {
            tracing::warn!("query_world_model: failed to register plan entity: {e}");
        }

        let entities = wm.query_entities(None, 0.0);
        let entity_summary: Vec<Value> = entities
            .iter()
            .map(|e: &crate::intelligence::world_model::WorldEntity| {
                serde_json::json!({
                    "id": e.id,
                    "name": e.name,
                    "entity_type": format!("{:?}", e.entity_type),
                    "confidence": e.confidence,
                    "properties": e.properties,
                })
            })
            .collect();

        let mut data = HashMap::new();
        data.insert(
            "environment".to_string(),
            Value::String("world-model-v1".to_string()),
        );
        data.insert("goal".to_string(), Value::String(goal));
        data.insert("entities".to_string(), Value::Array(entity_summary));
        data.insert(
            "query_timestamp_ms".to_string(),
            Value::Number(serde_json::Number::from(now_epoch_ms())),
        );

        let mut inner = write_guard(&self.inner).await;
        if let Some(plan) = inner.plans.get_mut(plan_id) {
            plan.world_model_data = Some(data);
        }
    }

    // ── Metacognitive feedback integration ───────────────────────────

    /// Query the metacognitive controller for historical corrective actions
    /// matching the given task type, and inject preventive measures as
    /// [`PlannerHint`]s into [`BrainLoopInner`].
    ///
    /// Detects repeated error patterns (3+ occurrences of the same error
    /// type) and generates warning hints.
    async fn integrate_metacognitive_feedback(&self, task_type: &str) {
        // Snapshot the metacognitive controller (if any) outside the write
        // lock to avoid a lock ordering inversion (sync Mutex inside async
        // RwLock).
        let mc = {
            let inner = read_guard(&self.inner).await;
            inner.metacognitive.clone()
        };

        let Some(mc) = mc else { return };

        // Query historical actions matching the task type.
        let historical = mc.get_historical_actions(task_type);
        if historical.is_empty() {
            return;
        }

        let mut hints: Vec<PlannerHint> = Vec::new();

        // Collect preventive measures from completed corrective results.
        for action in &historical {
            if let Some(ref result) = action.result {
                if !result.preventive_measures.is_empty() {
                    let hint = PlannerHint {
                        hint_type: if result.success {
                            "Info".to_string()
                        } else {
                            "Warning".to_string()
                        },
                        message: format!(
                            "Corrective action `{}`: {}. Root cause: {}",
                            action.action_type, action.description, result.root_cause,
                        ),
                        source: "metacognitive".to_string(),
                        preventive_measures: result.preventive_measures.clone(),
                    };
                    hints.push(hint);
                }
            }
        }

        // Detect repeated error patterns from historical failed actions.
        let mut error_type_counts: HashMap<String, u32> = HashMap::new();
        for action in &historical {
            if action.status == CorrectiveStatus::Failed {
                let et = action.action_type.clone();
                *error_type_counts.entry(et).or_insert(0) += 1;
            }
        }
        for (et, count) in &error_type_counts {
            if *count >= 3 {
                hints.push(PlannerHint {
                    hint_type: "Warning".to_string(),
                    message: format!(
                        "Action type `{et}` failed {count} times historically; consider a different strategy"
                    ),
                    source: "metacognitive".to_string(),
                    preventive_measures: vec![],
                });
            }
        }

        // Write hints into inner state.
        if !hints.is_empty() {
            let mut inner = write_guard(&self.inner).await;
            inner.planner_hints.extend(hints);
        }
    }

    // ── High-level orchestration (async) ───────────────────────────────

    /// Run the full Plan → Execute → Reflect → Replan cycle asynchronously.
    ///
    /// Starts a plan with the given `task` and `steps`, then iterates
    /// through pending steps — executing, reflecting, and optionally
    /// replanning — until the plan reaches a terminal phase.
    /// Returns a [`BrainLoopProfile`] snapshot at the end.
    pub async fn run_async(
        &self,
        task: &str,
        steps: Vec<BrainLoopStep>,
    ) -> anyhow::Result<BrainLoopProfile> {
        let plan_id = self.start_plan(task, steps)?;
        let task_type = task.to_string();

        // ── Check deep-reasoning configuration ────────────────────────
        let (enable_deep, engine, world_model_int) = {
            let inner = read_guard(&self.inner).await;
            let mut engine = DeepReasoningEngine::new(&inner.config);
            if let Some(ref registry) = inner.agent_registry {
                engine = engine.with_agent_registry(Arc::clone(registry));
            }
            (
                inner.config.enable_deep_reasoning,
                engine,
                inner.config.world_model_integration,
            )
        };

        // ── Deep-reasoning planning pass ──────────────────────────────
        if enable_deep {
            let plan = self.get_plan(&plan_id)?;
            let context = TaskContext {
                id: plan_id.clone(),
                reasoning_trace: vec!["Initial planning via BrainLoop run_async".to_string()],
                intermediate_findings: HashMap::new(),
                confidence: 0.8,
                open_questions: vec![],
                assumptions: vec![],
                parent_context_id: None,
            };
            let enriched = engine.plan_with_reasoning(&context, &plan).await;
            // Write back reasoning and world model data to the plan.
            {
                let mut inner = write_guard(&self.inner).await;
                if let Some(p) = inner.plans.get_mut(&plan_id) {
                    p.reasoning = enriched.reasoning;
                    p.world_model_data = enriched.world_model_data;
                }
            }
        }

        // ── World-model context query (runs regardless of deep reasoning) ──
        if world_model_int {
            self.query_world_model(&plan_id).await;
        }

        // ── Main Plan → Execute → Reflect → Replan loop ──────────────
        loop {
            // Collect pending step ids under a read lock.
            let pending: Vec<String> = {
                let inner = read_guard(&self.inner).await;
                inner
                    .plans
                    .get(&plan_id)
                    .map(|p| {
                        p.steps
                            .iter()
                            .filter(|s| s.status == StepStatus::Pending)
                            .map(|s| s.id.clone())
                            .collect()
                    })
                    .unwrap_or_default()
            };

            if pending.is_empty() {
                // ── Validate plan quality (deep mode) ─────────────────
                if enable_deep {
                    let plan = self.get_plan(&plan_id)?;
                    let quality = engine.quality_validate(&plan).await;
                    tracing::debug!(
                        "BrainLoop: deep quality validation score = {:.2} for plan `{plan_id}`",
                        quality
                    );
                }

                let phase = self.current_phase(&plan_id).await?;
                if !phase.is_terminal() {
                    self.complete_plan(&plan_id).await?;
                }

                // ── Metacognitive feedback integration ───────────────
                self.integrate_metacognitive_feedback(&task_type).await;

                return Ok(self.profile().await);
            }

            // Execute and reflect on each pending step.
            for step_id in &pending {
                if let Err(e) = self.execute_step(&plan_id, step_id, "").await {
                    // ── Track error for repeated-error detection ────────
                    let err_msg = e.to_string();
                    let error_type = extract_error_type(&err_msg);
                    {
                        let mut inner = write_guard(&self.inner).await;
                        *inner.error_counts.entry(error_type.clone()).or_insert(0) += 1;
                        let count = inner.error_counts[&error_type];
                        if count >= 3 && count % 3 == 0 {
                            let hint = PlannerHint {
                                hint_type: "Warning".to_string(),
                                message: format!(
                                    "Error type `{error_type}` occurred {count} times; consider a different approach"
                                ),
                                source: "metacognitive".to_string(),
                                preventive_measures: vec![],
                            };
                            inner.planner_hints.push(hint);
                        }
                    }

                    tracing::warn!(
                        "BrainLoop: step `{step_id}` execution failed: {e} — failing plan"
                    );
                    self.fail_plan(&plan_id, &err_msg).await?;
                    // Still integrate metacognitive feedback on failure.
                    self.integrate_metacognitive_feedback(&task_type).await;
                    return Ok(self.profile().await);
                }

                if enable_deep {
                    // Use deep-reasoning reflection.
                    let plan = self.get_plan(&plan_id)?;
                    let history = {
                        let inner = read_guard(&self.inner).await;
                        inner.reflections.clone()
                    };
                    let deep_reflection = engine
                        .reflect_with_reasoning("", &history, &plan, step_id)
                        .await;
                    if let Err(e) = self
                        .reflect(
                            &plan_id,
                            step_id,
                            deep_reflection.observations,
                            deep_reflection.issues,
                            deep_reflection.improvements,
                        )
                        .await
                    {
                        tracing::warn!("BrainLoop: deep reflection for `{step_id}` failed: {e}");
                    }
                } else {
                    // Standard reflection.
                    if let Err(e) = self
                        .reflect(&plan_id, step_id, vec![], vec![], vec![])
                        .await
                    {
                        tracing::warn!("BrainLoop: reflection for `{step_id}` failed: {e}");
                    }
                }
            }

            // Auto-replan if configured and within iteration limits.
            let should_continue = {
                let inner = read_guard(&self.inner).await;
                let config = &inner.config;
                config.auto_replan
                    && inner
                        .plans
                        .get(&plan_id)
                        .map(|p| !p.phase.is_terminal() && p.current_iteration < p.max_iterations)
                        .unwrap_or(false)
            };

            if should_continue {
                if enable_deep {
                    // Use deep-reasoning replanning based on reflection content.
                    let reflections = {
                        let inner = read_guard(&self.inner).await;
                        inner.reflections.clone()
                    };
                    if let Some(latest_reflection) = reflections.last() {
                        let plan = self.get_plan(&plan_id)?;
                        let new_steps =
                            engine.replan_with_reasoning(latest_reflection, &plan).await;
                        if !new_steps.is_empty() {
                            let _ = self.replan(&plan_id, new_steps).await;
                            continue;
                        }
                    }
                }

                // Fallback: complete the plan to avoid an infinite loop.
                let phase = self.current_phase(&plan_id).await?;
                if !phase.is_terminal() {
                    self.complete_plan(&plan_id).await?;
                }
            }
        }
    }

    /// Synchronous compatibility wrapper around [`run_async`].
    ///
    /// Creates a temporary single-threaded tokio runtime to drive the
    /// async loop to completion.  Prefer calling [`run_async`] directly
    /// when already in an async context.
    ///
    /// ╔══════════════════════════════════════════════════════════════╗
    /// ║  DEPRECATED — will be removed in a future release.         ║
    /// ║  Do NOT call from an async context — creating a nested     ║
    /// ║  runtime will panic. Use `run_async()` instead.            ║
    /// ╚══════════════════════════════════════════════════════════════╝
    #[deprecated(
        since = "1.2.0",
        note = "use run_async instead — this wrapper will be removed in a future release"
    )]
    #[allow(deprecated)] // TODO: migrate to cognitive loop in chat_phases.rs
    pub fn run(&self, task: &str, steps: Vec<BrainLoopStep>) -> anyhow::Result<BrainLoopProfile> {
        tracing::error!(
            "BrainLoop::run() is DEPRECATED and scheduled for removal — use run_async() directly instead"
        );
        let bl = self.clone();
        let task = task.to_string();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        rt.block_on(bl.run_async(&task, steps))
    }

    // ── Internal helpers ───────────────────────────────────────────────

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
fn extract_error_type(msg: &str) -> String {
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
