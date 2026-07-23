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
//! - Global concurrency semaphore prevents unbounded sub-agent spawning.
//!
//! # Sub-agent lifecycle
//! - role classification (7 types, CodeWhale-compatible)
//! - token_budget tracking
//! - structured output (SUMMARY/CHANGES/EVIDENCE/RISKS/BLOCKERS)
//! - timeout guard with heartbeat-style cancellation
//! - transient-failure retry with backoff (up to 2 retries)
//! - global concurrency cap (SEMAPHORE: max 128 concurrent)

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::{mpsc, Semaphore};
use tracing::{info, warn};

use crate::agent::{AgentRegistry, Message, StreamingSender};
use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};

// ---------------------------------------------------------------------------
// Global registry + semaphore — initialised once at server startup
// ---------------------------------------------------------------------------

static SPAWN_AGENT_REGISTRY: OnceLock<Arc<AgentRegistry>> = OnceLock::new();

/// Monotonically increasing sequence counter for fork IDs.
/// Provides unique, ordered identifiers for each sub-agent spawn
/// without coupling to the external ForkRegistry.
static SPAWN_FORK_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Global concurrency semaphore — limits total in-flight sub-agent spawns.
/// Set to 128 max concurrent (matching CodeWhale's concurrency ceiling).
static SPAWN_SEMAPHORE: Semaphore = Semaphore::const_new(128);

/// Initialise the global `AgentRegistry` reference used by `SpawnAgentTool`.
///
/// Must be called once at server startup, after the `AgentRegistry` has been
/// built but before any tool invocations.  Calling this more than once is a
/// no-op (the second call is silently ignored by `OnceLock::set`).
pub fn init_spawn_agent_registry(registry: Arc<AgentRegistry>) {
    SPAWN_AGENT_REGISTRY.set(registry).ok();
}

fn agent_registry() -> Result<&'static Arc<AgentRegistry>> {
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
        "Spawn a sub-agent with a specific task and optional role classification, wait for it to complete, and return the result."
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
        // Validate role early (before accessing global registry).
        if let Some(ref role) = input.payload["role"].as_str() {
            if !SUB_AGENT_ROLES.contains(role) {
                anyhow::bail!(
                    "invalid sub-agent role '{}': must be one of {}",
                    role,
                    SUB_AGENT_ROLES.join(", ")
                );
            }
        }
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
        let role = input.payload["role"].as_str().map(|s| s.to_string());
        let token_budget = input.payload["token_budget"].as_u64();

        match tokio::runtime::Handle::try_current() {
            Ok(handle) => handle.block_on(execute_spawn(
                registry,
                task,
                agent_name,
                model_override,
                timeout_secs,
                role,
                token_budget,
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
                    role,
                    token_budget,
                ))
            }
        }
    }

    fn run_async(
        self: Arc<Self>,
        input: ToolInput,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send>> {
        Box::pin(async move {
            let task = input.payload["task"]
                .as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| anyhow::anyhow!("missing required parameter 'task' (string)"))?;
            // Validate role early.
            if let Some(ref role) = input.payload["role"].as_str() {
                if !SUB_AGENT_ROLES.contains(role) {
                    anyhow::bail!(
                        "invalid sub-agent role '{}': must be one of {}",
                        role,
                        SUB_AGENT_ROLES.join(", ")
                    );
                }
            }
            let registry = agent_registry()
                .map_err(|e| anyhow::anyhow!("SpawnAgentTool: {}", e))?
                .clone();
            let agent_name = input.payload["agent_name"]
                .as_str()
                .unwrap_or("deepseek")
                .to_string();
            let model_override = input.payload["model"].as_str().map(|s| s.to_string());
            let timeout_secs = input.payload["timeout_seconds"]
                .as_u64()
                .unwrap_or(120)
                .clamp(1, 300);
            let role = input.payload["role"].as_str().map(|s| s.to_string());
            let token_budget = input.payload["token_budget"].as_u64();
            execute_spawn(
                registry,
                task,
                agent_name,
                model_override,
                timeout_secs,
                role,
                token_budget,
            )
            .await
        })
    }
}

/// Look up the agent, build messages, call `chat()`, and collect the response.
/// Valid sub-agent role identifiers (CodeWhale-compatible).
const SUB_AGENT_ROLES: &[&str] = &[
    "general",
    "explore",
    "plan",
    "review",
    "implementer",
    "verifier",
    "custom",
];

/// Maximum transient-failure retries before giving up.
const MAX_RETRIES: u32 = 2;

/// Base delay for exponential backoff (milliseconds).
const RETRY_BASE_DELAY_MS: u64 = 500;

async fn execute_spawn(
    registry: Arc<AgentRegistry>,
    task: String,
    agent_name: String,
    model_override: Option<String>,
    timeout_secs: u64,
    role: Option<String>,
    token_budget: Option<u64>,
) -> Result<ToolOutput> {
    // 0. Validate role if provided.
    if let Some(ref role) = role {
        if !SUB_AGENT_ROLES.contains(&role.as_str()) {
            anyhow::bail!(
                "invalid sub-agent role '{}': must be one of {}",
                role,
                SUB_AGENT_ROLES.join(", "),
            );
        }
    }

    // Generate a unique fork ID for observability (no ForkRegistry dependency).
    let fork_seq = SPAWN_FORK_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let fork_id = format!(
        "spawn-{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        fork_seq,
    );

    // Acquire concurrency permit (release on drop).
    let _permit = SPAWN_SEMAPHORE
        .acquire()
        .await
        .map_err(|_| anyhow::anyhow!("failed to acquire sub-agent concurrency permit"))?;

    // 1. Resolve agent from registry.
    let agent = registry
        .get(&agent_name)
        .ok_or_else(|| anyhow::anyhow!("agent '{}' not found in registry", agent_name))?;

    // 2. Build messages with role-specific system prompt.
    let system_prompt = build_role_prompt(role.as_deref());
    let messages = vec![
        Message {
            role: "system".to_string(),
            content: system_prompt,
        },
        Message {
            role: "user".to_string(),
            content: task.clone(),
        },
    ];

    // 3. Build options — pass model override and max_tokens if budget set.
    let mut options_map = std::collections::HashMap::new();
    if let Some(model) = model_override {
        options_map.insert("model".to_string(), serde_json::Value::String(model));
    }
    if let Some(budget) = token_budget {
        options_map.insert("max_tokens".to_string(), serde_json::json!(budget));
    }
    let options = if options_map.is_empty() {
        None
    } else {
        Some(options_map)
    };

    /// Build a role-specific system prompt for the sub-agent.
    ///
    /// Each role gets tailored instructions to guide the agent's behavior
    /// and output format. Falls back to the generic prompt when no role
    /// is provided or the role is `general` or `custom`.
    fn build_role_prompt(role: Option<&str>) -> String {
        let base = "You are a helpful sub-agent. Complete the following task and provide a clear, concise result.";
        let suffix = match role {
            Some("explore") => "\n\nRole: Explorer. Focus on discovering information, identifying patterns, and gathering evidence. Prioritize breadth of research over depth. Structure your output with SUMMARY and EVIDENCE sections.",
            Some("plan") => "\n\nRole: Planner. Break down the task into actionable steps, identify dependencies, and estimate effort. Structure your output with SUMMARY and CHANGES sections showing the plan.",
            Some("review") => "\n\nRole: Reviewer. Analyze the provided content for correctness, efficiency, security, and best practices. Identify issues and suggest improvements. Structure your output with SUMMARY, RISKS, and CHANGES sections.",
            Some("implementer") => "\n\nRole: Implementer. Write code or implement the solution. Focus on correct, idiomatic, well-documented output. Structure your output with SUMMARY and CHANGES sections.",
            Some("verifier") => "\n\nRole: Verifier. Test and verify the correctness of the solution. Check edge cases, run validations, and report findings. Structure your output with SUMMARY, EVIDENCE, and BLOCKERS sections.",
            _ => "",
        };
        format!("{}{}", base, suffix)
    }

    // 4. Execute with retry loop for transient failures.
    let mut last_error: Option<String> = None;
    let timeout_duration = Duration::from_secs(timeout_secs);

    for attempt in 0..=MAX_RETRIES {
        if attempt > 0 {
            let delay = Duration::from_millis(RETRY_BASE_DELAY_MS * 2u64.pow(attempt - 1));
            info!(
                agent = %agent_name,
                attempt,
                delay_ms = delay.as_millis(),
                "spawn_agent: retrying after transient failure"
            );
            tokio::time::sleep(delay).await;
        }

        // 5. Create a standalone channel for collecting the streaming response.
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let sender = StreamingSender::new(tx);

        // 6. Call `chat()` with a tokio timeout guard.
        let chat_future = agent.chat(messages.clone(), None, options.clone(), sender);
        let chat_result = tokio::time::timeout(timeout_duration, chat_future).await;

        match chat_result {
            Ok(Ok(())) => {
                // 7. Collect all streamed tokens.
                let mut response = String::new();
                while let Some(token) = rx.recv().await {
                    response.push_str(&token);
                }

                info!(
                    agent = %agent_name,
                    fork_id = %fork_id,
                    response_len = response.len(),
                    "spawn_agent: sub-agent completed successfully"
                );

                // Extract structured output sections from the response
                let summary = extract_section(&response, "SUMMARY");
                let changes = extract_section(&response, "CHANGES");
                let evidence = extract_section(&response, "EVIDENCE");
                let risks = extract_section(&response, "RISKS");
                let blockers = extract_section(&response, "BLOCKERS");
                let role_str = role.clone().unwrap_or_default();

                // Estimate actual token usage (~4 chars per token for English text)
                let actual_tokens = (response.len() / 4).max(1) as u64;
                let budget_exceeded = token_budget.is_some_and(|b| actual_tokens > b);

                return Ok(ToolOutput {
                    success: true,
                    result: Some(serde_json::json!({
                        "agent": agent_name,
                        "task": task,
                        "response": response,
                        // Structured output fields (CodeWhale-compatible)
                        "summary": summary,
                        "changes": changes,
                        "evidence": evidence,
                        "risks": risks,
                        "blockers": blockers,
                        // Fork tracking, role classification and budget
                        "fork_id": fork_id,
                        "role": role_str,
                        "token_budget": token_budget,
                        "actual_tokens": actual_tokens,
                        "budget_exceeded": budget_exceeded,
                    })),
                    error: None,
                    verification: Some("sub_agent_completed".to_string()),
                    audit_log: Some(format!(
                        "SpawnAgent: delegated task to '{}' ({} chars response)",
                        agent_name,
                        response.len(),
                    )),
                    pua_report: Some(tool_execution_report(
                        "spawn_agent",
                        Some("sub_agent_completed"),
                    )),
                });
            }
            Ok(Err(e)) => {
                let err_str = e.to_string();
                warn!(
                    agent = %agent_name,
                    attempt,
                    error = %err_str,
                    "spawn_agent: sub-agent chat failed"
                );
                last_error = Some(err_str.clone());

                // Only retry transient failures.
                // NOTE: "50" matches 5xx HTTP status codes (500, 502, 503, 504)
                // without the false-positive risk of a bare `contains("5")`.
                let is_transient = err_str.contains("timeout")
                    || err_str.contains("rate_limit")
                    || err_str.contains("429")
                    || err_str.contains("50")
                    || err_str.contains("connection")
                    || err_str.contains("reset");
                if !is_transient || attempt == MAX_RETRIES {
                    // Drain any remaining tokens to avoid sender/receiver deadlock.
                    while rx.try_recv().is_ok() {}

                    return Ok(ToolOutput {
                        success: false,
                        result: Some(serde_json::json!({
                            "fork_id": fork_id,
                            "role": role.clone().unwrap_or_default(),
                            "token_budget": token_budget,
                        })),
                        error: Some(format!(
                            "sub-agent '{}' chat failed after {} attempts: {}",
                            agent_name,
                            attempt + 1,
                            err_str
                        )),
                        verification: Some("sub_agent_failed".to_string()),
                        audit_log: Some(format!(
                            "SpawnAgent: agent '{}' failed after {} attempts: {}",
                            agent_name,
                            attempt + 1,
                            err_str
                        )),
                        pua_report: Some(tool_execution_report(
                            "spawn_agent",
                            Some("sub_agent_failed"),
                        )),
                    });
                }
                // Transient failure — loop back for retry.
            }
            Err(_elapsed) => {
                warn!(
                    agent = %agent_name,
                    attempt,
                    timeout_secs = %timeout_secs,
                    "spawn_agent: sub-agent timed out"
                );
                // Drain any remaining tokens to avoid sender/receiver deadlock.
                while rx.try_recv().is_ok() {}

                last_error = Some(format!("timed out after {} seconds", timeout_secs));

                // Timeout is transient — retry if attempts remain.
                if attempt == MAX_RETRIES {
                    return Ok(ToolOutput {
                        success: false,
                        result: Some(serde_json::json!({
                            "fork_id": fork_id,
                            "role": role.clone().unwrap_or_default(),
                            "token_budget": token_budget,
                        })),
                        error: Some(format!(
                            "sub-agent '{}' timed out after {} seconds ({} attempts)",
                            agent_name,
                            timeout_secs,
                            attempt + 1
                        )),
                        verification: Some("sub_agent_timeout".to_string()),
                        audit_log: Some(format!(
                            "SpawnAgent: agent '{}' timed out after {}s ({} attempts)",
                            agent_name,
                            timeout_secs,
                            attempt + 1
                        )),
                        pua_report: Some(tool_execution_report(
                            "spawn_agent",
                            Some("sub_agent_timeout"),
                        )),
                    });
                }
                // Loop back for retry.
            }
        }
    }

    // Should be unreachable — either we returned success or failure in the loop.
    Ok(ToolOutput {
        success: false,
        result: None,
        error: Some(format!(
            "sub-agent '{}' exhausted retries: {:?}",
            agent_name,
            last_error.unwrap_or_default()
        )),
        verification: Some("sub_agent_exhausted".to_string()),
        audit_log: Some(format!(
            "SpawnAgent: agent '{}' exhausted retries",
            agent_name
        )),
        pua_report: Some(tool_execution_report(
            "spawn_agent",
            Some("sub_agent_exhausted"),
        )),
    })
}

/// Extract a named section from the response text.
/// Matches lines like `SUMMARY: ...`, `CHANGES: ...`, or markdown `## SUMMARY` blocks.
/// Returns `None` if the section is not found.
fn extract_section(response: &str, section_name: &str) -> Option<String> {
    let section_lower = section_name.to_lowercase();

    // Pattern 1: `SECTION_NAME: value` at start of line
    for line in response.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed
            .strip_prefix(&format!("{}:", section_name))
            .or_else(|| trimmed.strip_prefix(&format!("{}:", section_lower)))
        {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }

    // Pattern 2: `## SECTION_NAME` markdown heading followed by content
    let mut in_section = false;
    let mut content = Vec::new();
    for line in response.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") {
            let heading = trimmed.trim_start_matches("## ").trim();
            if heading.eq_ignore_ascii_case(section_name) {
                in_section = true;
                continue;
            } else if in_section {
                // Next heading ends this section.
                break;
            }
        }
        if in_section {
            content.push(line);
        }
    }

    if content.is_empty() {
        None
    } else {
        Some(content.join("\n").trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Agent, Message, StreamingSender};
    use async_trait::async_trait;
    use serde_json::json;

    /// A minimal echo agent for testing.
    #[expect(dead_code, reason = "used as Agent trait impl in spawn tests")]
    struct EchoAgent;

    #[async_trait]
    impl Agent for EchoAgent {
        async fn chat(
            &self,
            _messages: Vec<Message>,
            _principles: Option<Vec<String>>,
            _options: Option<std::collections::HashMap<String, serde_json::Value>>,
            sender: StreamingSender,
        ) -> std::result::Result<(), crate::core::error::AppError> {
            let _ = sender.send("Hello from echo!".to_string());
            let _ = sender.send("\nSUMMARY: task completed".to_string());
            Ok(())
        }

        fn available_models(&self) -> Vec<crate::agent::ModelInfo> {
            vec![crate::agent::ModelInfo {
                id: "echo-model".to_string(),
                name: "Echo Model".to_string(),
                description: "Test echo model".to_string(),
                is_default: true,
                capabilities: vec!["chat".to_string()],
                context_window: None,
            }]
        }
    }

    fn make_input(task: &str, agent_name: &str) -> ToolInput {
        ToolInput {
            task_id: "test-task".to_string(),
            phase: "test".to_string(),
            agent_role: "general".to_string(),
            objective: "test sub-agent".to_string(),
            constraints: None,
            evidence: None,
            payload: json!({
                "task": task,
                "agent_name": agent_name,
            }),
            allowed_base_dir: None,
        }
    }

    #[test]
    fn spawn_agent_missing_task_returns_error() {
        let input = ToolInput {
            task_id: "test".to_string(),
            phase: "test".to_string(),
            agent_role: "general".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: json!({}),
            allowed_base_dir: None,
        };
        let tool = SpawnAgentTool;
        let result = tool.run(&input);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("missing required"),
            "expected 'missing required' error, got: {}",
            err
        );
    }

    #[test]
    fn spawn_agent_requires_registry() {
        let input = make_input("do something", "deepseek");
        let tool = SpawnAgentTool;
        let result = tool.run(&input);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not initialised") || err.contains("not found"),
            "expected 'not initialised' error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn spawn_agent_concurrency_permit_acquires_and_releases() {
        let permit = SPAWN_SEMAPHORE.acquire().await;
        assert!(permit.is_ok());
        drop(permit);

        let permit = SPAWN_SEMAPHORE.acquire().await;
        assert!(permit.is_ok());
    }

    #[test]
    fn spawn_agent_invalid_role_rejected() {
        let mut input = make_input("do something", "deepseek");
        input.payload = json!({
            "task": "do something",
            "role": "invalid_role_xyz",
        });
        let tool = SpawnAgentTool;
        let result = tool.run(&input);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("invalid sub-agent role"),
            "expected role validation error, got: {}",
            err
        );
    }

    #[test]
    fn spawn_agent_all_valid_roles_accepted_by_validator() {
        assert!(SUB_AGENT_ROLES.contains(&"general"));
        assert!(SUB_AGENT_ROLES.contains(&"explore"));
        assert!(SUB_AGENT_ROLES.contains(&"plan"));
        assert!(SUB_AGENT_ROLES.contains(&"review"));
        assert!(SUB_AGENT_ROLES.contains(&"implementer"));
        assert!(SUB_AGENT_ROLES.contains(&"verifier"));
        assert!(SUB_AGENT_ROLES.contains(&"custom"));
        assert!(!SUB_AGENT_ROLES.contains(&"bogus"));
    }
}
