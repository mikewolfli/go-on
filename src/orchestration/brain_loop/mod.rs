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
//! engine (embedding-based classification + keyword heuristics), which is
//! wired into `OrchestrationServerDeps.planner`, `response_finalizer`,
//! `planner_execution_graph`, `planner_embedding` and `planner_executor`.

// ---------------------------------------------------------------------------
// Sub-modules
// ---------------------------------------------------------------------------

pub mod plan_construction;
