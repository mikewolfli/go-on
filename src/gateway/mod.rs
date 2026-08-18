//! Multi-platform gateway (M3.4, architecture-pattern validation).
//!
//! The gateway turns a platform webhook into an agent turn and delivers the
//! reply back to the platform. The pattern — learned from hermes, without the
//! 30k-line god file — is:
//!
//! - [`adapter::PlatformAdapter`]: the only platform-specific surface. A
//!   second platform is a new adapter + a `register` call, nothing else.
//! - [`registry::PlatformRegistry`]: registered adapters, with RAII
//!   unregistration via `orchestration::registration::RegistrationGuard`.
//! - [`session`]: session hash `platform:chat_id` + the per-chat
//!   [`session::TurnLease`] that serializes turns for the same chat.
//! - [`ledger::DeliveryLedger`]: SQLite delivery ledger for dedup of platform
//!   redeliveries.
//! - [`webhook`]: the concrete JSON webhook adapter + a self-contained HTTP
//!   listener (`POST /webhook/<platform>`).
//!
//! The `ledger` and `webhook` modules are gated on `backend-sqlite` (the
//! ledger's store); the pattern modules are always available.

pub mod adapter;
#[cfg(feature = "backend-sqlite")]
pub mod ledger;
pub mod registry;
pub mod session;
#[cfg(feature = "backend-sqlite")]
pub mod webhook;

// ─────────────────────────────────────────────────────────────────────────────
// One-turn agent runner (mirrors the `go-on exec` engine)
// ─────────────────────────────────────────────────────────────────────────────

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use tokio::sync::mpsc;

use crate::acp::helpers::conversation::stream_would_exceed_limits;
use crate::acp::r#impl::request::tools_pack::global_tool_registry;
use crate::agents::agent::{Agent, AgentRegistry, Message, StreamingSender};
use crate::config::AppConfig;
use crate::intelligence::capability_graph::CapabilityGraph;
use crate::orchestration::autonomy_runtime::{classify_agent_token, AgentToken};
use crate::orchestration::tool::executor::{execute_tools_concurrent, ToolExecConfig};

/// Per-chunk receive timeout for agent streaming (mirrors the ACP pipeline).
const PER_CHUNK_TIMEOUT: Duration = Duration::from_secs(120);
/// Overall cap for one agent-chat collection (mirrors the ACP pipeline).
const OVERALL_TURN_TIMEOUT: Duration = Duration::from_secs(600);
/// Maximum number of tools executed concurrently (mirrors the CLI chat path).
const MAX_CONCURRENT_TOOLS: usize = 10;

/// Run one bounded agent turn against the config's primary agent, mirroring the
/// `go-on exec` engine: build the agent registry from config, stream the chat
/// response with the same per-chunk/overall timeouts, execute intercepted tool
/// calls headlessly (approval `never`, exactly like exec's default), and return
/// the collected response text (falling back to reasoning when the agent
/// streamed none).
///
/// The agent runtime (registry + skill bootstrap) is rebuilt per call — the
/// same cost `go-on exec` pays per invocation. A gateway that needs sub-second
/// turn startup can cache the registry; that is deliberately out of scope for
/// M3.4.
pub async fn run_agent_turn(
    config: Arc<AppConfig>,
    config_path: &Path,
    prompt: &str,
) -> Result<String> {
    if config.agents().is_empty() {
        bail!("no agents configured; run `go-on --setup` first");
    }

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .http1_only()
        .build()?;
    let capability_graph = Arc::new(std::sync::Mutex::new(CapabilityGraph::new()));
    let registry = Arc::new(AgentRegistry::from_config(
        Arc::clone(&config),
        http_client.clone(),
        Arc::clone(&capability_graph),
    )?);

    // SpawnAgentTool globals — mirror cli/chat setup so spawn_agent works when
    // the agent calls it.
    crate::orchestration::tool_extended::spawn_agent::init_spawn_agent_registry(registry.clone());
    let communication_bus = Arc::new(crate::agents::communication::bus::CommunicationBus::new());
    crate::orchestration::tool_extended::spawn_agent::init_spawn_agent_communication_bus(
        communication_bus,
    );
    crate::orchestration::tool_extended::spawn_agent::init_spawn_agent_budget();

    // Skill registry via the canonical bootstrap (skills must be registered
    // before skill_execute can resolve them); degrade gracefully like exec
    // when the bootstrap fails.
    let skill_registry =
        match crate::core::bootstrap::perform_bootstrap(&crate::core::bootstrap::BootstrapConfig {
            enable_i18n: true,
            config_path: config_path.to_path_buf(),
        })
        .await
        {
            Ok(registry) => Arc::new(std::sync::RwLock::new(registry)),
            Err(e) => {
                tracing::warn!(target: "go_on::gateway", "bootstrap skipped: {e}");
                Arc::new(std::sync::RwLock::new(
                    crate::orchestration::skill::SkillRegistry::default(),
                ))
            }
        };
    crate::orchestration::tool::set_skill_registry(skill_registry);

    // Command sandbox must obey config, exactly like chat/exec.
    crate::security::sandbox::init_command_sandbox(config.security.command_sandbox.clone());

    // The primary agent is the first configured agent by name (same rule as
    // exec and the terminal chat path).
    let mut names: Vec<String> = config.agents().keys().cloned().collect();
    names.sort();
    let agent_name = names[0].clone();
    let agent = registry
        .get(&agent_name)
        .ok_or_else(|| anyhow!("agent '{agent_name}' not found in registry"))?;

    collect_turn_response(agent, &agent_name, prompt).await
}

/// Stream one chat call, collect the response, and execute intercepted tool
/// calls — the exec-style single-turn engine, without an event emitter.
async fn collect_turn_response(
    agent: Arc<dyn Agent>,
    agent_name: &str,
    prompt: &str,
) -> Result<String> {
    let tool_names = global_tool_registry().all_names().join(", ");
    let messages = vec![
        Message {
            role: "system".to_string(),
            content: format!(
                "You are go-on, an AI coding assistant answering a gateway webhook as agent '{}'.\n\
                 You have access to the following tools via the __tool_call__:tool_name:{{\"arg\": \"value\"}} protocol:\n{tool_names}",
                agent_name
            ),
        },
        Message {
            role: "user".to_string(),
            content: prompt.to_string(),
        },
    ];
    let principles = vec![
        "You are a helpful AI coding assistant with access to tools.".to_string(),
        "Use the __tool_call__:tool_name:json_args protocol to invoke tools.".to_string(),
        format!("Built-in tools: {tool_names}"),
    ];

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let sender = StreamingSender::from(tx);
    let task =
        tokio::spawn(async move { agent.chat(messages, Some(principles), None, sender).await });
    let abort = task.abort_handle();

    let started = Instant::now();
    let mut response = String::new();
    let mut reasoning = String::new();
    let mut tool_calls: Vec<(String, String)> = Vec::new();
    let mut chunks = 0usize;
    let mut total_chars = 0usize;
    let mut interrupted = false; // timeout or stream-cap truncation

    loop {
        let elapsed = started.elapsed();
        if elapsed >= OVERALL_TURN_TIMEOUT {
            tracing::warn!(
                target: "go_on::gateway",
                "agent streaming overall timeout after {}s — aborting",
                OVERALL_TURN_TIMEOUT.as_secs()
            );
            interrupted = true;
            break;
        }
        let remaining = OVERALL_TURN_TIMEOUT - elapsed;
        let token = match tokio::time::timeout(PER_CHUNK_TIMEOUT.min(remaining), rx.recv()).await {
            Ok(Some(token)) => token,
            // Receiver closed — the agent finished streaming.
            Ok(None) => break,
            Err(_) => {
                tracing::warn!(
                    target: "go_on::gateway",
                    "agent streaming recv() timed out after {}s — aborting",
                    PER_CHUNK_TIMEOUT.as_secs()
                );
                interrupted = true;
                break;
            }
        };

        match classify_agent_token(&token) {
            AgentToken::ModelUsed(_) => {}
            AgentToken::ToolCall(name, args) => tool_calls.push((name, args)),
            AgentToken::ReasoningMarker | AgentToken::Telemetry => {}
            AgentToken::Reasoning(text) => {
                let next_chars = text.chars().count();
                if stream_would_exceed_limits(chunks, total_chars, next_chars) {
                    tracing::warn!(
                        target: "go_on::gateway",
                        "output truncated at {total_chars} chars (chunks {chunks})"
                    );
                    interrupted = true;
                    break;
                }
                reasoning.push_str(&text);
                chunks += 1;
                total_chars += next_chars;
            }
            AgentToken::Content(text) => {
                let next_chars = text.chars().count();
                if stream_would_exceed_limits(chunks, total_chars, next_chars) {
                    tracing::warn!(
                        target: "go_on::gateway",
                        "output truncated at {total_chars} chars (chunks {chunks})"
                    );
                    interrupted = true;
                    break;
                }
                response.push_str(&text);
                chunks += 1;
                total_chars += next_chars;
            }
        }
    }

    // If we bailed out early the agent task may still be streaming — stop it.
    if interrupted {
        abort.abort();
    }

    let chat_failed = match tokio::time::timeout(PER_CHUNK_TIMEOUT, task).await {
        Ok(Ok(Ok(()))) => false,
        Ok(Ok(Err(e))) => {
            tracing::warn!(target: "go_on::gateway", "agent chat failed: {e}");
            true
        }
        Ok(Err(e)) => {
            tracing::warn!(target: "go_on::gateway", "agent chat task panicked: {e}");
            true
        }
        Err(_) => {
            tracing::warn!(
                target: "go_on::gateway",
                "agent chat join timed out — aborting"
            );
            abort.abort();
            true
        }
    };

    if chat_failed || interrupted {
        bail!("agent turn failed");
    }

    // Execute intercepted tool calls headlessly — same policy as
    // `go-on exec --approval-mode never` (no prompts, no governance gate).
    if !tool_calls.is_empty() {
        let exec_result = execute_tools_concurrent(
            &tool_calls,
            global_tool_registry(),
            &ToolExecConfig {
                max_concurrency: MAX_CONCURRENT_TOOLS,
                circuit_breaker_limit: 0,
                operation_mode: "ask".to_string(),
                governance_required: false,
                is_safeguard: false,
                acp_session_id: None,
            },
            None,
            "go-on gateway",
            0,
        )
        .await;
        tracing::debug!(
            target: "go_on::gateway",
            failures = exec_result.failure_count,
            "intercepted tool calls executed"
        );
    }

    // When the agent produced only reasoning, use it as the response (mirrors
    // the ACP collection core).
    if response.trim().is_empty() && !reasoning.trim().is_empty() {
        response = reasoning;
    }

    Ok(response.trim().to_string())
}
