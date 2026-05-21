//! Execution pack — handles workflow execution, task execution, auto-repair,
//! learning feedback replay, and runtime subtask orchestration.
//!
//! This module was split from the original monolithic `exec_pack.rs` into
//! focused sub-modules:
//!
//! - [`workflow`] — workflow run lifecycle and `workflow.execute` handler
//! - [`task`] — `task.execute` handler
//! - [`learning`] — auto-repair loops, replay scoring, memory / guardrail helpers
//! - [`handlers`] — runtime execution context, subtask dispatch, utility functions

use super::*;

mod workflow;
pub(super) use workflow::*;

mod task;
pub(super) use task::*;

mod learning;
pub(super) use learning::*;

pub(crate) mod handlers;
pub(crate) use handlers::*;
