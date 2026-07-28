//! Tool search tool
//!
//! Enables the AI model to discover Deferred tools at runtime by searching
//! the global ToolRegistry for tools whose names or descriptions match a query.
//! This is the companion to the ToolExposure mechanism (Direct/Deferred/Hidden).

use crate::orchestration::tool::{Tool, ToolExposure, ToolInput, ToolOutput};
use anyhow::Result;

/// Tool that searches for available tools by name or description.
///
/// This is always Direct-exposed so the model can discover Deferred tools.
/// Takes a `query` string and optional `top_k` parameter.
pub struct ToolSearchTool;

impl Tool for ToolSearchTool {
    fn name(&self) -> &'static str {
        "tool_search"
    }

    fn description(&self) -> &str {
        "Search for available tools by name or description. Use this to discover niche or specialized tools that are not shown in the default tool list."
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let query = input
            .payload
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();

        let top_k = input
            .payload
            .get("top_k")
            .and_then(|v| v.as_i64())
            .unwrap_or(8)
            .min(20)
            .max(1) as usize;

        // Access the global tool registry to discover deferred tools.
        let registry = crate::acp::r#impl::request::tools_pack::global_tool_registry();

        // Collect deferred tool names and score them.
        let mut scored: Vec<(i32, serde_json::Value)> = Vec::new();

        for tool_name in registry.deferred_tool_names() {
            let desc = crate::shared::tool_descriptors::tool_descriptor(tool_name);
            let name_lower = tool_name.to_lowercase();
            let desc_text = desc.description.as_deref().unwrap_or("").to_lowercase();

            // Simple keyword matching: count occurrences of query terms.
            let mut score: i32 = 0;

            // High score for exact name match.
            if name_lower == query {
                score += 100;
            }

            // Medium score for name containing query.
            if name_lower.contains(&query) {
                score += 50;
            }

            // Low score for description containing query.
            // Also check individual words in the query.
            let query_words: Vec<&str> = query.split_whitespace().collect();
            for word in &query_words {
                if !word.is_empty() {
                    if name_lower.contains(word) {
                        score += 30;
                    }
                    if desc_text.contains(word) {
                        score += 10;
                    }
                }
            }

            if score > 0 {
                scored.push((
                    -score, // Negative for ascending sort (highest score first).
                    serde_json::json!({
                        "name": tool_name,
                        "description": desc.description,
                        "exposure": "deferred",
                    }),
                ));
            }
        }

        // Sort by score descending (negated for ascending sort on negative score).
        scored.sort_by_key(|(s, _)| *s);

        let results: Vec<serde_json::Value> =
            scored.into_iter().take(top_k).map(|(_, v)| v).collect();

        let result = serde_json::json!({
            "results": results,
            "total": results.len(),
        });

        Ok(ToolOutput {
            success: true,
            result: Some(result),
            error: None,
            verification: None,
            audit_log: None,
            pua_report: None,
        })
    }
}
