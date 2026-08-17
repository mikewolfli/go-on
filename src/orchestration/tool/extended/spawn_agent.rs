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
//! - Global SpawnGuard budget prevents unbounded sub-agent spawning (RAII).
//!
//! # Sub-agent lifecycle
//! - role classification (7 types, CodeWhale-compatible)
//! - token_budget tracking
//! - structured output (SUMMARY/CHANGES/EVIDENCE/RISKS/BLOCKERS)
//! - timeout guard with heartbeat-style cancellation
//! - transient-failure retry with backoff (up to 2 retries)
//! - global concurrency cap (SpawnGuard: max 128 concurrent)
//!
//! # BLUE70 CommunicationBus integration
//! - Each spawn is registered in the CommunicationBus AgentTree for observability.
//! - Delegate/Result messages are sent via the CommunicationBus Messenger.
//! - The global CommunicationBus is initialised at server startup via
//!   `init_spawn_agent_communication_bus()`.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::agents::communication::agent_thread::SpawnGuard;

use crate::agent::{AgentRegistry, Message, StreamingSender};
use crate::agents::communication::bus::CommunicationBus;
use crate::agents::communication::message::{AgentMessage, AgentTarget};
use crate::agents::communication::path::AgentPath;
use crate::agents::communication::tree::AgentNodeMetadata;
use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};

// ---------------------------------------------------------------------------
// Global registry + CommunicationBus + SpawnGuard budget — initialised at server startup
// ---------------------------------------------------------------------------

static SPAWN_AGENT_REGISTRY: OnceLock<Arc<AgentRegistry>> = OnceLock::new();

/// Global CommunicationBus for tree-based agent communication (BLUE70).
static SPAWN_COMMUNICATION_BUS: OnceLock<Arc<CommunicationBus>> = OnceLock::new();

/// Monotonically increasing sequence counter for fork IDs.
static SPAWN_FORK_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Global concurrency budget counter — tracks current in-flight spawns (0 = none).
/// Paired with SPAWN_MAX_CONCURRENCY for the hard limit.
static SPAWN_BUDGET: OnceLock<Arc<AtomicU64>> = OnceLock::new();

/// Maximum concurrent sub-agent spawns. Default 128.
const DEFAULT_MAX_CONCURRENCY: u64 = 128;

/// Initialise the global `AgentRegistry` reference used by `SpawnAgentTool`.
pub fn init_spawn_agent_registry(registry: Arc<AgentRegistry>) {
    SPAWN_AGENT_REGISTRY.set(registry).ok();
}

/// Initialise the global `CommunicationBus` for agent tree-based routing (BLUE70).
///
/// Must be called once at server startup, after the `CommunicationBus` has been
/// built but before any tool invocations.
pub fn init_spawn_agent_communication_bus(bus: Arc<CommunicationBus>) {
    SPAWN_COMMUNICATION_BUS.set(bus).ok();
}

/// Initialise the global concurrency budget for SpawnGuard (BLUE71 §5).
/// Budget starts at 0; each SpawnGuard::try_reserve increments atomically.
pub fn init_spawn_agent_budget() {
    SPAWN_BUDGET.set(Arc::new(AtomicU64::new(0))).ok();
}

/// Get the global SpawnGuard budget, if initialised.
fn spawn_budget() -> Option<&'static Arc<AtomicU64>> {
    SPAWN_BUDGET.get()
}

fn agent_registry() -> Result<&'static Arc<AgentRegistry>> {
    SPAWN_AGENT_REGISTRY
        .get()
        .ok_or_else(|| anyhow::anyhow!("SpawnAgentTool: AgentRegistry not initialised"))
}

/// Access the process-wide CommunicationBus (set by `new_acp_server`).
/// Exposed for governance.status agent-tree observability (BLUE70).
pub(crate) fn communication_bus() -> Option<&'static Arc<CommunicationBus>> {
    SPAWN_COMMUNICATION_BUS.get()
}

// ---------------------------------------------------------------------------
// Tool definition
// ---------------------------------------------------------------------------

/// Parsed and validated SpawnAgentTool parameters.
struct SpawnParams {
    task: String,
    agent_name: String,
    model_override: Option<String>,
    timeout_secs: u64,
    role: Option<String>,
    token_budget: Option<u64>,
}

/// Parse and validate the tool parameters from `input`.
///
/// Shared by `run()` and `run_async()` so the two entry points can never
/// drift apart. Validation happens before any global-registry access, so
/// bad-input errors are deterministic in both paths.
fn parse_spawn_params(input: &ToolInput) -> Result<SpawnParams> {
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
    Ok(SpawnParams {
        task,
        agent_name,
        model_override,
        timeout_secs,
        role,
        token_budget,
    })
}

/// Spawn a sub-agent with a specific task and collect its response.
pub struct SpawnAgentTool;

impl Tool for SpawnAgentTool {
    fn name(&self) -> &'static str {
        "spawn_agent"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        // This tool is inherently async (agent chat is async). The sync `run()`
        // always executes on the shared dedicated blocking runtime (see
        // `exec_common::blocking_runtime()`), mirroring `web_search.rs`/`lsp.rs`.
        // It must NOT use `Handle::try_current() → handle.block_on`: that would
        // block a tokio worker thread when `run()` is called from inside an
        // existing runtime. Async callers should always use run_async.
        // Validate parameters FIRST so bad-input tests get a proper error.
        let params = parse_spawn_params(input)?;
        let registry = agent_registry()?.clone();

        // Always use the dedicated blocking runtime. Never `Handle::try_current()`
        // + `handle.block_on()` here — when `run()` is invoked from within an
        // existing tokio runtime that would block a worker thread. The guard
        // serializes concurrent sync `run()` calls on the shared
        // current-thread runtime.
        crate::orchestration::tool::exec_common::with_blocking_runtime(|rt| {
            rt.block_on(execute_spawn(
                registry,
                params.task,
                params.agent_name,
                params.model_override,
                params.timeout_secs,
                params.role,
                params.token_budget,
            ))
        })
    }

    fn run_async(
        self: Arc<Self>,
        input: ToolInput,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send>> {
        Box::pin(async move {
            let params = parse_spawn_params(&input)?;
            let registry = agent_registry()
                .map_err(|e| anyhow::anyhow!("SpawnAgentTool: {}", e))?
                .clone();
            execute_spawn(
                registry,
                params.task,
                params.agent_name,
                params.model_override,
                params.timeout_secs,
                params.role,
                params.token_budget,
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

    // ── Concurrency limit (BLUE71 §5) ───────────────────────────────
    // Acquire RAII concurrency slot via SpawnGuard. Auto-releases on drop
    // (even during panic). Uses global budget counter. (The former
    // ExecutionGovernor check_limits block was removed: it ran against a
    // freshly constructed budget — depth 2 < 10, active_children 0 < 128,
    // no token ceiling — so it could never deny, and the "spawn" tree
    // registration it relied on was never consumed elsewhere.)
    let budget_arc = spawn_budget()
        .ok_or_else(|| anyhow::anyhow!("SpawnGuard budget not initialised"))?
        .clone();
    let _guard = SpawnGuard::try_reserve(budget_arc, DEFAULT_MAX_CONCURRENCY)
        .map_err(|e| anyhow::anyhow!("sub-agent concurrency limit: {}", e))?;

    // 1. Resolve agent from registry.
    let agent = registry
        .get(&agent_name)
        .ok_or_else(|| anyhow::anyhow!("agent '{}' not found in registry", agent_name))?;

    // ── BLUE70: Register in CommunicationBus AgentTree ─────────────────
    // Register the spawn in the agent tree for observability and future
    // tree-based routing. Uses fork_id as the agent path segment.
    let agent_path_str = format!("spawn/{}", fork_id);
    let child_path = AgentPath::parse(&agent_path_str).ok();
    if let Some(ref cp) = child_path {
        if let Some(bus) = communication_bus() {
            let metadata = AgentNodeMetadata::new()
                .with_role(role.as_deref().unwrap_or("general"))
                .with_model(model_override.as_deref().unwrap_or(&agent_name));
            let _ = bus.register_agent(cp, &agent_name, metadata).await;

            // Send a Delegate message for observability tracing
            let delegate_msg = AgentMessage::delegate(
                AgentPath::parse("root").unwrap_or_else(|_| cp.clone()),
                AgentTarget::Direct(cp.clone()),
                task.clone(),
                role.clone(),
                token_budget,
                timeout_secs,
            );
            let _ = bus.send_message(delegate_msg).await;
            bus.record_fork();
        }
    }
    // ────────────────────────────────────────────────────────────────────

    // ── BLUE70: Use ContextForker for context inheritance ───────────
    // Use the CommunicationBus ContextForker to create the ForkContext,
    // which provides proper parent-to-child context inheritance with
    // KV cache fingerprint support.
    let fork_context = child_path.as_ref().and_then(|cp| {
        let bus = communication_bus()?;
        let parent_path = AgentPath::parse("root").ok()?;
        Some(bus.forker().fork(
            &parent_path,
            cp,
            |_| crate::agents::communication::forker::ParentContext {
                conversation_summary: None,
                principles: vec![
                    "Follow the principles defined in docs/blueprints/principle.md".to_string(),
                    "Complete the assigned task thoroughly with structured output".to_string(),
                ],
                allowed_base_dir: None,
                memories: Vec::new(),
            },
            None,
        ))
    });
    // ────────────────────────────────────────────────────────────────────

    // 2. Build messages with role-specific system prompt.
    //    Include ForkContext summary/principles when available.
    let system_prompt = {
        let base = build_role_prompt(role.as_deref());
        if let Some(ref ctx) = fork_context {
            let mut extra = String::new();
            if !ctx.principles.is_empty() {
                extra.push_str("\n\nInherited principles:");
                for p in &ctx.principles {
                    extra.push_str("\n- ");
                    extra.push_str(p);
                }
            }
            if let Some(ref summary) = ctx.conversation_summary {
                extra.push_str("\n\nParent conversation summary:\n");
                extra.push_str(summary);
            }
            if extra.is_empty() {
                base
            } else {
                format!("{}{}", base, extra)
            }
        } else {
            base
        }
    };
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

    // ── BLUE71 §7: lifecycle transition to Active ─────────────────────
    if let Some(cp) = child_path.as_ref() {
        if let Some(bus) = communication_bus() {
            bus.set_lifecycle(
                cp,
                crate::agents::communication::lifecycle::AgentLifecycle::Active {
                    phase: crate::agents::communication::lifecycle::AgentPhase::Executing,
                    started_at_ms: crate::shared::timestamps::now_ts_ms() as u64,
                    tokens_used: 0,
                },
            )
            .await;
        }
    }

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
                // 7. Collect all streamed tokens, bounded by the shared
                // stream caps (the response is surfaced to the caller).
                let response =
                    crate::acp::helpers::conversation::drain_channel_capped(&mut rx).await;

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

                // Estimate actual token usage via the shared CJK-aware
                // estimator so budget enforcement agrees with the rest of the
                // binary (instead of a naive 4-chars/token heuristic).
                let actual_tokens =
                    crate::shared::token_estimator::estimate_tokens(&response).max(1) as u64;
                let budget_exceeded = token_budget.is_some_and(|b| actual_tokens > b);

                // ── BLUE71 §7: lifecycle Completed ──
                if let Some(cp) = child_path.as_ref() {
                    if let Some(bus) = communication_bus() {
                        bus.set_lifecycle(
                            cp,
                            crate::agents::communication::lifecycle::AgentLifecycle::Completed {
                                result: summary.clone().unwrap_or_default(),
                                tokens_used: actual_tokens,
                                wall_time_ms: 0,
                                completed_at_ms: crate::shared::timestamps::now_ts_ms() as u64,
                            },
                        )
                        .await;
                    }
                }

                unregister_spawned_agent(child_path.as_ref()).await;

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
                // 5xx status codes (500/502/503/504) and their usual wording
                // indicate a transient upstream failure; a bare `contains("50")`
                // previously matched any error string that happened to include
                // "50" (line numbers, error codes), triggering spurious retries.
                let is_transient = err_str.contains("timeout")
                    || err_str.contains("rate_limit")
                    || err_str.contains("429")
                    || err_str.contains("500")
                    || err_str.contains("502")
                    || err_str.contains("503")
                    || err_str.contains("504")
                    || err_str.contains("connection")
                    || err_str.contains("reset");
                if !is_transient || attempt == MAX_RETRIES {
                    // Drain any remaining tokens to avoid sender/receiver deadlock.
                    while rx.try_recv().is_ok() {}

                    mark_spawned_agent_errored(child_path.as_ref(), &err_str).await;
                    unregister_spawned_agent(child_path.as_ref()).await;

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
                    mark_spawned_agent_errored(
                        child_path.as_ref(),
                        &format!("timed out after {} seconds", timeout_secs),
                    )
                    .await;
                    unregister_spawned_agent(child_path.as_ref()).await;

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
    mark_spawned_agent_errored(
        child_path.as_ref(),
        &last_error
            .clone()
            .unwrap_or_else(|| "retries exhausted".to_string()),
    )
    .await;
    unregister_spawned_agent(child_path.as_ref()).await;

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

/// Remove a finished spawned agent from the CommunicationBus AgentTree and
/// messenger inbox so neither accumulates entries per spawn (previously nodes
/// were registered and never removed — a long-running leak in
/// `governance.status` counts, and inboxes never had a cleanup path).
/// The `spawn` namespace node itself is kept; only the per-fork child is
/// removed.
async fn unregister_spawned_agent(child_path: Option<&AgentPath>) {
    if let Some(cp) = child_path {
        if let Some(bus) = communication_bus() {
            bus.cleanup_agent(cp).await;
        }
    }
}

/// Transition the spawned agent's lifecycle node to Errored.
async fn mark_spawned_agent_errored(child_path: Option<&AgentPath>, error: &str) {
    if let Some(cp) = child_path {
        if let Some(bus) = communication_bus() {
            bus.set_lifecycle(
                cp,
                crate::agents::communication::lifecycle::AgentLifecycle::Errored {
                    error: error.to_string(),
                    tokens_used: 0,
                    wall_time_ms: 0,
                },
            )
            .await;
        }
    }
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
    use serde_json::json;

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
    async fn spawn_agent_guard_reserves_and_releases() {
        let budget = Arc::new(AtomicU64::new(0));

        let guard = SpawnGuard::try_reserve(budget.clone(), 128).unwrap();
        // Drop guard releases the slot automatically
        drop(guard);

        let guard = SpawnGuard::try_reserve(budget.clone(), 128).unwrap();
        drop(guard);
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
