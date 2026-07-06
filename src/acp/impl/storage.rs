//! Storage implementation functions for ACP server

use crate::cache::ResponseCache;
use anyhow::Result;
use std::sync::Arc;
use tokio::task::spawn_blocking;

pub async fn cache_clear(cache: Arc<ResponseCache>) -> Result<usize> {
    spawn_blocking(move || cache.clear_all())
        .await
        .map_err(|e| anyhow::anyhow!("cache_clear join error: {}", e))?
}
