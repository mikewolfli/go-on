//! ACP Prelude - Constants
//!
//! Shared constants used throughout the ACP system.

use std::time::Duration;

/// Maximum checkpoints per conversation
pub const MAX_CHECKPOINTS_PER_CONVERSATION: usize = 256;

/// Slow lock wait threshold (used by lock monitor).
/// `pub(crate)` so sibling modules under `prelude/` can access it.
#[expect(dead_code, reason = "F-GAP-49 reserved for lock monitor re-activation")]
pub(crate) const ACP_LOCK_SLOW_WAIT_THRESHOLD: Duration = Duration::from_millis(5);
