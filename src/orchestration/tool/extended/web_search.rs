//! Web search tool — searches the web using configurable search providers.
//!
//! Uses the `go-on-web-search` crate under the hood, defaulting to the
//! DuckDuckGo Instant Answer API (free, no API key required).

use std::sync::{Arc, OnceLock};

use anyhow::Result;
use go_on_web_search::{WebSearchClient, WebSearchConfig};
use serde_json::Value;

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};

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
        // Always uses the dedicated runtime to avoid block_on on an async thread;
        // the guard serializes concurrent sync `run()` calls on the shared
        // current-thread runtime.
        let result = crate::orchestration::tool::exec_common::with_blocking_runtime(|rt| {
            rt.block_on(self.search_impl(&query, max_results))
        })?;

        Ok(build_output(query, result, max_results))
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

            Ok(build_output(query, result, max_results))
        })
    }
}

/// Shared `ToolOutput` construction for both `run` and `run_async` — the two
/// entry points previously duplicated the payload/audit/pua construction.
fn build_output(query: String, result: Vec<serde_json::Value>, _max_results: usize) -> ToolOutput {
    ToolOutput {
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
    }
}

impl WebSearchTool {
    /// Shared async implementation used by both `run` and `run_async`.
    ///
    /// The HTTP client is built once and reused across searches (connection
    /// pooling); only the query and result limit vary per call.
    async fn search_impl(&self, query: &str, max_results: usize) -> Result<Vec<serde_json::Value>> {
        // Client is built once with fixed defaults and reused across searches
        // (connection pooling). It must NOT depend on per-call parameters: the
        // previous `max_results` capture meant the first call fixed the client
        // config globally (and the field is unused by `search()` anyway — the
        // result limit is passed per call). timeout_secs 15 matches the crate
        // default; per-call limits are always honored via `client.search(...)`.
        static CLIENT: OnceLock<Result<WebSearchClient, String>> = OnceLock::new();
        let client = CLIENT
            .get_or_init(|| {
                WebSearchClient::new(WebSearchConfig::default()).map_err(|e| e.to_string())
            })
            .as_ref()
            .map_err(|e| anyhow::anyhow!("web_search client init failed: {}", e))?;
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
