//! Core type definitions for the go-on tool system.
//!
//! Contains `ToolInput`, `ToolOutput`, `RetryPolicy`, `ToolRiskLevel`,
//! `ToolCapabilityProfile`, the `Tool` trait, and `ToolRegistry`.
//!
//! These types are re-exported from `crate::orchestration::tool` for
//! convenience; external code should continue referencing the parent module.

use crate::governance::pua::PuaExecutionReport;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::agents::communication::bus::CommunicationBus;
// Reserved for future AgentCommunicationHook use
// use crate::agents::communication::path::AgentPath;
// use crate::agents::communication::tree::AgentNodeMetadata;

// ---------------------------------------------------------------------------
// Tool lifecycle hooks — observer trait + registry
// ---------------------------------------------------------------------------

/// Observer for tool execution lifecycle events.
///
/// Hooks are invoked during tool dispatch. Sync hooks run on both sync and async
/// paths. Async hooks (`async_pre_execute`) only run on the async path
/// (`run_with_fallback_async`). All methods have default no-op implementations.
#[async_trait]
pub trait ToolHook: Send + Sync {
    /// Called immediately before a tool is executed, after governance checks.
    fn pre_execute(&self, _tool_name: &str, _input: &ToolInput) -> Result<()> {
        Ok(())
    }

    /// Async variant of pre_execute — called by run_with_fallback_async.
    /// Default implementation delegates to the sync pre_execute.
    async fn async_pre_execute(&self, tool_name: &str, input: &ToolInput) -> Result<()> {
        self.pre_execute(tool_name, input)
    }

    /// Called immediately after a tool completes (success or failure).
    fn post_execute(
        &self,
        _tool_name: &str,
        _input: &ToolInput,
        _output: &ToolOutput,
        _duration_ms: u64,
    ) -> Result<()> {
        Ok(())
    }
}

/// A thread-safe collection of `ToolHook` observers.
///
/// Registered hooks are invoked in insertion order. A failing hook logs
/// a warning but does not abort the tool execution pipeline.
#[derive(Default)]
pub struct ToolHookRegistry {
    hooks: std::sync::Mutex<Vec<Arc<dyn ToolHook>>>,
}

// ── BLUE70: AgentCommunicationHook ────────────────────────────────

/// Tool hook that registers spawn events in the CommunicationBus AgentTree.
///
/// When the `spawn_agent` tool is executed, this hook:
/// - Pre-execute: registers the spawned agent in the AgentTree
/// - Post-execute: records execution metrics on the CommunicationBus
pub struct AgentCommunicationHook {
    /// Reference to the global CommunicationBus.
    bus: Arc<CommunicationBus>,
}

impl AgentCommunicationHook {
    /// Create a new hook with a reference to the CommunicationBus.
    pub fn new(bus: Arc<CommunicationBus>) -> Self {
        Self { bus }
    }
}

// ── BLUE71 §11: GuardianHook — async model-based review ─────────────

/// Tool hook that uses GuardianReviewer to review tool actions before execution.
/// Only takes effect on the async tool path (run_with_fallback_async).
pub struct GuardianHook {
    reviewer: std::sync::Arc<crate::governance::guardian::GuardianReviewer>,
}

impl GuardianHook {
    /// Create a new GuardianHook with the given reviewer.
    pub fn new(reviewer: std::sync::Arc<crate::governance::guardian::GuardianReviewer>) -> Self {
        Self { reviewer }
    }
}

#[async_trait]
impl ToolHook for GuardianHook {
    async fn async_pre_execute(&self, tool_name: &str, input: &ToolInput) -> Result<()> {
        let decision = self
            .reviewer
            .review_action(tool_name, input, "tool pre-execution review")
            .await;
        match decision {
            crate::governance::guardian::GuardianDecision::Allow { .. } => Ok(()),
            crate::governance::guardian::GuardianDecision::Deny { reason } => {
                anyhow::bail!("guardian denied: {}", reason)
            }
            crate::governance::guardian::GuardianDecision::EscalateToUser { reason } => {
                anyhow::bail!("guardian escalated: {}", reason)
            }
        }
    }
}

#[async_trait]
impl ToolHook for AgentCommunicationHook {
    fn pre_execute(&self, tool_name: &str, _input: &ToolInput) -> Result<()> {
        if tool_name == "spawn_agent" {
            // Registration happens inside execute_spawn() via the global
            // SPAWN_COMMUNICATION_BUS — this hook provides observability.
            tracing::debug!(tool = tool_name, "AgentCommunicationHook: pre_execute");
        }
        Ok(())
    }

    fn post_execute(
        &self,
        tool_name: &str,
        _input: &ToolInput,
        output: &ToolOutput,
        duration_ms: u64,
    ) -> Result<()> {
        if tool_name == "spawn_agent" {
            self.bus
                .record_metrics(tool_name, duration_ms, output.success);
        }
        Ok(())
    }
}

impl ToolHookRegistry {
    /// Register a new hook. Hooks are invoked in registration order.
    pub fn register(&self, hook: Arc<dyn ToolHook>) {
        if let Ok(mut hooks) = self.hooks.lock() {
            hooks.push(hook);
        }
    }

    /// Invoke all registered pre-execute hooks (sync path).
    pub fn run_pre(&self, tool_name: &str, input: &ToolInput) {
        if let Ok(hooks) = self.hooks.lock() {
            for hook in hooks.iter() {
                if let Err(e) = hook.pre_execute(tool_name, input) {
                    tracing::warn!(tool = %tool_name, error = %e, "pre-execute hook failed");
                }
            }
        }
    }

    /// Invoke all registered pre-execute hooks (async path — calls async_pre_execute).
    /// Returns an error if ANY hook fails (fail-fast: first error stops execution).
    pub async fn run_pre_async(&self, tool_name: &str, input: &ToolInput) -> Result<()> {
        // Clone hooks under lock so we don't hold the lock across await points.
        let hooks: Vec<Arc<dyn ToolHook>> = if let Ok(guard) = self.hooks.lock() {
            guard.clone()
        } else {
            return Ok(());
        };
        for hook in hooks.iter() {
            hook.async_pre_execute(tool_name, input).await?;
        }
        Ok(())
    }

    /// Invoke all registered post-execute hooks.
    pub fn run_post(
        &self,
        tool_name: &str,
        input: &ToolInput,
        output: &ToolOutput,
        duration_ms: u64,
    ) {
        if let Ok(hooks) = self.hooks.lock() {
            for hook in hooks.iter() {
                if let Err(e) = hook.post_execute(tool_name, input, output, duration_ms) {
                    tracing::warn!(tool = %tool_name, error = %e, "post-execute hook failed");
                }
            }
        }
    }
}

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
    /// Tool lifecycle hooks, invoked in registration order.
    pub hooks: ToolHookRegistry,
}
