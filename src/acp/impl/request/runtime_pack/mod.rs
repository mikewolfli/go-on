//! Runtime operations — health probes, metrics, self-model, governance, copilot, etc.
//!
//! This module was split from the original monolithic `runtime_pack.rs` into
//! several sub-modules.  All public items are re-exported here so that
//! existing `use runtime_pack::*` imports continue to work.

pub(crate) mod copilot;
pub(crate) mod governance;
pub(crate) mod handlers;
pub(crate) mod health;
pub(crate) mod metrics;
pub(crate) mod self_model;

pub(crate) use copilot::*;
pub(crate) use governance::*;
pub(crate) use handlers::*;
pub(crate) use health::*;
pub(crate) use metrics::*;
pub(crate) use self_model::*;
