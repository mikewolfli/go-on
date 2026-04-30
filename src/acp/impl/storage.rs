//! Storage implementation functions for ACP server

use crate::acp::server::AcpServer;
use crate::cache::ResponseCache;
use anyhow::Result;
use std::sync::Arc;
use tokio::task::spawn_blocking;

pub async fn cache_clear(_server: &AcpServer, cache: Arc<ResponseCache>) -> Result<usize> {
    spawn_blocking(move || cache.clear_all())
        .await
        .map_err(|e| anyhow::anyhow!("cache_clear join error: {}", e))?
}
