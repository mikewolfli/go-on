//! Terminal chat mode — full-featured interactive AI chat in the terminal
//! (like Claude Code, Codex, OpenClaw).
//!
//! Features:
//! - Streaming agent chat with thinking/reasoning display
//! - Tool execution (file read/write, search, code execution, etc.)
//! - Skill invocation
//! - Multi-turn conversation with history persistence
//! - Ctrl+C interrupt handling
//! - Session save/resume
//! - Built-in commands
//!
//! Usage: `go-on -a` or `go-on --chat`

mod agent_turn;
mod commands;
mod display;
mod git;
mod session;
mod simple_tool;
mod tokens;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::RwLock;

use anyhow::Result;
use tokio::io::AsyncBufReadExt;
use tokio::sync::{mpsc, Notify};
use tokio::time::Duration;

use crate::agents::agent::{Agent, AgentRegistry, Message};
use crate::config::AppConfig;
use crate::i18n::runtime::{t, tf};
use crate::intelligence::capability_graph::CapabilityGraph;
use crate::orchestration::mode::{resolve_mode_runtime, GenericModeRuntime, ModeKind, ModeRuntime};
use crate::orchestration::tool::ToolRegistry;

use self::commands::{
    dispatch_builtin_command, process_user_message_and_run_agent, read_user_input,
};
use self::display::print_chat_banner;
use self::session::save_session_on_exit;
use self::tokens::TokenTracker;

/// Maximum number of characters from a tool result sent to the LLM.
const MAX_TOOL_RESULT_CHARS: usize = 100_000;

/// Maximum number of tool results included in a single follow-up message.
/// Mirrors ACP's default max_tools_per_round (8). Prevents message bloat.
const MAX_TOOLS_IN_FOLLOWUP: usize = 8;

/// Default timeout (seconds) for the follow-up agent chat call.
const DEFAULT_FOLLOWUP_TIMEOUT_SECS: u64 = 60;

/// Maximum number of concurrent tool executions.
/// Prevents resource exhaustion when the agent emits many parallel tool calls.
const MAX_CONCURRENT_TOOLS: usize = 10;
/// Session file name for conversation persistence (inside the go-on data dir).
const SESSION_FILE: &str = "chat-session.json";

/// Threshold at which we prompt the user to compact the conversation.
const COMPACT_PROMPT_THRESHOLD: usize = 30;

/// Threshold at which we automatically compact.
const AUTO_COMPACT_THRESHOLD: usize = 60;

/// How many most recent messages to keep after auto-compaction.
const AUTO_COMPACT_KEEP: usize = 40;

/// Notify mechanism for session auto-save completion — prevents concurrent disk writes.
/// Used instead of spin-wait + AtomicBool to avoid busy-waiting on exit.
/// Initialized on first access via OnceLock.
static SAVE_NOTIFY: OnceLock<Notify> = OnceLock::new();

fn save_notify() -> &'static Notify {
    SAVE_NOTIFY.get_or_init(Notify::new)
}

/// Debounce flag for session auto-save — prevents concurrent disk writes.
static SAVE_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// Cached InjectionDetector for terminal chat — created once per session.
/// Uses default detection config (threshold 0.7, contamination check enabled).
static INJECTION_DETECTOR: OnceLock<crate::security::prompt_injection::InjectionDetector> =
    OnceLock::new();

fn injection_detector() -> &'static crate::security::prompt_injection::InjectionDetector {
    INJECTION_DETECTOR.get_or_init(|| {
        crate::security::prompt_injection::InjectionDetector::new(
            crate::security::prompt_injection::DetectionConfig::default(),
        )
    })
}

/// Type alias for the safeguard approval return type to satisfy clippy::type_complexity.
type SafeguardApprovalResult<'a> = Result<(&'a [(String, String)], bool)>;

/// Canonical mode string for a [`ModeKind`] — single source used by the
/// banner, session serialization, and mode/agent switching so the displayed
/// and persisted names cannot drift (previously three copies of this match,
/// and `serialize_session` used a `Debug`-derived "fullauto" spelling).
fn mode_kind_str(kind: ModeKind) -> &'static str {
    match kind {
        ModeKind::Ask => "ask",
        ModeKind::Plan => "plan",
        ModeKind::Edit => "edit",
        ModeKind::FullAuto => "full_auto",
        ModeKind::SafeGuard => "safeguard",
    }
}

/// Returns the global ToolRegistry reference (shared process-wide singleton).
fn tool_registry() -> &'static ToolRegistry {
    static REGISTRY: OnceLock<&'static ToolRegistry> = OnceLock::new();
    let arc = crate::acp::r#impl::request::tools_pack::global_tool_registry();
    REGISTRY.get_or_init(|| Arc::as_ref(arc))
}

/// RAII guard that resets SAVE_IN_FLIGHT and notifies waiters on drop (prevents permanent lock).
struct AutoSaveGuard;
impl Drop for AutoSaveGuard {
    fn drop(&mut self) {
        SAVE_IN_FLIGHT.store(false, Ordering::Release);
        save_notify().notify_waiters();
    }
}

/// Helper to produce ANSI escape codes; expands to empty string on Windows.
macro_rules! ansi {
    ($code:expr) => {{
        #[cfg(not(target_os = "windows"))]
        {
            concat!("\u{001B}[", $code, "m")
        }
        #[cfg(target_os = "windows")]
        {
            ""
        }
    }};
}

// Re-export so sibling submodules can `use super::ansi;`.
// Note: this must appear AFTER the macro_rules! definition (macro_rules!
// macros are only in scope textually after their definition point).
pub(crate) use ansi;

/// Run an interactive terminal chat session with full agent capabilities.
pub async fn run_terminal_chat(
    config: Arc<AppConfig>,
    skill_registry: Option<Arc<RwLock<crate::orchestration::skill::SkillRegistry>>>,
    config_path: &std::path::Path,
) -> Result<()> {
    if config.agents().is_empty() {
        eprintln!("{}", tf("error.no_agents", &[]));
        return Ok(());
    }

    // ── Delegate to sub-functions for each phase ──
    let session = setup_chat_environment(config, skill_registry, config_path).await?;
    let ChatEnvironment {
        registry,
        mut current_agent,
        mut current_agent_name,
        mut current_mode,
        mut messages,
        mut token_tracker,
        mut stdin_rx,
        session_path,
    } = session;

    // ── Main chat loop ──
    loop {
        eprint!("🟢 {} > ", current_agent_name);
        std::io::Write::flush(&mut std::io::stdout()).ok();

        // Read user input
        let line = match read_user_input(&mut stdin_rx).await {
            Some(l) => l,
            None => break,
        };

        if line.is_empty() {
            continue;
        }

        // Built-in commands
        if let Some(cmd) = line.strip_prefix('/') {
            let should_exit = dispatch_builtin_command(
                cmd,
                &mut messages,
                &mut current_agent,
                &mut current_agent_name,
                &mut current_mode,
                &registry,
                &mut token_tracker,
                &session_path,
                &mut stdin_rx,
            )
            .await;
            if should_exit {
                break;
            }
            continue;
        }

        // Process user message through agent
        process_user_message_and_run_agent(
            &line,
            &mut messages,
            &current_agent,
            &current_agent_name,
            &mut token_tracker,
            &mut current_mode,
            &session_path,
            &mut stdin_rx,
        )
        .await;
    }

    // ── Save session on clean exit ──
    save_session_on_exit(&messages, &current_agent_name, &current_mode, &session_path).await;

    eprintln!("{}", t("cli.chat.goodbye"));
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Sub-functions for run_terminal_chat
// ─────────────────────────────────────────────────────────────────────────────

struct ChatEnvironment {
    registry: Arc<AgentRegistry>,
    current_agent: Arc<dyn Agent>,
    current_agent_name: String,
    current_mode: Box<dyn ModeRuntime>,
    messages: Vec<Message>,
    token_tracker: TokenTracker,
    stdin_rx: mpsc::Receiver<String>,
    session_path: std::path::PathBuf,
}

async fn setup_chat_environment(
    config: Arc<AppConfig>,
    skill_registry: Option<Arc<RwLock<crate::orchestration::skill::SkillRegistry>>>,
    config_path: &std::path::Path,
) -> Result<ChatEnvironment> {
    // ── Initialize runtime components ──
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .http1_only()
        .build()?;

    let capability_graph = Arc::new(Mutex::new(CapabilityGraph::new()));
    let registry = Arc::new(AgentRegistry::from_config(
        Arc::clone(&config),
        http_client.clone(),
        Arc::clone(&capability_graph),
    )?);

    // ── Initialize SpawnAgentTool globals for the CLI path ──
    // The server path initializes these in server_builder.rs; without the
    // equivalent here, spawn_agent would fail with "AgentRegistry not
    // initialised" / "SpawnGuard budget not initialised" in terminal chat.
    crate::orchestration::tool_extended::spawn_agent::init_spawn_agent_registry(registry.clone());
    let communication_bus = Arc::new(crate::agents::communication::bus::CommunicationBus::new());
    crate::orchestration::tool_extended::spawn_agent::init_spawn_agent_communication_bus(
        communication_bus,
    );
    crate::orchestration::tool_extended::spawn_agent::init_spawn_agent_budget();

    // ── Initialize skill registry for terminal chat ──
    // The registry comes from bootstrap (main/mod.rs perform_bootstrap), which
    // already discovered ~/.agents/skills/ and registered built-ins. Reusing it
    // avoids a second full directory scan and a second global set_skill_registry
    // (previously chat mode discovered and registered its own copy, so the two
    // registries could diverge).
    let skill_registry = match skill_registry {
        Some(registry) => registry,
        None => {
            let registry = Arc::new(RwLock::new(
                crate::orchestration::skill::SkillRegistry::default(),
            ));
            if let Ok(mut reg) = registry.write() {
                if let Err(e) = reg.discover_and_register_local_skills(None) {
                    tracing::warn!("Failed to discover local skills in terminal chat: {e}");
                }
            }
            registry
        }
    };
    crate::orchestration::tool::set_skill_registry(skill_registry);

    let mut agent_names: Vec<String> = config.agents().keys().cloned().collect();
    agent_names.sort();
    let primary = agent_names[0].clone();

    let current_agent_name = primary.clone();
    let current_agent = registry
        .get(&current_agent_name)
        .ok_or_else(|| anyhow::anyhow!("Agent '{}' not found in registry", current_agent_name))?;

    // ── Resolve current mode runtime ──
    // Default to GOON_DEFAULT_MODE env var, or load from goon-cli-mode.json,
    // or fall back to "edit".
    let default_mode = std::env::var("GOON_DEFAULT_MODE")
        .ok()
        .or_else(|| {
            let config_path = std::path::Path::new("goon-cli-mode.json");
            std::fs::read_to_string(config_path)
                .ok()
                .and_then(|content| {
                    serde_json::from_str::<serde_json::Value>(&content)
                        .ok()
                        .and_then(|v| {
                            v.get("mode")
                                .and_then(|m| m.as_str().map(|s| s.to_string()))
                        })
                })
        })
        .unwrap_or_else(|| "edit".to_string());
    let current_mode = resolve_mode_runtime(
        &default_mode,
        Some(registry.clone()),
        Some(current_agent_name.clone()),
    )
    .unwrap_or_else(|_| {
        Box::new(GenericModeRuntime::new(
            ModeKind::Edit,
            registry.clone(),
            Some(current_agent_name.clone()),
        ))
    });

    // ── Print banner ──
    print_chat_banner(&current_agent_name, current_mode.kind());

    // ── Build initial system message ──
    // The static category lines below are a curated highlight for the model;
    // the exhaustive, authoritative inventory is the dynamic "All registered
    // tools ({} total)" list that follows (and, per turn, `build_cli_principles`
    // re-states the live names). The category highlights intentionally differ
    // from `HELP_TEXT` (which is a human summary) — this text must name the
    // exact `__tool_call__` protocol tools the agent can invoke.
    let mut messages = Vec::new();
    let all_tool_names = tool_registry().all_names();
    let tool_list_str = all_tool_names.join(", ");
    let agent_list_str = agent_names.join(", ");
    messages.push(Message {
        role: "system".to_string(),
        content: format!(
            "You are go-on, an AI coding assistant running inside a terminal.\n\
             You have access to the following tool categories:\n\
             - **File tools**: read_file, write_file, read_file_lines, search_files, list_directory, find_path, create_directory, copy_path, move_path, delete_path\
             - **Patch**: apply_patch\
             - **Shell**: shell_exec (bash commands)\
             - **Git**: inspect_git_diff, git operations (status, log, commit, diff)\
             - **Build/Test**: cargo_check, cargo_test, run_tests, diagnostics\
             - **Skills**: skill_list, skill_execute, skill_create, skill_reload\
             - **Network**: http_request, dns_lookup, ping, port_scan\
             - **Archive**: archive_inspect, archive_extract\
             - **Data**: jsonl_read, jsonl_write, compress, decompress\
             - **Date/Time**: date_time, environment_info\
             - **Search**: grep, find_files, find_path, code_index_search\
             - **Diff**: diff (file comparison)\n\n\
             All registered tools ({} total): {}\n\n\
             To invoke a tool, use the __tool_call__ protocol:\n\
             `__tool_call__:tool_name:{{\"arg\": \"value\"}}`\n\n\
             Current configured agents: {}\n\
             Use /model <name> to switch agents.",
            all_tool_names.len(),
            tool_list_str,
            agent_list_str
        ),
    });

    let token_tracker = TokenTracker::default();

    // ── Async stdin reader using tokio::io::stdin().lines() ──
    // Replaces the previous spawn_blocking approach so that Ctrl+C
    // during input is handled immediately (not blocked on read_line).
    let mut stdin_lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let stdin_rx = {
        // Bounded channel (32) provides backpressure: if the user pastes
        // faster than the agent processes, the channel buffers the last 32
        // lines and the stdin reader task awaits before reading more.
        let (stdin_tx, stdin_rx) = mpsc::channel::<String>(32);
        tokio::spawn(async move {
            loop {
                match stdin_lines.next_line().await {
                    Ok(Some(line)) => {
                        let trimmed = line.trim().to_string();
                        if stdin_tx.send(trimmed).await.is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
        });
        stdin_rx
    };

    // ── Session persistence path ──
    // Resolve via the canonical goon_paths resolver so the session file lands
    // in the same data root as reinforcement/learning/metacognitive artifacts
    // (config-dir/.goon when -c points elsewhere, CWD/.goon otherwise).
    let session_path =
        crate::shared::goon_paths::resolve_goon_root(Some(config_path)).join(SESSION_FILE);
    if let Some(parent) = session_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    Ok(ChatEnvironment {
        registry,
        current_agent,
        current_agent_name,
        current_mode,
        messages,
        token_tracker,
        stdin_rx,
        session_path,
    })
}
