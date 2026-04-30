use super::*;
use crate::acp::prelude::ConversationCheckpoint;

pub(super) async fn list_checkpoint_records(
    server: &AcpServer,
    conversation_id: &str,
    branch_id: Option<&str>,
    limit: Option<usize>,
) -> Vec<crate::acp::prelude::ConversationCheckpoint> {
    let state = server.conversation_state.lock().await;
    let mut checkpoints = state
        .checkpoints
        .iter()
        .filter(|checkpoint| {
            checkpoint.conversation_id == conversation_id
                && branch_id
                    .map(|branch| checkpoint.branch_id == branch)
                    .unwrap_or(true)
        })
        .cloned()
        .collect::<Vec<_>>();
    checkpoints.sort_by_key(|checkpoint| std::cmp::Reverse(checkpoint.created_at));
    if let Some(limit) = limit {
        checkpoints.truncate(limit);
    }
    checkpoints
}

pub(super) async fn find_checkpoint(
    server: &AcpServer,
    conversation_id: &str,
    checkpoint_id: &str,
) -> Option<crate::acp::prelude::ConversationCheckpoint> {
    let state = server.conversation_state.lock().await;
    state
        .checkpoints
        .iter()
        .find(|checkpoint| {
            checkpoint.conversation_id == conversation_id
                && checkpoint.checkpoint_id == checkpoint_id
        })
        .cloned()
}

pub(super) async fn get_branch_head_id(
    server: &AcpServer,
    conversation_id: &str,
    branch_id: &str,
) -> Option<String> {
    let state = server.conversation_state.lock().await;
    state
        .branch_heads
        .get(&format!("{}:{}", conversation_id, branch_id))
        .cloned()
}

pub(super) async fn prune_checkpoints(
    server: &AcpServer,
    conversation_id: &str,
    branch_id: &str,
    keep: usize,
) -> (usize, usize, usize) {
    let mut state = server.conversation_state.lock().await;
    let mut checkpoints = state
        .checkpoints
        .iter()
        .filter(|checkpoint| {
            checkpoint.conversation_id == conversation_id && checkpoint.branch_id == branch_id
        })
        .cloned()
        .collect::<Vec<_>>();
    checkpoints.sort_by_key(|checkpoint| std::cmp::Reverse(checkpoint.created_at));
    let retained = checkpoints
        .iter()
        .take(keep)
        .map(|checkpoint| checkpoint.checkpoint_id.clone())
        .collect::<Vec<_>>();
    let before = state.checkpoints.len();
    state.checkpoints.retain(|checkpoint| {
        checkpoint.conversation_id != conversation_id
            || checkpoint.branch_id != branch_id
            || retained.contains(&checkpoint.checkpoint_id)
    });
    let removed = before.saturating_sub(state.checkpoints.len());

    let branch_key = format!("{}:{}", conversation_id, branch_id);
    let mut repaired_heads = 0;
    if let Some(head) = state.branch_heads.get(&branch_key).cloned() {
        if !retained.contains(&head) {
            if let Some(new_head) = retained.first() {
                state.branch_heads.insert(branch_key, new_head.clone());
                repaired_heads = 1;
            }
        }
    }

    (removed, repaired_heads, 0)
}

pub(super) fn params_task(params: &Value) -> Option<String> {
    params
        .get("task")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Create a checkpoint record and store it in the server's conversation state.
///
/// Called from chat.rs and conversation.rs modules after a response is produced.
pub async fn create_checkpoint_record(
    server: &AcpServer,
    conversation_id: &str,
    branch_id: &str,
    messages: Vec<Message>,
    note: Option<String>,
    parent_checkpoint_id: Option<String>,
) -> ConversationCheckpoint {
    use std::time::{SystemTime, UNIX_EPOCH};

    let checkpoint_id = format!(
        "cp-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        messages.len()
    );

    // Acquire a single lock for the duration of the read + write.
    let mut state = server.conversation_state.lock().await;
    let branch_key = format!("{}:{}", conversation_id, branch_id);

    // Auto-detect parent checkpoint from current branch head when not explicitly provided.
    // This ensures checkpoints created after rollback or normal creation form a proper chain.
    let resolved_parent =
        parent_checkpoint_id.or_else(|| state.branch_heads.get(&branch_key).cloned());

    let checkpoint = ConversationCheckpoint {
        checkpoint_id,
        conversation_id: conversation_id.to_string(),
        branch_id: branch_id.to_string(),
        parent_checkpoint_id: resolved_parent,
        created_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64,
        note,
        metacognitive_loop: None,
        messages,
    };

    // Update branch head to this checkpoint
    state
        .branch_heads
        .insert(branch_key, checkpoint.checkpoint_id.clone());
    state.checkpoints.push(checkpoint.clone());
    // Enforce capacity by removing oldest excess checkpoints
    enforce_checkpoint_capacity(&mut state, 1, Some(&checkpoint.checkpoint_id));
    checkpoint
}

/// Persist a metacognitive loop state into the most recent checkpoint.
///
/// Called from chat.rs after a response completes to track reflection/agent selection state.
/// Returns the loop_state so callers can store it alongside the checkpoint.
pub async fn persist_checkpoint_metacognitive_loop(
    server: &AcpServer,
    conversation_id: &str,
    branch_id: &str,
    checkpoint_id: &str,
    loop_state: Value,
) -> Value {
    let mut state = server.conversation_state.lock().await;
    if let Some(checkpoint) = state.checkpoints.iter_mut().find(|cp| {
        cp.checkpoint_id == checkpoint_id
            && cp.conversation_id == conversation_id
            && cp.branch_id == branch_id
    }) {
        checkpoint.metacognitive_loop = Some(loop_state.clone());
    }
    loop_state
}
