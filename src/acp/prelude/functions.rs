//! ACP Prelude - Utility Functions
//!
//! Free functions used across the ACP system.

use crate::acp::prelude::constants::MAX_CHECKPOINTS_PER_CONVERSATION;
use crate::acp::prelude::types::ConversationState;

/// Get current timestamp in seconds (delegates to `crate::shared::timestamps`).
pub fn now_ts() -> i64 {
    crate::shared::timestamps::now_ts()
}

/// Enforce checkpoint capacity
pub fn enforce_checkpoint_capacity(
    state: &mut ConversationState,
    incoming: usize,
    rollback_target: Option<&str>,
) {
    let total_after_insert = state.checkpoints.len().saturating_add(incoming);
    if total_after_insert <= MAX_CHECKPOINTS_PER_CONVERSATION {
        return;
    }

    let mut overflow = total_after_insert - MAX_CHECKPOINTS_PER_CONVERSATION;
    let mut cursor = 0usize;

    // Prefer removing oldest checkpoints, but keep the rollback target when requested.
    while overflow > 0 && cursor < state.checkpoints.len() {
        let checkpoint = &state.checkpoints[cursor];
        if rollback_target.is_some_and(|target| checkpoint.checkpoint_id == target) {
            cursor += 1;
            continue;
        }

        // Remove this checkpoint
        state.checkpoints.remove(cursor);
        overflow -= 1;
        // Don't increment cursor because we removed the element at this position
    }
}
