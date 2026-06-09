//! ACP Prelude - Re-exports from other modules
//!
//! These types are defined in other ACP or governance modules and re-exported
//! here for convenience.

/// Re-export of `ReviewTimeoutPolicy` from the agent implementation module.
/// The canonical definition lives in `crate::acp::impl::agent`.
pub use crate::acp::r#impl::agent::ReviewTimeoutPolicy;

/// Re-export of `ReviewGateOutcome` from the agent implementation module.
/// The canonical definition lives in `crate::acp::impl::agent`.
pub use crate::acp::r#impl::agent::ReviewGateOutcome;

/// Online controller state - real implementation from governance module
pub use crate::governance::runtime_controls::OnlineControllerState;
