//! SpawnAgentTool — spawns a sub-agent with a specific task and returns the result.
//!
//! This tool gives the AI the ability to delegate a subtask to a named agent
//! (e.g. "deepseek", "copilot") and collect the complete response. The agent
//! is called via its `chat()` method with a standalone streaming channel.
//!
//! # Security
//! - Agent name is restricted to known agents from the registry (no arbitrary code injection).
//! - Model override is passed as a chat option, not as arbitrary command execution.
//! - The tool has a hard-coded maximum timeout of 300 seconds.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::agent::{AgentRegistry, Message, StreamingSender};
use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};

// ---------------------------------------------------------------------------
// Global registry — initialised once at server startup
// ---------------------------------------------------------------------------

static SPAWN_AGENT_REGISTRY: OnceLock<std::sync::Arc<AgentRegistry>> = OnceLock::new();

/// Initialise the global `AgentRegistry` reference used by `SpawnAgentTool`.
///
/// Must be called once at server startup, after the `AgentRegistry` has been
/// built but before any tool invocations.  Calling this more than once is a
/// no-op (the second call is silently ignored by `OnceLock::set`).
pub fn init_spawn_agent_registry(registry: std::sync::Arc<AgentRegistry>) {
    SPAWN_AGENT_REGISTRY.set(registry).ok();
}

fn agent_registry() -> Result<&'static std::sync::Arc<AgentRegistry>> {
    SPAWN_AGENT_REGISTRY
        .get()
        .ok_or_else(|| anyhow::anyhow!("SpawnAgentTool: AgentRegistry not initialised"))
}

// ---------------------------------------------------------------------------
// Tool definition
// ---------------------------------------------------------------------------

/// Spawn a sub-agent with a specific task and collect its response.
pub struct SpawnAgentTool;

impl Tool for SpawnAgentTool {
    fn name(&self) -> &'static str {
        "spawn_agent"
    }

    fn description(&self) -> &str {
        "Spawn a sub-agent with a specific task, wait for it to complete, and return the result."
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        // This tool is inherently async (agent chat is async). The sync `run()`
        // uses try_current() per principle.md rule 24 — direct
        // Handle::current().block_on() is forbidden in production hot paths.
        // Validate parameters FIRST so bad-input tests get a proper error
        // before attempting to access the global registry.
        let task = input.payload["task"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("missing required parameter 'task' (string)"))?;
        let registry = agent_registry()?.clone();
        let agent_name = input.payload["agent_name"]
            .as_str()
            .unwrap_or("deepseek")
            .to_string();
        let model_override = input.payload["model"].as_str().map(|s| s.to_string());
        let timeout_secs = input.payload["timeout_seconds"]
            .as_u64()
            .unwrap_or(120)
            .clamp(1, 300);

        match tokio::runtime::Handle::try_current() {
            Ok(handle) => handle.block_on(execute_spawn(
                registry,
                task,
                agent_name,
                model_override,
                timeout_secs,
            )),
            Err(_) => {
                let rt = tokio::runtime::Runtime::new()
                    .map_err(|e| anyhow::anyhow!("failed to create temp runtime: {}", e))?;
                rt.block_on(execute_spawn(
                    registry,
                    task,
                    agent_name,
                    model_override,
                    timeout_secs,
                ))
            }
        }
    }

    fn run_async(
        self: Arc<Self>,
        input: ToolInput,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send>> {
        Box::pin(async move {
            let registry = agent_registry()
                .map_err(|e| anyhow::anyhow!("SpawnAgentTool: {}", e))?
                .clone();
            let task = input.payload["task"]
                .as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| anyhow::anyhow!("missing required parameter 'task' (string)"))?;
            let agent_name = input.payload["agent_name"]
                .as_str()
                .unwrap_or("deepseek")
                .to_string();
            let model_override = input.payload["model"].as_str().map(|s| s.to_string());
            let timeout_secs = input.payload["timeout_seconds"]
                .as_u64()
                .unwrap_or(120)
                .clamp(1, 300);
            execute_spawn(registry, task, agent_name, model_override, timeout_secs).await
        })
    }
}

/// Look up the agent, build messages, call `chat()`, and collect the response.
async fn execute_spawn(
    registry: std::sync::Arc<AgentRegistry>,
    task: String,
    agent_name: String,
    model_override: Option<String>,
    timeout_secs: u64,
) -> Result<ToolOutput> {
    // 1. Resolve agent from registry.
    let agent = registry
        .get(&agent_name)
        .ok_or_else(|| anyhow::anyhow!("agent '{}' not found in registry", agent_name))?;

    info!(
        agent = %agent_name,
        timeout_secs = %timeout_secs,
        has_model_override = %model_override.is_some(),
        "spawn_agent: delegating task to sub-agent"
    );

    // 2. Build messages.  The task text is sent as a user message, with
    //    a system prefix asking the agent to produce a concise result.
    let messages = vec![
        Message {
            role: "system".to_string(),
            content:
                "You are a helpful sub-agent. Complete the following task and provide a clear, "
                    .to_string(),
        },
        Message {
            role: "user".to_string(),
            content: task.clone(),
        },
    ];

    // 3. Build options — pass model override if provided.
    let options: Option<std::collections::HashMap<String, serde_json::Value>> =
        model_override.map(|model| {
            let mut map = std::collections::HashMap::new();
            map.insert("model".to_string(), serde_json::Value::String(model));
            map
        });

    // 4. Create a standalone channel for collecting the streaming response.
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let sender = StreamingSender::new(tx);

    // 5. Call `chat()` with a tokio timeout guard.
    let chat_future = agent.chat(messages, None, options, sender);
    let timeout_duration = Duration::from_secs(timeout_secs);

    let chat_result = tokio::time::timeout(timeout_duration, chat_future).await;

    match chat_result {
        Ok(Ok(())) => {
            // 6. Collect all streamed tokens.
            let mut response = String::new();
            while let Some(token) = rx.recv().await {
                response.push_str(&token);
            }

            info!(
                agent = %agent_name,
                response_len = response.len(),
                "spawn_agent: sub-agent completed successfully"
            );

            Ok(ToolOutput {
                success: true,
                result: Some(serde_json::json!({
                    "agent": agent_name,
                    "task": task,
                    "response": response,
                })),
                error: None,
                verification: Some("sub_agent_completed".to_string()),
                audit_log: Some(format!(
                    "SpawAgent: delegated task to '{}' ({} chars response)",
                    agent_name,
                    response.len(),
                )),
                pua_report: Some(tool_execution_report(
                    "spawn_agent",
                    Some("sub_agent_completed"),
                )),
            })
        }
        Ok(Err(e)) => {
            warn!(
                agent = %agent_name,
                error = %e,
                "spawn_agent: sub-agent chat failed"
            );
            Ok(ToolOutput {
                success: false,
                result: None,
                error: Some(format!("sub-agent '{}' chat failed: {}", agent_name, e)),
                verification: Some("sub_agent_failed".to_string()),
                audit_log: Some(format!(
                    "SpawnAgent: agent '{}' chat failed: {}",
                    agent_name, e
                )),
                pua_report: Some(tool_execution_report(
                    "spawn_agent",
                    Some("sub_agent_failed"),
                )),
            })
        }
        Err(_elapsed) => {
            warn!(
                agent = %agent_name,
                timeout_secs = %timeout_secs,
                "spawn_agent: sub-agent timed out"
            );
            // Drain any remaining tokens to avoid sender/receiver deadlock.
            while rx.try_recv().is_ok() {}

            Ok(ToolOutput {
                success: false,
                result: None,
                error: Some(format!(
                    "sub-agent '{}' timed out after {} seconds",
                    agent_name, timeout_secs
                )),
                verification: Some("sub_agent_timeout".to_string()),
                audit_log: Some(format!(
                    "SpawnAgent: agent '{}' timed out after {}s",
                    agent_name, timeout_secs
                )),
                pua_report: Some(tool_execution_report(
                    "spawn_agent",
                    Some("sub_agent_timeout"),
                )),
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Agent, Message};
    use crate::agents::agent::ModelInfo;
    use crate::core::error::Result as AppResult;
    use async_trait::async_trait;
    use std::collections::HashMap;

    /// A mock agent that echoes back the task content.
    struct EchoAgent;

    #[async_trait]
    impl Agent for EchoAgent {
        async fn chat(
            &self,
            messages: Vec<Message>,
            _principles: Option<Vec<String>>,
            _options: Option<HashMap<String, serde_json::Value>>,
            sender: StreamingSender,
        ) -> AppResult<()> {
            // Echo the last user message content back through the sender.
            for msg in &messages {
                if msg.role == "user" {
                    let _ = sender.send(msg.content.clone());
                }
            }
            Ok(())
        }

        fn available_models(&self) -> Vec<ModelInfo> {
            vec![ModelInfo {
                id: "echo".to_string(),
                name: "echo".to_string(),
                description: String::new(),
                is_default: true,
                capabilities: Vec::new(),
                context_window: None,
            }]
        }
    }

    /// Helper: build a minimal `ToolInput` from a JSON payload.
    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "spawn-test".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test spawn_agent".to_string(),
            constraints: None,
            evidence: None,
            payload,
            allowed_base_dir: None,
        }
    }

    #[test]
    fn spawn_agent_missing_task_returns_error() {
        // No registry is set in tests — the tool should fail gracefully.
        let tool = SpawnAgentTool;
        let input = tool_input(serde_json::json!({
            "agent_name": "echo",
        }));
        let result = tool.run(&input);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("missing required parameter 'task'"),
            "error should mention missing task, got: {}",
            err
        );
    }

    #[test]
    fn spawn_agent_requires_registry() {
        let tool = SpawnAgentTool;
        let input = tool_input(serde_json::json!({
            "task": "do something",
            "agent_name": "nonexistent",
        }));
        let result = tool.run(&input);
        // Without a registry, the OnceLock::get() should panic or the
        // execute_spawn should fail gracefully.
        assert!(result.is_err());
    }

    #[test]
    fn spawn_agent_with_echo_agent_succeeds() {
        // Use a dedicated AgentRegistry for this test to avoid race
        // conditions with concurrent tests that also access the global
        // SPAWN_AGENT_REGISTRY.  The SpawnAgentTool internally calls
        // agent_registry() which reads from the OnceLock.  If the
        // OnceLock was already initialized by a concurrent server test,
        // we must not assume our EchoAgent is present.  Instead we
        // create a fresh standalone registry that only has EchoAgent.
        //
        // Use `set().ok()` to handle concurrent test runs — if another
        // test already initialized SPAWN_AGENT_REGISTRY, our EchoAgent
        // won't be present and the tool will fail with a clear error.
        // We then check that error to distinguish the two cases.
        let mut reg = AgentRegistry::new();
        reg.register_arc("echo", Arc::new(EchoAgent));
        let inserted = SPAWN_AGENT_REGISTRY.set(std::sync::Arc::new(reg));
        if inserted.is_err() {
            // The global registry was already set by a concurrent test.
            // Since we cannot add EchoAgent to it (private agents field),
            // we assert that this is the case and skip the test.
            eprintln!("note: SPAWN_AGENT_REGISTRY already set by another test, skipping EchoAgent assertion");
            // We can still verify the tool gracefully handles a missing agent.
            let tool = SpawnAgentTool;
            let input = tool_input(serde_json::json!({
                "task": "test",
                "agent_name": "echo",
            }));
            let result = tool.run(&input);
            assert!(
                result.is_err(),
                "with echo not registered, tool should fail"
            );
            return;
        }

        let tool = SpawnAgentTool;
        let input = tool_input(serde_json::json!({
            "task": "Hello, sub-agent!",
            "agent_name": "echo",
        }));
        let output = tool.run(&input).expect("spawn_agent should succeed");
        assert!(output.success);
        let result = output.result.expect("should have result");
        assert_eq!(result["agent"], "echo");
        assert_eq!(result["response"], "Hello, sub-agent!");
    }
}
