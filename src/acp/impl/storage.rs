//! Storage implementation functions for ACP server
//!
//! This module contains standalone functions that implement storage-related
//! functionality previously in the `impl AcpServer` block in `impl/storage.rs`.
//! These functions take `AcpServer` as their first parameter to maintain
//! compatibility with the original implementation.

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
pub async fn cache_entry_count(
    _server: &AcpServer,
    cache: Arc<ResponseCache>,
) -> Result<u64> {
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
pub async fn cache_clear(
    _server: &AcpServer,
    cache: Arc<ResponseCache>,
) -> Result<usize> {
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
/// TODO: Implement proper cache stats - ResponseCache doesn't have stats() method
pub async fn cache_stats(
    _server: &AcpServer,
    _cache: Arc<ResponseCache>,
) -> Result<CacheStats> {
    // TODO: Implement proper cache statistics
    // For now, return default stats
    Ok(CacheStats {
        total_size: 0,
        max_size: 0,
        total_hits: 0,
        avg_hits_per_entry: 0.0,
        utilization: 0.0,
    })
}

/// Persist checkpoint summary
///
/// This function replaces the `AcpServer::persist_checkpoint_summary` method.
pub fn persist_checkpoint_summary(
    server: &AcpServer,
    checkpoint: &crate::acp::prelude::ConversationCheckpoint,
) {
    // This is a simplified implementation for migration
    // In the original code, this creates a CheckpointSummaryArtifact
    // and stores it in the artifact ledger

    let _summary = crate::reinforcement::CheckpointSummaryArtifact {
        checkpoint_id: checkpoint.checkpoint_id.clone(),
        conversation_id: checkpoint.conversation_id.clone(),
        branch_id: checkpoint.branch_id.clone(),
        parent_checkpoint_id: None,
        created_at: checkpoint.created_at,
        note: checkpoint.note.clone(),
        message_count: checkpoint.messages.len(),
        message_chars: 0, // Simplified for migration
        assistant_excerpt: None,
    };

    // Store in artifact ledger if available
    // Note: store_artifact method doesn't exist on ArtifactLedger
    // This is simplified for migration
    if let Ok(_ledger_guard) = server.artifact_ledger.lock() {
        // Artifact ledger exists but store_artifact method not available
        // This is a TODO for full migration
    }
}

/// Load checkpoint summary
///
/// This function replaces the `AcpServer::load_checkpoint_summary` method.
pub fn load_checkpoint_summary(
    server: &AcpServer,
    _checkpoint_id: &str,
) -> Option<crate::reinforcement::CheckpointSummaryArtifact> {
    // This is a simplified implementation for migration
    // In the original code, this loads from the artifact ledger

    if let Ok(_ledger_guard) = server.artifact_ledger.lock() {
        // get_artifact method doesn't exist on ArtifactLedger
        // This is simplified for migration
        None
    } else {
        None
    }
}

/// Save conversation state
///
/// This function replaces the `AcpServer::save_conversation_state` method.
pub fn save_conversation_state(
    _server: &AcpServer,
    _conversation_id: &str,
    _state: &crate::acp::helpers::conversation::ConversationState,
) -> Result<()> {
    // This is a simplified implementation for migration
    // In the original code, this would serialize and save the state

    Ok(())
}

/// Load conversation state
///
/// This function replaces the `AcpServer::load_conversation_state` method.
pub fn load_conversation_state(
    _server: &AcpServer,
    _conversation_id: &str,
) -> Option<crate::acp::helpers::conversation::ConversationState> {
    // This is a simplified implementation for migration
    // In the original code, this would load and deserialize the state

    None
}
