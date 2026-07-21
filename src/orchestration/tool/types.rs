//! Core type definitions for the go-on tool system.
//!
//! Contains `ToolInput`, `ToolOutput`, `RetryPolicy`, `ToolRiskLevel`,
//! `ToolCapabilityProfile`, the `Tool` trait, and `ToolRegistry`.
//!
//! These types are re-exported from `crate::orchestration::tool` for
//! convenience; external code should continue referencing the parent module.

use crate::governance::pua::PuaExecutionReport;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Tool input envelope
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInput {
    pub task_id: String,
    pub phase: String,
    pub agent_role: String,
    pub objective: String,
    pub constraints: Option<String>,
    pub evidence: Option<String>,
    pub payload: serde_json::Value,
    pub allowed_base_dir: Option<PathBuf>,
}

/// Tool output envelope
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub success: bool,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
    pub verification: Option<String>,
    pub audit_log: Option<String>,
    pub pua_report: Option<PuaExecutionReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryPolicy {
    pub max_retries: usize,
    pub retry_on_failure: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRiskLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCapabilityProfile {
    pub capability: String,
    pub risk_level: ToolRiskLevel,
    pub timeout_budget_ms: u64,
    pub retry_policy: RetryPolicy,
    pub fallback_chain: Vec<String>,
}

/// Tool trait
///
/// All tools must implement this trait. The `run` method should be instrumented
/// for tracing and performance monitoring in the implementation, not on the
/// trait itself.
pub trait Tool: Send + Sync + 'static {
    /// Returns the tool's unique name.
    fn name(&self) -> &'static str;

    /// Returns a human-readable description of what this tool does.
    /// Override this to provide rich descriptions for LLM function-calling schemas.
    fn description(&self) -> &str {
        ""
    }

    /// Returns the JSON Schema for this tool's input parameters.
    /// Used when building OpenAI/Anthropic-compatible function-calling schemas.
    ///
    /// Default implementation delegates to `tool_descriptor()` in
    /// `shared::tool_descriptors`, which has schemas for all built-in tools.
    /// Individual tools CAN override this for custom schemas, but most
    /// should rely on the shared tool_descriptor definitions.
    fn input_schema(&self) -> serde_json::Value {
        let desc = crate::shared::tool_descriptors::tool_descriptor_value(self.name());
        desc.get("input_schema")
            .or_else(|| desc.get("inputSchema"))
            .cloned()
            .unwrap_or_else(|| {
                serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                })
            })
    }

    /// Executes the tool with the given input. Should emit tracing spans for
    /// performance analysis (implementations only).
    fn run(&self, input: &ToolInput) -> Result<ToolOutput>;

    /// Async variant of `run` for non-blocking execution in async contexts.
    /// The default implementation offloads the synchronous `run` call to
    /// `tokio::task::spawn_blocking`, which moves the work off the async
    /// runtime worker thread and onto the blocking thread pool.
    ///
    /// I/O-bound tools SHOULD override this method with a fully async
    /// implementation for optimal performance.
    fn run_async(
        self: Arc<Self>,
        input: ToolInput,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolOutput>> + Send>> {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || self.run(&input))
                .await
                .map_err(|e| anyhow::anyhow!("tool blocking task failed: {}", e))?
        })
    }
}

/// Tool registry
pub struct ToolRegistry {
    pub(crate) tools: Vec<Arc<dyn Tool>>,
    pub(crate) profiles: HashMap<&'static str, ToolCapabilityProfile>,
    /// Alias map: alias → canonical tool name.
    /// Allows looking up tools by alternative names
    /// (e.g. "terminal" → "shell_exec").
    pub(crate) aliases: HashMap<&'static str, &'static str>,
}
