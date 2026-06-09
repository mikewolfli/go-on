//! ACP Prelude - Utility Functions
//!
//! Free functions used across the ACP system.

use std::collections::HashMap;
use std::sync::Mutex as StdMutex;

use crate::agent::Message;
use crate::acp::prelude::constants::{
    MAX_CHECKPOINTS_PER_CONVERSATION, MAX_CONVERSATIONS_TRACKED,
};
use crate::acp::prelude::types::ConversationState;

/// Get current timestamp in seconds (delegates to `crate::shared::timestamps`).
pub fn now_ts() -> i64 {
    crate::shared::timestamps::now_ts()
}

/// Get current timestamp in milliseconds (delegates to `crate::shared::timestamps`).
pub fn now_ts_ms() -> i64 {
    crate::shared::timestamps::now_ts_ms()
}

/// Calculate checkpoint message characters
#[allow(dead_code)] // F-GAP-49 — planned wiring
pub fn checkpoint_message_chars(messages: &[Message]) -> usize {
    messages.iter().map(|m| m.content.chars().count()).sum()
}

/// Touch conversation order (update LRU)
#[allow(dead_code)] // F-GAP-49 — planned wiring
pub fn touch_conversation_order(order: &StdMutex<Vec<String>>, conversation_id: &str) {
    let mut guard = order.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("conversation order lock poisoned, recovering");
        poisoned.into_inner()
    });
    // Remove if exists
    guard.retain(|id| id != conversation_id);
    // Add to front (most recent)
    guard.insert(0, conversation_id.to_string());
    // Trim if too long
    if guard.len() > MAX_CONVERSATIONS_TRACKED {
        guard.truncate(MAX_CONVERSATIONS_TRACKED);
    }
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

/// Evict oldest conversation
#[allow(dead_code)] // F-GAP-49 — planned wiring
pub fn evict_oldest_conversation(
    store: &mut HashMap<String, ConversationState>,
    order: &StdMutex<Vec<String>>,
) -> Option<String> {
    let mut order_guard = match order.lock() {
        Ok(guard) => guard,
        Err(_) => return None,
    };

    while let Some(oldest_id) = order_guard.pop() {
        if store.contains_key(&oldest_id) {
            store.remove(&oldest_id);
            return Some(oldest_id);
        }
    }

    None
}
