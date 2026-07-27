//! Web search tool — searches the web using configurable search providers.
//!
//! Uses the `go-on-web-search` crate under the hood, defaulting to the
//! DuckDuckGo Instant Answer API (free, no API key required).

use std::sync::{Arc, OnceLock};

use anyhow::Result;
use go_on_web_search::{SearchProvider, WebSearchClient, WebSearchConfig};
use serde_json::Value;

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};

/// Shared tokio runtime for web search operations.
static WEB_SEARCH_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn web_search_runtime() -> &'static tokio::runtime::Runtime {
    WEB_SEARCH_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build shared web_search runtime")
    })
}

// ---------------------------------------------------------------------------
// WebSearchTool
// ---------------------------------------------------------------------------

/// Searches the web using the configured search provider.
///
/// Input:
/// - `query` (required, string): The search query.
/// - `max_results` (optional, number, default: 5): Maximum number of results.
pub struct WebSearchTool;

impl Tool for WebSearchTool {
    fn name(&self) -> &'static str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web for information. Returns a list of results with titles, URLs, and snippets. Uses DuckDuckGo by default (free, no API key needed)."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query (required)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of search results to return (default: 5)",
                    "default": 5,
                    "minimum": 1,
                    "maximum": 20
                }
            },
            "required": ["query"]
        })
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        // Extract parameters
        let query = input.payload["query"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: 'query'"))?
            .to_string();

        let max_results = input.payload["max_results"]
            .as_u64()
            .unwrap_or(5)
            .clamp(1, 20) as usize;

        // Use the shared blocking runtime to run the async search synchronously.
        // Always uses the dedicated runtime to avoid block_on on an async thread.
        let result = web_search_runtime().block_on(self.search_impl(&query, max_results))?;

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "results": result,
                "total": result.len(),
                "query": query,
            })),
            error: None,
            verification: Some("web_search_completed".to_string()),
            audit_log: Some(format!(
                "web_search: query='{}' returned {} results",
                query,
                result.len()
            )),
            pua_report: Some(tool_execution_report(
                "web_search",
                Some("web_search_completed"),
            )),
        })
    }

    fn run_async(
        self: Arc<Self>,
        input: ToolInput,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolOutput>> + Send>> {
        Box::pin(async move {
            let query = input.payload["query"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing required parameter: 'query'"))?
                .to_string();

            let max_results = input.payload["max_results"]
                .as_u64()
                .unwrap_or(5)
                .clamp(1, 20) as usize;

            let result = self.search_impl(&query, max_results).await?;

            Ok(ToolOutput {
                success: true,
                result: Some(serde_json::json!({
                    "results": result,
                    "total": result.len(),
                    "query": query,
                })),
                error: None,
                verification: Some("web_search_completed".to_string()),
                audit_log: Some(format!(
                    "web_search: query='{}' returned {} results",
                    query,
                    result.len()
                )),
                pua_report: Some(tool_execution_report(
                    "web_search",
                    Some("web_search_completed"),
                )),
            })
        })
    }
}

impl WebSearchTool {
    /// Shared async implementation used by both `run` and `run_async`.
    async fn search_impl(&self, query: &str, max_results: usize) -> Result<Vec<serde_json::Value>> {
        let config = WebSearchConfig {
            provider: SearchProvider::DuckDuckGo,
            timeout_secs: 15,
            max_results,
        };

        let client = WebSearchClient::new(config)?;
        let results = client.search(query, max_results).await?;

        let json_results: Vec<Value> = results
            .into_iter()
            .map(|r| {
                serde_json::json!({
                    "title": r.title,
                    "url": r.url,
                    "snippet": r.snippet,
                })
            })
            .collect();

        Ok(json_results)
    }
}
