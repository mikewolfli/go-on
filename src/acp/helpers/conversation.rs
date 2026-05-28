//! Conversation helper functions for ACP server
//!
//! This module provides utility functions for managing conversation state,
//! latency monitoring, pipeline gates, and storage validation.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::agent::Message;
use crate::orchestration::task_router::{RoutingDecision, TaskCharacteristics};
use crate::roles::AgentRole;

/// Histogram bucket boundaries for latency monitoring (seconds)
const HISTOGRAM_BUCKETS_SECONDS: [f64; 9] = [
    0.001, // 1ms
    0.005, // 5ms
    0.01,  // 10ms
    0.05,  // 50ms
    0.1,   // 100ms
    0.5,   // 500ms
    1.0,   // 1s
    5.0,   // 5s
    10.0,  // 10s
];

/// Maximum checkpoints per conversation
pub const MAX_CHECKPOINTS_PER_CONVERSATION: usize = 256;

/// Maximum stream chunks
pub const MAX_STREAM_CHUNKS: usize = 4_096;

/// Maximum stream characters
pub const MAX_STREAM_CHARS: usize = 256_000;

/// Conversation state structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationState {
    /// Conversation ID
    pub conversation_id: String,
    /// Checkpoints in this conversation
    pub checkpoints: Vec<crate::acp::ConversationCheckpoint>,
    /// Branch heads mapping
    pub branch_heads: HashMap<String, String>,
    /// Last touched timestamp
    pub last_touched_at: u64,
}

// Use the ConversationCheckpoint from prelude module

/// Approval strategy enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // F-GAP-17 — planned wiring for conversation approval strategy
pub enum ApprovalStrategy {
    /// No approval required
    None,
    /// Single approval required
    Single,
    /// Dual approval required
    Dual,
}

impl ApprovalStrategy {
    /// Check if dual review is needed
    pub fn needs_dual_review(&self) -> bool {
        matches!(self, Self::Dual)
    }
}

/// Observe latency in histogram
#[allow(dead_code)] // F-GAP-09 — reserved for standalone histogram integration
pub fn observe_latency_histogram(
    duration: Duration,
    count: &mut u64,
    sum_seconds: &mut f64,
    buckets: &mut [u64; HISTOGRAM_BUCKETS_SECONDS.len() + 1],
) {
    let value = duration.as_secs_f64();
    *count += 1;
    *sum_seconds += value;
    let mut idx = HISTOGRAM_BUCKETS_SECONDS.len();
    for (i, bound) in HISTOGRAM_BUCKETS_SECONDS.iter().enumerate() {
        if value <= *bound {
            idx = i;
            break;
        }
    }
    buckets[idx] = buckets[idx].saturating_add(1);
}

/// Extract task description from messages
#[allow(dead_code)] // F-GAP-16 — reserved for multi-agent routing pipeline
pub fn extract_task_description(messages: &[Message]) -> String {
    messages
        .iter()
        .rev()
        .find(|message| {
            message.role.eq_ignore_ascii_case("user") && !message.content.trim().is_empty()
        })
        .map(|message| message.content.clone())
        .or_else(|| messages.last().map(|message| message.content.clone()))
        .unwrap_or_else(|| "general task".to_string())
}

/// Check for pipeline gate violations
#[allow(dead_code)] // F-GAP-17 — planned wiring for pipeline gate enforcement
pub fn pipeline_gate_violation(
    analyzed_task: &TaskCharacteristics,
    routing: &RoutingDecision,
    approval_strategy: ApprovalStrategy,
) -> Option<String> {
    let non_trivial = analyzed_task.complexity >= 3
        || analyzed_task.needs_verification
        || analyzed_task.involves_multiple_modules
        || analyzed_task.has_safety_concerns;

    if non_trivial && routing.roles.is_empty() {
        return Some("routing produced no roles for a non-trivial task".to_string());
    }

    let reviewer_required = routing.roles.contains(&AgentRole::Reviewer)
        || routing
            .pua_enforcement
            .mandatory_roles
            .contains(&AgentRole::Reviewer);
    if reviewer_required && !approval_strategy.needs_dual_review() {
        return Some(
            "reviewer role required by pipeline routing, but current mode does not enable dual review gate"
                .to_string(),
        );
    }

    if non_trivial && routing.pua_enforcement.mandatory_safeguards.is_empty() {
        return Some("PUA safeguards missing for non-trivial task".to_string());
    }

    None
}

/// Touch conversation order (move to most recent)
#[cfg_attr(not(test), allow(dead_code))]
pub fn touch_conversation_order(order: &StdMutex<Vec<String>>, conversation_id: &str) {
    let mut guard = order.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("lock poisoned in conversation.rs: recovering");
        poisoned.into_inner()
    });
    if let Some(position) = guard.iter().position(|item| item == conversation_id) {
        guard.remove(position);
    }
    guard.push(conversation_id.to_string());
}

/// Evict oldest conversation
#[allow(dead_code)] // F-GAP-11 — reserved for LRU eviction in storage layer
pub fn evict_oldest_conversation(
    store: &mut HashMap<String, ConversationState>,
    order: &StdMutex<Vec<String>>,
) -> Option<String> {
    let mut guard = order.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("lock poisoned in conversation.rs: recovering");
        poisoned.into_inner()
    });
    while let Some(candidate) = guard.first().cloned() {
        guard.remove(0);
        if store.remove(&candidate).is_some() {
            return Some(candidate);
        }
    }
    None
}

/// Enforce checkpoint capacity
#[allow(dead_code)] // F-GAP-11 — reserved for checkpoint capacity enforcement
pub fn enforce_checkpoint_capacity(
    state: &mut ConversationState,
    incoming: usize,
    protected_checkpoint_id: Option<&str>,
) {
    let total_after_insert = state.checkpoints.len().saturating_add(incoming);
    if total_after_insert <= MAX_CHECKPOINTS_PER_CONVERSATION {
        return;
    }

    let mut overflow = total_after_insert - MAX_CHECKPOINTS_PER_CONVERSATION;
    let mut cursor = 0usize;

    // Prefer removing oldest checkpoints, but keep the rollback target when requested.
    while overflow > 0 && cursor < state.checkpoints.len() {
        let can_remove = protected_checkpoint_id
            .map(|protected| state.checkpoints[cursor].checkpoint_id != protected)
            .unwrap_or(true);
        if can_remove {
            state.checkpoints.remove(cursor);
            overflow -= 1;
        } else {
            cursor += 1;
        }
    }

    if overflow > 0 {
        let drain_to = overflow.min(state.checkpoints.len());
        state.checkpoints.drain(0..drain_to);
    }

    repair_conversation_branch_heads(state);
}

/// Check if streaming would exceed limits
pub fn stream_would_exceed_limits(
    current_chunks: usize,
    current_chars: usize,
    next_token_chars: usize,
) -> bool {
    current_chunks.saturating_add(1) > MAX_STREAM_CHUNKS
        || current_chars.saturating_add(next_token_chars) > MAX_STREAM_CHARS
}

/// Validate storage key
#[allow(dead_code)] // F-GAP-11 — reserved for storage key validation
pub fn validate_storage_key(
    value: &str,
    field: &str,
    max_len: usize,
) -> std::result::Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{} cannot be empty", field));
    }
    if trimmed.len() > max_len {
        return Err(format!("{} exceeds maximum length of {}", field, max_len));
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':' | '/'))
    {
        return Err(format!(
            "{} contains invalid characters; allowed: [A-Za-z0-9_.:/-]",
            field
        ));
    }

    Ok(trimmed.to_string())
}

/// Calculate total characters in checkpoint messages
#[allow(dead_code)] // F-GAP-11 — reserved for checkpoint size tracking
pub fn checkpoint_message_chars(messages: &[Message]) -> usize {
    messages.iter().map(|msg| msg.content.chars().count()).sum()
}

/// Repair conversation branch heads
pub fn repair_conversation_branch_heads(state: &mut ConversationState) {
    let existing_ids = state
        .checkpoints
        .iter()
        .map(|checkpoint| checkpoint.checkpoint_id.clone())
        .collect::<HashSet<_>>();
    let mut repaired_heads: HashMap<String, String> = HashMap::new();
    for (branch, head_id) in state.branch_heads.clone() {
        if existing_ids.contains(&head_id) {
            repaired_heads.insert(branch, head_id);
            continue;
        }

        if let Some(fallback) = state
            .checkpoints
            .iter()
            .rev()
            .find(|checkpoint| checkpoint.branch_id == branch)
            .map(|checkpoint| checkpoint.checkpoint_id.clone())
        {
            repaired_heads.insert(branch, fallback);
        }
    }
    state.branch_heads = repaired_heads;
}

/// Calculate branch head adjustment counts
#[allow(dead_code)] // F-GAP-11 — reserved for branch tracking diagnostics
pub fn branch_head_adjustment_counts(
    before: &HashMap<String, String>,
    after: &HashMap<String, String>,
) -> (usize, usize) {
    let mut repaired = 0usize;
    let mut dropped = 0usize;
    for (branch, old_head) in before {
        match after.get(branch) {
            Some(new_head) if new_head != old_head => repaired = repaired.saturating_add(1),
            Some(_) => {}
            None => dropped = dropped.saturating_add(1),
        }
    }

    (repaired, dropped)
}

/// Infer PUA stage from event type and phase
#[allow(dead_code)] // F-GAP-02 — reserved for PUA stage inference
pub fn infer_pua_stage(event_type: &str, phase: &str) -> Option<String> {
    if event_type.starts_with("phase.") {
        return Some(phase.to_string());
    }
    None
}

/// Normalize trace attributes
#[allow(dead_code)] // F-GAP-06 — reserved for trace attribute normalization
pub fn normalize_trace_attributes(
    event_type: &str,
    phase: &str,
    status: &str,
    inputs: Value,
) -> Value {
    let mut attrs = match inputs {
        Value::Object(map) => map,
        other => {
            let mut map = Map::new();
            map.insert("payload".to_string(), other);
            map
        }
    };

    attrs
        .entry("event_type".to_string())
        .or_insert_with(|| Value::String(event_type.to_string()));
    attrs
        .entry("phase".to_string())
        .or_insert_with(|| Value::String(phase.to_string()));
    attrs
        .entry("stage".to_string())
        .or_insert_with(|| Value::String(phase.to_string()));
    attrs.entry("policy_status".to_string()).or_insert_with(|| {
        Value::String(
            match status {
                "ok" => "pass",
                "error" => "error",
                _ => "unknown",
            }
            .to_string(),
        )
    });

    Value::Object(attrs)
}
