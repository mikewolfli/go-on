use super::*;

async fn list_checkpoint_records(
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

async fn find_checkpoint(
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

async fn get_branch_head_id(
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

async fn prune_checkpoints(
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

fn params_task(params: &Value) -> Option<String> {
    params
        .get("task")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn session_id_for_task(task: &str) -> String {
    let compact = task
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(24)
        .collect::<String>();
    format!(
        "clarify-{}",
        if compact.is_empty() {
            "session"
        } else {
            compact.as_str()
        }
    )
}
