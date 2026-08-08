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

use futures_util::future::join_all;

// ---------------------------------------------------------------------------
// Tool lifecycle hooks — observer trait + registry
// ---------------------------------------------------------------------------

/// Observer for tool execution lifecycle events.
///
/// Hooks are invoked during tool dispatch. All execution paths (ACP autonomy
/// loop, MCP `tools/call`, ACP bridge, CLI) run the async pre-execute chain
/// (`run_pre_async`), which invokes every hook's `async_pre_execute`. Sync
/// hooks are covered via the default delegation of `async_pre_execute` to
/// `pre_execute`, so async hooks such as `GuardianHook` run on every path.
/// All methods have default no-op implementations.
///
/// The `Any` supertrait enables registry introspection (e.g. governance status
/// probing whether a `GuardianHook` is registered) via downcasting.
#[async_trait]
pub trait ToolHook: Send + Sync + std::any::Any {
    /// Called immediately before a tool is executed, after governance checks.
    fn pre_execute(&self, _tool_name: &str, _input: &ToolInput) -> Result<()> {
        Ok(())
    }

    /// Async variant of pre_execute — called by `run_pre_async` on every
    /// execution path. Default implementation delegates to the sync
    /// pre_execute.
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

// ── BLUE71 §11: GuardianHook — async model-based tool review ───────────

/// Tool hook that uses GuardianReviewer to review tool actions before execution.
/// Runs on every tool execution path via `run_pre_async` (sync-only hooks are
/// covered by the trait's default delegation, but this hook overrides
/// `async_pre_execute` so the model review always fires).
/// Activated via config: `guardian_enabled = true` + `guardian_agent = "..."`
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

impl ToolHookRegistry {
    /// Register a new hook. Hooks are invoked in registration order.
    pub fn register(&self, hook: Arc<dyn ToolHook>) {
        if let Ok(mut hooks) = self.hooks.lock() {
            hooks.push(hook);
        }
    }

    /// Returns the number of registered hooks.
    pub fn len(&self) -> usize {
        self.hooks.lock().map(|g| g.len()).unwrap_or(0)
    }

    /// Returns true if no hooks are registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether a `GuardianHook` (BLUE71 §11 model-based tool review) is
    /// registered. Used by the governance status probe so the guardian module
    /// reports its true runtime state instead of the config flag alone.
    pub fn has_guardian(&self) -> bool {
        self.hooks
            .lock()
            .map(|hooks| {
                hooks
                    .iter()
                    .any(|hook| hook.type_id() == std::any::TypeId::of::<GuardianHook>())
            })
            .unwrap_or(false)
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
    /// Hooks that are independent are executed in parallel for lower latency.
    /// Returns an error if ANY hook fails (fail-fast: first error stops execution).
    pub async fn run_pre_async(&self, tool_name: &str, input: &ToolInput) -> Result<()> {
        // Clone hooks under lock so we don't hold the lock across await points.
        let hooks: Vec<Arc<dyn ToolHook>> = if let Ok(guard) = self.hooks.lock() {
            guard.clone()
        } else {
            return Ok(());
        };
        if hooks.is_empty() {
            return Ok(());
        }
        // Execute all hooks in parallel and collect errors.
        // This significantly reduces latency compared to serial execution
        // when multiple hooks (e.g. audit, metrics) are registered.
        let results: Vec<Result<()>> = join_all(
            hooks
                .iter()
                .map(|hook| hook.async_pre_execute(tool_name, input)),
        )
        .await;
        // First error fails fast — maintains existing contract.
        for result in results {
            result?;
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

/// Controls where a tool is exposed to the model.
///
/// - `Direct`: included in the initial model-visible tool list (default).
/// - `Deferred`: registered but omitted from initial list; discoverable via search.
/// - `Hidden`: registered for dispatch only, never exposed to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ToolExposure {
    /// Include this tool in the initial model-visible tool list.
    #[default]
    Direct,
    /// Register this tool for later discovery, but omit it from the initial
    /// model-visible tool list. The model must use tool_search to find it.
    Deferred,
    /// Keep this tool registered for dispatch without exposing it to the model.
    Hidden,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
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

    /// Returns how this tool should be exposed to the AI model.
    /// Override to return `Deferred` or `Hidden` for niche/system tools.
    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
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
    /// Tools stored in a HashMap keyed by name for O(1) lookup.
    pub(crate) tools: HashMap<&'static str, Arc<dyn Tool>>,
    pub(crate) profiles: HashMap<&'static str, ToolCapabilityProfile>,
    /// Alias map: alias → canonical tool name.
    /// Allows looking up tools by alternative names
    /// (e.g. "terminal" → "shell_exec").
    pub(crate) aliases: HashMap<&'static str, &'static str>,
    /// Tool lifecycle hooks, invoked in registration order.
    pub hooks: ToolHookRegistry,
}
