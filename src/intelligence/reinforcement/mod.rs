//! Sub-modules for the BLUE2 reinforcement utilities.
//!
//! The original monolithic `reinforcement.rs` has been split into focused
//! modules. The top-level `reinforcement.rs` re-exports all public items
//! to preserve backward compatibility for paths like `crate::reinforcement::*`.

pub mod action_check;
pub mod health;
pub mod learning;
pub mod task_plan;
