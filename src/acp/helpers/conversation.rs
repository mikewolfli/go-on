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
#[allow(dead_code)] // F-GAP-49 — planned wiring for conversation approval strategy
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
#[allow(dead_code)] // F-GAP-49 — reserved for standalone histogram integration
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
#[allow(dead_code)] // F-GAP-49 — reserved for multi-agent routing pipeline
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
#[allow(dead_code)] // F-GAP-49 — planned wiring for pipeline gate enforcement
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
#[cfg_attr(not(test), allow(dead_code))] // F-GAP-49
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
#[allow(dead_code)] // F-GAP-49 — reserved for LRU eviction in storage layer
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
#[allow(dead_code)] // F-GAP-49 — reserved for checkpoint capacity enforcement
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
#[allow(dead_code)] // F-GAP-49 — reserved for storage key validation
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
#[allow(dead_code)] // F-GAP-49 — reserved for checkpoint size tracking
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
#[allow(dead_code)] // F-GAP-49 — reserved for branch tracking diagnostics
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
#[allow(dead_code)] // F-GAP-49 — reserved for PUA stage inference
pub fn infer_pua_stage(event_type: &str, phase: &str) -> Option<String> {
    if event_type.starts_with("phase.") {
        return Some(phase.to_string());
    }
    None
}

/// Normalize trace attributes
#[allow(dead_code)] // F-GAP-49 — reserved for trace attribute normalization
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── validate_storage_key ──────────────────────────────────────────

    #[test]
    fn validate_storage_key_accepts_valid_keys() {
        let result = validate_storage_key("my-key_1/abc:def", "test", 256);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "my-key_1/abc:def");
    }

    #[test]
    fn validate_storage_key_rejects_empty() {
        let result = validate_storage_key("  ", "field", 256);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cannot be empty"));
    }

    #[test]
    fn validate_storage_key_rejects_oversized() {
        let long = "a".repeat(300);
        let result = validate_storage_key(&long, "field", 10);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exceeds maximum length"));
    }

    #[test]
    fn validate_storage_key_rejects_invalid_chars() {
        let result = validate_storage_key("hello world!", "field", 256);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid characters"));
    }

    // ── ApprovalStrategy ──────────────────────────────────────────────

    #[test]
    fn approval_strategy_dual_needs_dual_review() {
        assert!(ApprovalStrategy::Dual.needs_dual_review());
    }

    #[test]
    fn approval_strategy_none_does_not_need_dual_review() {
        assert!(!ApprovalStrategy::None.needs_dual_review());
    }

    #[test]
    fn approval_strategy_single_does_not_need_dual_review() {
        assert!(!ApprovalStrategy::Single.needs_dual_review());
    }

    // ── stream limits ─────────────────────────────────────────────────

    #[test]
    fn stream_would_exceed_limits_chunk_boundary() {
        assert!(stream_would_exceed_limits(MAX_STREAM_CHUNKS, 0, 0));
        assert!(!stream_would_exceed_limits(MAX_STREAM_CHUNKS - 1, 0, 10));
    }

    #[test]
    fn stream_would_exceed_limits_char_boundary() {
        assert!(stream_would_exceed_limits(0, MAX_STREAM_CHARS, 1));
        assert!(!stream_would_exceed_limits(0, MAX_STREAM_CHARS - 100, 50));
    }

    #[test]
    fn stream_would_exceed_limits_zero_chars_ok() {
        assert!(!stream_would_exceed_limits(0, 0, 0));
    }

    // ── extract_task_description ──────────────────────────────────────

    #[test]
    fn extract_task_description_finds_last_user_message() {
        let msgs = vec![
            Message {
                role: "user".to_string(),
                content: "first".to_string(),
            },
            Message {
                role: "assistant".to_string(),
                content: "response".to_string(),
            },
            Message {
                role: "user".to_string(),
                content: "second".to_string(),
            },
        ];
        assert_eq!(extract_task_description(&msgs), "second");
    }

    #[test]
    fn extract_task_description_falls_back_to_last_message() {
        let msgs = vec![
            Message {
                role: "assistant".to_string(),
                content: "hello".to_string(),
            },
            Message {
                role: "assistant".to_string(),
                content: "world".to_string(),
            },
        ];
        assert_eq!(extract_task_description(&msgs), "world");
    }

    #[test]
    fn extract_task_description_empty_messages_returns_general_task() {
        assert_eq!(extract_task_description(&[]), "general task");
    }

    // ── repair_conversation_branch_heads ───────────────────────────────

    #[test]
    fn repair_conversation_branch_heads_removes_stale_heads_and_falls_back() {
        let mut state = ConversationState {
            conversation_id: "conv-1".to_string(),
            checkpoints: vec![
                crate::acp::ConversationCheckpoint {
                    checkpoint_id: "cp-1".to_string(),
                    branch_id: "main".to_string(),
                    conversation_id: "conv-1".to_string(),
                    parent_checkpoint_id: None,
                    created_at: 0,
                    note: None,
                    metacognitive_loop: None,
                    messages: vec![],
                },
                crate::acp::ConversationCheckpoint {
                    checkpoint_id: "cp-2".to_string(),
                    branch_id: "main".to_string(),
                    conversation_id: "conv-1".to_string(),
                    parent_checkpoint_id: None,
                    created_at: 0,
                    note: None,
                    metacognitive_loop: None,
                    messages: vec![],
                },
            ],
            branch_heads: vec![
                ("main".to_string(), "cp-2".to_string()),
                ("stale".to_string(), "cp-3".to_string()),
            ]
            .into_iter()
            .collect(),
            last_touched_at: 0,
        };

        repair_conversation_branch_heads(&mut state);

        // stale branch should be dropped (no checkpoint with cp-3)
        assert!(!state.branch_heads.contains_key("stale"));
        // main branch should still point to cp-2
        assert_eq!(state.branch_heads.get("main").unwrap(), "cp-2");
    }

    // ── enforce_checkpoint_capacity ────────────────────────────────────

    #[test]
    fn enforce_checkpoint_capacity_evicts_oldest_when_over_limit() {
        let mut state = ConversationState {
            conversation_id: "conv-1".to_string(),
            checkpoints: (0..MAX_CHECKPOINTS_PER_CONVERSATION)
                .map(|i| crate::acp::ConversationCheckpoint {
                    checkpoint_id: format!("cp-{}", i),
                    branch_id: "main".to_string(),
                    conversation_id: "conv-1".to_string(),
                    parent_checkpoint_id: None,
                    created_at: 0,
                    note: None,
                    metacognitive_loop: None,
                    messages: vec![],
                })
                .collect(),
            branch_heads: [(
                "main".to_string(),
                format!("cp-{}", MAX_CHECKPOINTS_PER_CONVERSATION - 1),
            )]
            .into_iter()
            .collect(),
            last_touched_at: 0,
        };

        enforce_checkpoint_capacity(&mut state, 2, None);
        assert_eq!(state.checkpoints.len(), MAX_CHECKPOINTS_PER_CONVERSATION);
        // Should have removed cp-0 and cp-1 (oldest first)
        assert!(!state.checkpoints.iter().any(|c| c.checkpoint_id == "cp-0"));
        assert!(!state.checkpoints.iter().any(|c| c.checkpoint_id == "cp-1"));
    }

    #[test]
    fn enforce_checkpoint_capacity_protects_rollback_checkpoint() {
        let mut state = ConversationState {
            conversation_id: "conv-1".to_string(),
            checkpoints: (0..MAX_CHECKPOINTS_PER_CONVERSATION)
                .map(|i| crate::acp::ConversationCheckpoint {
                    checkpoint_id: format!("cp-{}", i),
                    branch_id: "main".to_string(),
                    conversation_id: "conv-1".to_string(),
                    parent_checkpoint_id: None,
                    created_at: 0,
                    note: None,
                    metacognitive_loop: None,
                    messages: vec![],
                })
                .collect(),
            branch_heads: [(
                "main".to_string(),
                format!("cp-{}", MAX_CHECKPOINTS_PER_CONVERSATION - 1),
            )]
            .into_iter()
            .collect(),
            last_touched_at: 0,
        };

        // Protect cp-5 — it should survive eviction
        enforce_checkpoint_capacity(&mut state, 2, Some("cp-5"));
        assert!(state.checkpoints.iter().any(|c| c.checkpoint_id == "cp-5"));
    }

    #[test]
    fn enforce_checkpoint_capacity_noop_when_under_limit() {
        let mut state = ConversationState {
            conversation_id: "conv-1".to_string(),
            checkpoints: (0..3)
                .map(|i| crate::acp::ConversationCheckpoint {
                    checkpoint_id: format!("cp-{}", i),
                    branch_id: "main".to_string(),
                    conversation_id: "conv-1".to_string(),
                    parent_checkpoint_id: None,
                    created_at: 0,
                    note: None,
                    metacognitive_loop: None,
                    messages: vec![],
                })
                .collect(),
            branch_heads: [("main".to_string(), "cp-2".to_string())]
                .into_iter()
                .collect(),
            last_touched_at: 0,
        };
        let len_before = state.checkpoints.len();
        enforce_checkpoint_capacity(&mut state, 1, None);
        assert_eq!(state.checkpoints.len(), len_before);
    }

    // ── pipeline_gate_violation ───────────────────────────────────────

    fn make_characteristics(
        complexity: u8,
        needs_verification: bool,
        involves_multiple_modules: bool,
        has_safety_concerns: bool,
    ) -> TaskCharacteristics {
        TaskCharacteristics {
            complexity,
            needs_verification,
            involves_multiple_modules,
            has_safety_concerns,
            description: "generic".to_string(),
            task_type: crate::orchestration::task_router::TaskType::Unknown,
            required_capabilities: vec![],
            is_time_critical: false,
        }
    }

    #[test]
    fn pipeline_gate_violation_non_trivial_missing_roles() {
        let task = make_characteristics(5, true, true, false);
        let routing = RoutingDecision {
            roles: vec![],
            requirements: vec![],
            predicted_success_rate: 0.5,
            estimated_duration_seconds: 30,
            can_parallelize: vec![],
            risk_factors: vec![],
            recommended_safeguards: vec![],
            pua_enforcement: crate::governance::pua::PuaEnforcementPlan {
                mandatory_roles: vec![],
                mandatory_safeguards: vec!["test".to_string()],
                escalation_level: "L1".to_string(),
                red_lines: vec![],
                quality_compass: vec![],
                mandatory_evidence: vec![],
                stage_requirements: vec![],
            },
        };
        let violation = pipeline_gate_violation(&task, &routing, ApprovalStrategy::None);
        assert!(violation.is_some());
        assert!(violation.unwrap().contains("no roles"));
    }

    #[test]
    fn pipeline_gate_violation_missing_dual_review_for_reviewer_role() {
        let task = make_characteristics(3, false, false, false);
        let routing = RoutingDecision {
            roles: vec![AgentRole::Reviewer],
            requirements: vec![],
            predicted_success_rate: 0.5,
            estimated_duration_seconds: 30,
            can_parallelize: vec![],
            risk_factors: vec![],
            recommended_safeguards: vec![],
            pua_enforcement: crate::governance::pua::PuaEnforcementPlan {
                mandatory_roles: vec![],
                mandatory_safeguards: vec!["test".to_string()],
                escalation_level: "L1".to_string(),
                red_lines: vec![],
                quality_compass: vec![],
                mandatory_evidence: vec![],
                stage_requirements: vec![],
            },
        };
        let violation = pipeline_gate_violation(&task, &routing, ApprovalStrategy::None);
        assert!(violation.is_some());
        assert!(violation.unwrap().contains("dual review"));
    }

    #[test]
    fn pipeline_gate_violation_missing_safeguards() {
        let task = make_characteristics(3, true, true, true);
        let routing = RoutingDecision {
            roles: vec![AgentRole::Coder],
            requirements: vec![],
            predicted_success_rate: 0.5,
            estimated_duration_seconds: 30,
            can_parallelize: vec![],
            risk_factors: vec![],
            recommended_safeguards: vec![],
            pua_enforcement: crate::governance::pua::PuaEnforcementPlan {
                mandatory_roles: vec![],
                mandatory_safeguards: vec![],
                escalation_level: "L1".to_string(),
                red_lines: vec![],
                quality_compass: vec![],
                mandatory_evidence: vec![],
                stage_requirements: vec![],
            },
        };
        let violation = pipeline_gate_violation(&task, &routing, ApprovalStrategy::Dual);
        assert!(violation.is_some());
        assert!(violation.unwrap().contains("safeguards"));
    }

    #[test]
    fn pipeline_gate_violation_trivial_task_no_violation() {
        let task = make_characteristics(1, false, false, false);
        let routing = RoutingDecision {
            roles: vec![AgentRole::Coder],
            requirements: vec![],
            predicted_success_rate: 0.5,
            estimated_duration_seconds: 30,
            can_parallelize: vec![],
            risk_factors: vec![],
            recommended_safeguards: vec![],
            pua_enforcement: crate::governance::pua::PuaEnforcementPlan {
                mandatory_roles: vec![],
                mandatory_safeguards: vec!["safeguard-1".to_string()],
                escalation_level: "L1".to_string(),
                red_lines: vec![],
                quality_compass: vec![],
                mandatory_evidence: vec![],
                stage_requirements: vec![],
            },
        };
        assert!(pipeline_gate_violation(&task, &routing, ApprovalStrategy::None).is_none());
    }

    // ── branch_head_adjustment_counts ──────────────────────────────────

    #[test]
    fn branch_head_adjustment_counts_tracks_repaired_and_dropped() {
        let mut before = HashMap::new();
        before.insert("a".to_string(), "cp-1".to_string());
        before.insert("b".to_string(), "cp-2".to_string());
        before.insert("c".to_string(), "cp-3".to_string());

        let mut after = HashMap::new();
        after.insert("a".to_string(), "cp-1".to_string()); // unchanged
        after.insert("b".to_string(), "cp-4".to_string()); // repaired
                                                           // c is dropped

        let (repaired, dropped) = branch_head_adjustment_counts(&before, &after);
        assert_eq!(repaired, 1);
        assert_eq!(dropped, 1);
    }

    // ── evict_oldest_conversation ──────────────────────────────────────

    #[test]
    fn evict_oldest_conversation_removes_oldest_entry() {
        let mut store = HashMap::new();
        store.insert(
            "conv-1".to_string(),
            ConversationState {
                conversation_id: "conv-1".to_string(),
                checkpoints: vec![],
                branch_heads: HashMap::new(),
                last_touched_at: 100,
            },
        );
        store.insert(
            "conv-2".to_string(),
            ConversationState {
                conversation_id: "conv-2".to_string(),
                checkpoints: vec![],
                branch_heads: HashMap::new(),
                last_touched_at: 200,
            },
        );

        let order = StdMutex::new(vec!["conv-1".to_string(), "conv-2".to_string()]);
        let evicted = evict_oldest_conversation(&mut store, &order);
        assert_eq!(evicted, Some("conv-1".to_string()));
        assert!(!store.contains_key("conv-1"));
    }

    #[test]
    fn evict_oldest_conversation_skips_missing_entries() {
        let mut store = HashMap::new();
        store.insert(
            "conv-2".to_string(),
            ConversationState {
                conversation_id: "conv-2".to_string(),
                checkpoints: vec![],
                branch_heads: HashMap::new(),
                last_touched_at: 0,
            },
        );

        let order = StdMutex::new(vec!["conv-1".to_string(), "conv-2".to_string()]);
        // conv-1 is not in store, should be skipped
        let evicted = evict_oldest_conversation(&mut store, &order);
        assert_eq!(evicted, Some("conv-2".to_string()));
    }

    // ── normalize_trace_attributes ─────────────────────────────────────

    #[test]
    fn normalize_trace_attributes_adds_default_fields() {
        let result = normalize_trace_attributes("phase.start", "coding", "ok", Value::Null);
        let obj = result.as_object().unwrap();
        assert_eq!(obj["event_type"], "phase.start");
        assert_eq!(obj["phase"], "coding");
        assert_eq!(obj["policy_status"], "pass");
    }

    #[test]
    fn normalize_trace_attributes_preserves_input_object() {
        let input = json!({"existing": "value"});
        let result = normalize_trace_attributes("test", "phase-1", "error", input);
        let obj = result.as_object().unwrap();
        assert_eq!(obj["existing"], "value");
        assert_eq!(obj["policy_status"], "error");
    }

    #[test]
    fn normalize_trace_attributes_maps_unknown_status() {
        let result = normalize_trace_attributes("x", "y", "unknown_status", Value::Null);
        assert_eq!(result["policy_status"], "unknown");
    }
}
