//! Conversation handling implementation functions for ACP server
//!
//! This module contains standalone functions that implement conversation handling
//! functionality previously in the `impl AcpServer` block in `impl/conversation.rs`.
//! These functions take `AcpServer` as their first parameter to maintain
//! compatibility with the original implementation.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde_json::{json, Value};

use crate::acp::prelude::ConversationCheckpoint;
use crate::acp::server::AcpServer;
use crate::agent::Message;

/// Maximum characters allowed in checkpoint messages
const MAX_CHECKPOINT_MESSAGE_CHARS: usize = 100_000;

/// Create a conversation checkpoint
///
/// This function replaces the `AcpServer::create_conversation_checkpoint` method.
pub async fn create_conversation_checkpoint(
    server: &AcpServer,
    conversation_id: &str,
    message: &Message,
    note: Option<String>,
    branch: Option<String>,
) -> Result<ConversationCheckpoint> {
    if message.content.chars().count() > MAX_CHECKPOINT_MESSAGE_CHARS {
        anyhow::bail!("checkpoint message exceeds maximum supported size");
    }

    Ok(crate::acp::r#impl::request::create_checkpoint_record(
        server,
        conversation_id,
        branch.as_deref().unwrap_or("main"),
        vec![message.clone()],
        note,
        None,
    )
    .await)
}

/// Get conversation state
///
/// This function replaces the `AcpServer::get_conversation_state` method.
pub async fn get_conversation_state(
    server: &AcpServer,
    conversation_id: &str,
) -> Result<Option<crate::acp::prelude::ConversationState>> {
    let state = server.conversation_state.lock().await;

    // Check if any checkpoint belongs to this conversation
    let has_conversation = state
        .checkpoints
        .iter()
        .any(|cp| cp.conversation_id == conversation_id);

    if has_conversation {
        // Return a clone of the state with only checkpoints for this conversation
        let mut filtered_state = state.clone();
        filtered_state
            .checkpoints
            .retain(|cp| cp.conversation_id == conversation_id);
        Ok(Some(filtered_state))
    } else {
        Ok(None)
    }
}

/// Get conversation checkpoint
///
/// This function replaces the `AcpServer::get_conversation_checkpoint` method.
pub async fn get_conversation_checkpoint(
    server: &AcpServer,
    conversation_id: &str,
    checkpoint_id: &str,
) -> Result<Option<ConversationCheckpoint>> {
    let state = server.conversation_state.lock().await;

    let checkpoint = state
        .checkpoints
        .iter()
        .find(|cp| cp.conversation_id == conversation_id && cp.checkpoint_id == checkpoint_id)
        .cloned();

    Ok(checkpoint)
}

/// List conversation checkpoints
///
/// This function replaces the `AcpServer::list_conversation_checkpoints` method.
pub async fn list_conversation_checkpoints(
    server: &AcpServer,
    conversation_id: &str,
    limit: Option<usize>,
) -> Result<Vec<ConversationCheckpoint>> {
    let state = server.conversation_state.lock().await;

    let mut checkpoints: Vec<ConversationCheckpoint> = state
        .checkpoints
        .iter()
        .filter(|cp| cp.conversation_id == conversation_id)
        .cloned()
        .collect();

    // Sort by creation time (newest first)
    checkpoints.sort_by_key(|checkpoint| std::cmp::Reverse(checkpoint.created_at));

    // Apply limit
    if let Some(limit) = limit {
        checkpoints.truncate(limit);
    }

    Ok(checkpoints)
}

/// Delete conversation
///
/// This function replaces the `AcpServer::delete_conversation` method.
pub async fn delete_conversation(server: &AcpServer, conversation_id: &str) -> Result<()> {
    let mut state = server.conversation_state.lock().await;

    // Remove all checkpoints for this conversation
    state
        .checkpoints
        .retain(|cp| cp.conversation_id != conversation_id);

    // Remove branch heads for this conversation
    state
        .branch_heads
        .retain(|_, head_id| !head_id.contains(conversation_id));

    Ok(())
}

/// Get branch head
///
/// This function replaces the `AcpServer::get_branch_head` method.
pub async fn get_branch_head(
    server: &AcpServer,
    conversation_id: &str,
    branch: &str,
) -> Result<Option<String>> {
    let state = server.conversation_state.lock().await;
    let key = format!("{}:{}", conversation_id, branch);

    Ok(state.branch_heads.get(&key).cloned())
}

/// Set branch head
///
/// This function replaces the `AcpServer::set_branch_head` method.
pub async fn set_branch_head(
    server: &AcpServer,
    conversation_id: &str,
    branch: &str,
    checkpoint_id: &str,
) -> Result<()> {
    let mut state = server.conversation_state.lock().await;
    let key = format!("{}:{}", conversation_id, branch);

    state.branch_heads.insert(key, checkpoint_id.to_string());

    Ok(())
}

/// Clear old conversations
///
/// This function replaces the `AcpServer::clear_old_conversations` method.
pub async fn clear_old_conversations(server: &AcpServer, max_age_seconds: i64) -> Result<usize> {
    let mut state = server.conversation_state.lock().await;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let cutoff = now - max_age_seconds;

    let old_count = state.checkpoints.len();

    // Remove checkpoints older than cutoff
    state.checkpoints.retain(|cp| cp.created_at >= cutoff);

    let removed = old_count - state.checkpoints.len();

    // Also clean up branch heads for removed conversations
    if removed > 0 {
        // Extract conversation IDs from remaining checkpoints
        let remaining_conversations: std::collections::HashSet<String> = state
            .checkpoints
            .iter()
            .map(|cp| cp.conversation_id.clone())
            .collect();

        // Remove branch heads for conversations that no longer exist
        state.branch_heads.retain(|key, _| {
            if let Some(conv_id) = key.split(':').next() {
                remaining_conversations.contains(conv_id)
            } else {
                false
            }
        });
    }

    Ok(removed)
}

/// Get conversation statistics
///
/// This function replaces the `AcpServer::get_conversation_stats` method.
pub async fn get_conversation_stats(server: &AcpServer) -> Result<Value> {
    let state = server.conversation_state.lock().await;

    // Count conversations by ID
    let mut conversation_counts: HashMap<String, usize> = HashMap::new();
    for checkpoint in &state.checkpoints {
        *conversation_counts
            .entry(checkpoint.conversation_id.clone())
            .or_insert(0) += 1;
    }

    let stats = json!({
        "total_checkpoints": state.checkpoints.len(),
        "total_conversations": conversation_counts.len(),
        "branch_heads_count": state.branch_heads.len(),
        "last_touched_at": state.last_touched_at,
        "conversation_counts": conversation_counts,
    });

    Ok(stats)
}
