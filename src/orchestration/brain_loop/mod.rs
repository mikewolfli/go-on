//! Brain loop — plan construction (FUTURE5.MD M5).
//!
//! ## Status
//!
//! The former `BrainLoop` orchestration state machine (Plan → Execute →
//! Reflect → Replan) was removed in round 23: the `use_brain_loop` branch in
//! `autonomy_loop_adapter` was deleted earlier (bookkeeping-only, never invoked
//! the agent or tools), so the only production surface was `new()` + `profile()`
//! which reported all-zero state — no plan was ever started, executed, or
//! reflected upon in production. The `omnipotent` module was removed for the
//! same reason.
//!
//! The live part is [`plan_construction`]: the `Planner` task-decomposition
//! engine (keyword + subtask-hint heuristics), which is used by
//! `response_finalizer`, `planner_execution_graph` and `planner_executor`.
//! The former `planner_embedding` module was removed: its
//! `EmbeddingTaskClassifier` duplicated `Planner::analyze_task` and its
//! complexity result was always overwritten by the analyze_task context.

// ---------------------------------------------------------------------------
// Sub-modules
// ---------------------------------------------------------------------------

pub mod plan_construction;
