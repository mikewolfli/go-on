//! # Brain Loop — Plan → Execute → Reflect → Replan
//!
//! This module implements F-GAP-17 "脑回路" — an iterative orchestration cycle
//! that drives a plan forward through the Plan → Execute → Reflect → Replan
//! loop until convergence, failure, or exhaustion of iterations.
//!
//! ## Deprecation note (GAP-46-07)
//!
//! The structured `brain_loop` sub-module has been superseded by the flat
//! `crate::orchestration::brain_loop` module.  The flat version now includes
//! all features previously only available here (BrainLoopProfile convergence
//! info, BrainLoopReport, Reflection, convergence detection).
//!
//! The `brain_loop` sub-module is retained for backward-compatibility of
//! existing serialized data but should not be used for new integrations.

pub mod brain_loop;

// Re-exports kept for backward compatibility only — prefer
// `crate::orchestration::brain_loop` for new code.
#[allow(unused_imports)]
pub use brain_loop::{
    BrainLoop, BrainLoopConfig, BrainLoopProfile, BrainLoopReport, BrainLoopState, BrainLoopStep,
    Reflection,
};
