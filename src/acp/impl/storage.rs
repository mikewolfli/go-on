//! Storage implementation functions for ACP server
//!
//! This module contains standalone functions that implement storage-related
//! functionality previously in the `impl AcpServer` block in `impl/storage.rs`.
//! These functions take `AcpServer` as their first parameter to maintain
//! compatibility with the original implementation.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tokio::task::spawn_blocking;

use crate::acp::server::AcpServer;
use crate::cache::ResponseCache;
use crate::i18n::runtime::tf;
use crate::performance::CacheStats;

/// Get value from cache
///
/// This function replaces the `AcpServer::cache_get` method.
pub async fn cache_get(
    _server: &AcpServer,
    cache: Arc<ResponseCache>,
    cache_key: String,
) -> Result<Option<crate::cache::CachedResponse>> {
    spawn_blocking(move || cache.get(&cache_key))
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "{}",
                tf(
                    "error.task_join",
                    &[("task", "cache_get"), ("error", &format!("{}", e))]
                )
            )
        })?
}

/// Put value into cache
///
/// This function replaces the `AcpServer::cache_put` method.
pub async fn cache_put(
    _server: &AcpServer,
    cache: Arc<ResponseCache>,
    cache_key: String,
    response_text: String,
    agent_name: String,
    ttl: Option<u64>,
) -> Result<()> {
    spawn_blocking(move || cache.put(&cache_key, &response_text, &agent_name, ttl))
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "{}",
                tf(
                    "error.task_join",
                    &[("task", "cache_put"), ("error", &format!("{}", e))]
                )
            )
        })?
}

/// Get cache entry count
///
/// This function replaces the `AcpServer::cache_entry_count` method.
pub async fn cache_entry_count(_server: &AcpServer, cache: Arc<ResponseCache>) -> Result<u64> {
    spawn_blocking(move || cache.entry_count())
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "{}",
                tf(
                    "error.task_join",
                    &[("task", "cache_entry_count"), ("error", &format!("{}", e))]
                )
            )
        })?
}

/// Clear cache
///
/// This function replaces the `AcpServer::cache_clear` method.
pub async fn cache_clear(_server: &AcpServer, cache: Arc<ResponseCache>) -> Result<usize> {
    spawn_blocking(move || cache.clear_all())
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "{}",
                tf(
                    "error.task_join",
                    &[("task", "cache_clear"), ("error", &format!("{}", e))]
                )
            )
        })?
}

/// Get cache statistics
///
/// This function replaces the `AcpServer::cache_stats` method.
pub async fn cache_stats(_server: &AcpServer, cache: Arc<ResponseCache>) -> Result<CacheStats> {
    let snapshot = spawn_blocking(move || cache.stats()).await.map_err(|e| {
        anyhow::anyhow!(
            "{}",
            tf(
                "error.task_join",
                &[("task", "cache_stats"), ("error", &format!("{}", e))]
            )
        )
    })??;

    let total_size = snapshot.entry_count as usize;
    let max_size = snapshot.max_entries.max(1);
    let utilization = (total_size as f64 / max_size as f64).clamp(0.0, 1.0);
    let total_hits = snapshot.total_hits.min(u32::MAX as u64) as u32;

    Ok(CacheStats {
        total_size,
        max_size,
        total_hits,
        avg_hits_per_entry: snapshot.avg_hits_per_entry,
        utilization,
    })
}

/// Persist checkpoint summary
///
/// This function replaces the `AcpServer::persist_checkpoint_summary` method.
pub fn persist_checkpoint_summary(
    server: &AcpServer,
    checkpoint: &crate::acp::prelude::ConversationCheckpoint,
) {
    let summary = crate::reinforcement::CheckpointSummaryArtifact {
        checkpoint_id: checkpoint.checkpoint_id.clone(),
        conversation_id: checkpoint.conversation_id.clone(),
        branch_id: checkpoint.branch_id.clone(),
        parent_checkpoint_id: None,
        created_at: checkpoint.created_at,
        note: checkpoint.note.clone(),
        message_count: checkpoint.messages.len(),
        message_chars: checkpoint
            .messages
            .iter()
            .map(|message| message.content.chars().count())
            .sum(),
        assistant_excerpt: None,
    };

    if let Ok(ledger) = server.artifact_ledger.lock() {
        let dir = ledger.root().join("checkpoints");
        let path = checkpoint_summary_path(&dir, &checkpoint.checkpoint_id);
        if fs::create_dir_all(&dir).is_ok() {
            let _ = fs::write(
                path,
                serde_json::to_vec_pretty(&summary).unwrap_or_default(),
            );
        }
    }
}

/// Load checkpoint summary
///
/// This function replaces the `AcpServer::load_checkpoint_summary` method.
pub fn load_checkpoint_summary(
    server: &AcpServer,
    checkpoint_id: &str,
) -> Option<crate::reinforcement::CheckpointSummaryArtifact> {
    let ledger = server.artifact_ledger.lock().ok()?;
    let path = checkpoint_summary_path(&ledger.root().join("checkpoints"), checkpoint_id);
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Save conversation state
///
/// This function replaces the `AcpServer::save_conversation_state` method.
pub fn save_conversation_state(
    _server: &AcpServer,
    conversation_id: &str,
    state: &crate::acp::helpers::conversation::ConversationState,
) -> Result<()> {
    let root = _server
        .artifact_ledger
        .lock()
        .map(|ledger| ledger.root().join("conversation-state"))
        .unwrap_or_else(|_| PathBuf::from(".goon/conversation-state"));
    fs::create_dir_all(&root)?;
    fs::write(
        root.join(format!("{}.json", conversation_id)),
        serde_json::to_vec_pretty(state)?,
    )?;
    Ok(())
}

/// Load conversation state
///
/// This function replaces the `AcpServer::load_conversation_state` method.
pub fn load_conversation_state(
    server: &AcpServer,
    conversation_id: &str,
) -> Option<crate::acp::helpers::conversation::ConversationState> {
    let root = server
        .artifact_ledger
        .lock()
        .map(|ledger| ledger.root().join("conversation-state"))
        .ok()?;
    let raw = fs::read_to_string(root.join(format!("{}.json", conversation_id))).ok()?;
    serde_json::from_str(&raw).ok()
}

fn checkpoint_summary_path(root: &std::path::Path, checkpoint_id: &str) -> PathBuf {
    root.join(format!("checkpoint-{}.json", checkpoint_id))
}
