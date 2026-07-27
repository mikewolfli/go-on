//! Storage implementation functions for ACP server

use crate::cache::ResponseCache;
use anyhow::Result;
use std::sync::Arc;

/// Clear the cache.
pub async fn cache_clear(cache: Arc<ResponseCache>) -> Result<usize> {
    cache.clear_all().await
}
