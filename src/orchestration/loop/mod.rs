//! # Brain Loop — Plan → Execute → Reflect → Replan
//!
//! This module implements F-GAP-17 "脑回路" — an iterative orchestration cycle
//! that drives a plan forward through the Plan → Execute → Reflect → Replan
//! loop until convergence, failure, or exhaustion of iterations.
//!
//! ## Sub-modules
//!
//! | Module | Description |
//! |--------|-------------|
//! | `brain_loop` | Core state machine, data types, and the full loop runner |

pub mod brain_loop;

// ---------------------------------------------------------------------------
// Re-exports — the most commonly used items can be pulled straight from `loop`.
// ---------------------------------------------------------------------------

pub use brain_loop::{
    BrainLoop, BrainLoopConfig, BrainLoopProfile, BrainLoopReport, BrainLoopState,
    BrainLoopStep, Reflection,
};
