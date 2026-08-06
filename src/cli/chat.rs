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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::AsyncBufReadExt;
use tokio::signal;
use tokio::sync::{mpsc, Notify};
use tokio::time::{timeout, Duration};
use tracing::{debug, warn};

use crate::acp::helpers::autonomy::terminal_chat_contract_snapshot;
use crate::acp::helpers::context::run_with_optional_timeout;
use crate::acp::r#impl::chat::agent_runtime::{collect_agent_responses, CollectedResponse};
use crate::agents::agent::{Agent, AgentRegistry, Message, StreamingSender};
use crate::cli::markdown_renderer::StreamMarkdownRenderer;
use crate::config::AppConfig;
use crate::i18n::runtime::{t, tf};
use crate::orchestration::mode::{resolve_mode_runtime, GenericModeRuntime, ModeKind, ModeRuntime};

use crate::governance::status::quick_check_tool as governance_gate;
use crate::intelligence::capability_graph::CapabilityGraph;
use crate::orchestration::autonomy_runtime::{
    build_tool_execution_followup_message, build_tool_result_block, parse_tool_call_token,
    REASONING_END, REASONING_START, TOKEN_FINISH_REASON_PREFIX, TOKEN_THINKING_PREFIX,
    TOKEN_USAGE_PREFIX,
};
use crate::orchestration::tool::executor::{execute_tools_concurrent, ToolExecConfig};
use crate::orchestration::tool::{ToolInput, ToolOutput, ToolRegistry};

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
/// Session file name for conversation persistence (inside .goon/ directory).
const SESSION_FILE: &str = ".goon/chat-session.json";

/// Threshold at which we prompt the user to compact the conversation.
const COMPACT_PROMPT_THRESHOLD: usize = 30;

/// Threshold at which we automatically compact.
const AUTO_COMPACT_THRESHOLD: usize = 60;

/// How many most recent messages to keep after auto-compaction.
const AUTO_COMPACT_KEEP: usize = 40;

/// Default pricing fallback: GPT-4o input cost per token ($0.15 per 1M tokens).
/// Used when provider cost info is unavailable.
const GPT4O_INPUT_COST_PER_TOKEN: f64 = 0.15 / 1_000_000.0;

/// Default pricing fallback: GPT-4o output cost per token ($0.60 per 1M tokens).
/// Used when provider cost info is unavailable.
const GPT4O_OUTPUT_COST_PER_TOKEN: f64 = 0.60 / 1_000_000.0;

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
type SafeguardApprovalResult<'a> = Result<(&'a [(String, String)], Option<usize>)>;

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

/// Max lines to display for help text.
const HELP_TEXT: &str = "\
Commands:
  /quit        Exit chat
  /clear       Clear conversation history
  /save        Save session to file
  /load        Load session from file
  /help        Show this help
  /agents      List configured agents
  /model       Switch active agent model
  /tools       List available tools
  /skills      List available skills
  /stats       Show conversation stats
  /context     Show context window usage (estimated tokens/characters)
  /compact     Summarize & compact conversation history
  /cost        Show token usage and estimated cost
  /diff        Show git diff (optional path filter)
  /commit      AI-powered git commit (generates message, confirms before committing)
  /review      AI-powered code review of current git diff
  /plan        AI-generated structured execution plan from conversation context
  /find_path   Search for files by name glob
  /models      List available models for current agent
  /retry       Re-send the last user message

The AI agent has access to tools:
  - Read/write files (read_file, write_file, read_file_lines)
  - Search files and directories (search_files, grep, find_path, list_directory)
  - Apply patches (apply_patch)
  - Execute shell commands (shell_exec)
  - Git operations (diff, status, log, commit, review)
  - Cargo commands (cargo_check, cargo_test)
  - Diagnostics (diagnostics)
  - Network tools (http_request, dns_lookup, ping, port_scan)
  - Archive tools (archive_inspect, archive_extract)
  - Compression (compress, decompress)
  - Data tools (jsonl_read, jsonl_write)
  - Environment info (date_time, environment_info)
  - Code search (code_index_search)
  - File comparison (diff)
  - Skills (skill_list, skill_execute, skill_create, skill_reload)
  - Multi-turn conversation with context
  - File operations (copy_path, move_path, delete_path, create_directory)
";

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

/// Session data for persistence.
#[derive(Serialize, Deserialize)]
struct ChatSession {
    messages: Vec<Message>,
    agent_name: String,
    #[serde(default)]
    mode: String,
}

/// Estimate token count from text using the canonical CJK/ASCII-weighted
/// estimator (see [`crate::shared::token_estimator::estimate_tokens`]).
pub fn estimate_tokens(text: &str) -> usize {
    crate::shared::token_estimator::estimate_tokens(text)
}

/// Track cumulative token usage and cost across the session.
#[derive(Default, Clone, Serialize, Deserialize)]
struct TokenTracker {
    total_prompt_tokens: usize,
    total_completion_tokens: usize,
    total_cost_usd: f64,
}

impl TokenTracker {
    fn record_usage(&mut self, prompt_tokens: usize, completion_tokens: usize) {
        self.total_prompt_tokens += prompt_tokens;
        self.total_completion_tokens += completion_tokens;
        // Use default pricing fallback when provider cost info is unavailable.
        self.total_cost_usd += (prompt_tokens as f64 * GPT4O_INPUT_COST_PER_TOKEN)
            + (completion_tokens as f64 * GPT4O_OUTPUT_COST_PER_TOKEN);
    }

    fn display(&self) -> String {
        format!(
            "{}Tokens:{}{} prompt + {} completion = {} total  |  Cost: ${:.6}\n",
            ansi!("1"),
            ansi!("0"),
            self.total_prompt_tokens,
            self.total_completion_tokens,
            self.total_prompt_tokens + self.total_completion_tokens,
            self.total_cost_usd,
        )
    }
}

/// Run an interactive terminal chat session with full agent capabilities.
pub async fn run_terminal_chat(config: Arc<AppConfig>) -> Result<()> {
    if config.agents().is_empty() {
        eprintln!("{}", tf("error.no_agents", &[]));
        return Ok(());
    }

    // ── Delegate to sub-functions for each phase ──
    let session = setup_chat_environment(config).await?;
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

async fn setup_chat_environment(config: Arc<AppConfig>) -> Result<ChatEnvironment> {
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

    // ── Initialize skill registry for terminal chat ──
    {
        let skill_registry = Arc::new(RwLock::new(
            crate::orchestration::skill::SkillRegistry::default(),
        ));
        if let Ok(mut reg) = skill_registry.write() {
            if let Err(e) = reg.discover_and_register_local_skills(None) {
                tracing::warn!("Failed to discover local skills in terminal chat: {e}");
            }
        }
        crate::orchestration::tool::set_skill_registry(skill_registry);
    }

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
    print_chat_banner(&current_agent_name);

    // ── Build initial system message ──
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
    let session_path = std::path::PathBuf::from(SESSION_FILE);
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

/// Print the startup banner for the terminal chat session.
fn print_chat_banner(current_agent_name: &str) {
    let version = env!("CARGO_PKG_VERSION");
    eprintln!("╔════════════════════════════════════════════════════════════════╗");
    eprintln!("║            go-on terminal chat v{:<46} ║", version);
    eprintln!("╠════════════════════════════════════════════════════════════════╣");
    eprintln!("║  Agent: {:<60} ║", current_agent_name);
    eprintln!("║  Mode:  {:<60} ║", "edit");
    eprintln!("║  Commands: /help /quit /clear /save /load /cost /compact    ║");
    eprintln!("║   /diff /commit /plan /model /models /retry /context /tools /skills   ║");
    eprintln!("║   /mode /stats /find_path  ║");
    eprintln!("╚════════════════════════════════════════════════════════════════╝");
    eprintln!();
}

/// Read a line from stdin with Ctrl+C handling. Returns None on EOF.
async fn read_user_input(stdin_rx: &mut mpsc::Receiver<String>) -> Option<String> {
    let mut buffer = String::new();
    loop {
        tokio::select! {
            line = stdin_rx.recv() => {
                let line = match line {
                    Some(l) => l,
                    None => return if buffer.is_empty() { None } else { Some(buffer) },
                };
                // If we already have buffered content, this is a continuation line.
                if !buffer.is_empty() {
                    buffer.push('\n');
                    buffer.push_str(line.trim_end());
                } else {
                    // Check for backslash continuation: line ends with \
                    if let Some(trimmed) = line.strip_suffix('\\') {
                        buffer.push_str(trimmed.trim_end());
                        // Continue reading
                        continue;
                    }
                    // Check for whitespace continuation: line starts with whitespace
                    if line.starts_with(' ') || line.starts_with('\t') {
                        buffer.push_str(line.trim_end());
                        continue;
                    }
                    buffer = line.trim_end().to_string();
                }

                // Check for unbalanced braces (multi-line payloads like JSON).
                let open_count = buffer.chars().filter(|c| *c == '{').count();
                let close_count = buffer.chars().filter(|c| *c == '}').count();
                if open_count > close_count {
                    // Braces are unbalanced — continue reading.
                    continue;
                }

                return Some(buffer);
            }
            _ = signal::ctrl_c() => {
                if buffer.is_empty() {
                    eprintln!("\n{}{}{}", ansi!("33"), t("cli.chat.interrupted"), ansi!("0"));
                    return Some(String::new());
                } else {
                    // Return whatever we've buffered so far.
                    return Some(buffer);
                }
            }
        }
    }
}

/// Dispatch a built-in command. Returns `true` if the caller should exit the main loop.
#[allow(clippy::too_many_arguments)]
async fn dispatch_builtin_command(
    cmd: &str,
    messages: &mut Vec<Message>,
    current_agent: &mut Arc<dyn Agent>,
    current_agent_name: &mut String,
    current_mode: &mut Box<dyn ModeRuntime>,
    registry: &Arc<AgentRegistry>,
    token_tracker: &mut TokenTracker,
    session_path: &std::path::Path,
    stdin_rx: &mut mpsc::Receiver<String>,
) -> bool {
    match cmd {
        // ── Session commands ──
        "quit" | "exit" | "q" => return true,
        "help" | "h" => {
            eprint!("{}", HELP_TEXT);
        }
        "clear" => {
            messages.clear();
            eprintln!(
                "{}{}{}",
                ansi!("32"),
                t("cli.chat.conversation_cleared"),
                ansi!("0")
            );
        }
        "save" => {
            handle_save_command(messages, current_agent_name, current_mode, session_path).await;
        }
        "load" => {
            handle_load_command(
                session_path,
                messages,
                current_agent,
                current_agent_name,
                current_mode,
                registry,
            )
            .await;
        }
        // ── Agent / tool info commands ──
        "agents" => {
            let names = registry.names();
            for name in &names {
                eprintln!("{}", tf("cli.chat.agents_list", &[("name", name)]));
            }
            eprintln!(
                "{}",
                tf(
                    "cli.chat.switch_agent_hint",
                    &[("name", current_agent_name)]
                )
            );
        }
        "tools" => {
            let reg = tool_registry();
            let names = reg.all_names();
            eprintln!(
                "{}",
                tf(
                    "cli.chat.tools_count",
                    &[("count", &names.len().to_string())]
                )
            );
            for name in names {
                if let Some(profile) = reg.profile(name) {
                    eprintln!(
                        "{}",
                        tf(
                            "cli.chat.tools_list_entry",
                            &[
                                ("name", name),
                                ("capability", &profile.capability.to_string())
                            ]
                        )
                    );
                } else {
                    eprintln!("  {name}");
                }
            }
        }
        "skills" => {
            display_skills();
        }
        // ── Information commands ──
        "stats" => {
            display_stats(messages, token_tracker);
        }
        "cost" => {
            eprint!("{}", token_tracker.display());
        }
        "context" => {
            display_context(messages);
        }
        // ── Compact ──
        "compact" => {
            execute_compact_command(messages, current_agent).await;
        }
        // ── Git commands ──
        cmd if cmd == "diff" || cmd.starts_with("diff ") => {
            execute_diff_command(cmd).await;
        }
        "commit" => {
            execute_commit_command(messages, current_agent, stdin_rx).await;
        }
        "review" => {
            execute_review_command(current_agent).await;
        }
        // ── Plan ──
        "plan" => {
            execute_plan_command(
                messages,
                current_agent,
                registry,
                current_agent_name,
                current_mode,
            )
            .await;
        }
        // ── Find path ──
        find_cmd if find_cmd.starts_with("find_path") || find_cmd.starts_with("find ") => {
            execute_find_path_command(find_cmd).await;
        }
        // ── Mode ──
        mode_cmd if mode_cmd.starts_with("mode") => {
            execute_mode_command(mode_cmd, current_mode, registry, current_agent_name).await;
        }
        // ── Models ──
        "models" => {
            display_models(current_agent, current_agent_name);
        }
        // ── Retry ──
        "retry" => {
            execute_retry_command(messages, current_agent, current_mode, token_tracker).await;
        }
        // ── Model (switch agent) ──
        model_cmd if model_cmd.starts_with("model") => {
            execute_switch_agent(
                model_cmd,
                current_agent,
                current_agent_name,
                current_mode,
                registry,
            )
            .await;
        }
        _ => {
            eprintln!("{}", tf("cli.chat.unknown_command", &[("cmd", cmd)]));
        }
    }
    false
}

/// Process a user message through injection detection, run the agent, and auto-save.
async fn process_user_message_and_run_agent(
    line: &str,
    messages: &mut Vec<Message>,
    current_agent: &Arc<dyn Agent>,
    current_agent_name: &str,
    token_tracker: &mut TokenTracker,
    current_mode: &mut Box<dyn ModeRuntime>,
    session_path: &std::path::Path,
) {
    // ── Prompt injection detection ──
    {
        use crate::security::severity::DetectionSeverity as InjectionSeverity;
        let detector = injection_detector();
        let (sanitized, result) = detector.detect_and_sanitize(line);

        if result.detected {
            for v in &result.violations {
                warn!(
                    target: "cli_injection",
                    category = ?v.category,
                    severity = ?v.base.severity,
                    pattern_id = ?v.pattern_id,
                    description = %v.base.description,
                    "prompt injection detected in user input"
                );
            }

            if detector.should_block(&result, InjectionSeverity::High) {
                let critical: Vec<String> = result
                    .violations
                    .iter()
                    .filter(|v| v.base.severity >= InjectionSeverity::High)
                    .map(|v| format!("{:?}: {}", v.category, v.base.description))
                    .collect();
                eprintln!(
                    "{}{}{}",
                    ansi!("31"),
                    tf(
                        "cli.chat.injection_blocked",
                        &[("violations", &critical.join("; "))]
                    ),
                    ansi!("0")
                );
                return;
            }

            eprintln!(
                "{}{}{}",
                ansi!("33"),
                tf(
                    "cli.chat.injection_warning",
                    &[("score", &format!("{:.2}", result.contamination_score))]
                ),
                ansi!("0")
            );
            messages.push(Message {
                role: "user".to_string(),
                content: sanitized,
            });
        } else {
            messages.push(Message {
                role: "user".to_string(),
                content: line.to_string(),
            });
        }
    }

    eprint!("{}🤖 {}", ansi!("1"), ansi!("0"));
    std::io::Write::flush(&mut std::io::stdout()).ok();

    let principles = build_cli_principles();

    match run_agent_with_tools(
        current_agent,
        messages,
        principles,
        Some(current_mode.as_ref()),
    )
    .await
    {
        Ok((resp, prompt_tokens, completion_tokens)) => {
            token_tracker.record_usage(prompt_tokens, completion_tokens);
            if !resp.trim().is_empty() {
                eprintln!(
                    "{}{}{}",
                    ansi!("90"),
                    tf(
                        "cli.chat.turn_complete",
                        &[("tokens", &(prompt_tokens + completion_tokens).to_string())]
                    ),
                    ansi!("0")
                );
            }
        }
        Err(e) => {
            let err_msg = tf("error.generation_failed", &[("reason", &e.to_string())]);
            eprintln!("\n{}⚠️  {} {}", ansi!("31"), err_msg, ansi!("0"));
            // Clean up the failed assistant message to avoid token waste on retry
            if messages.last().map(|m| m.role.as_str()) == Some("assistant") {
                let last_empty = messages
                    .last()
                    .map(|m| m.content.is_empty())
                    .unwrap_or(false);
                if last_empty {
                    messages.pop();
                }
            }
        }
    }

    // ── Auto-save session every turn ──
    auto_save_turn(messages, current_agent_name, current_mode, session_path);

    // ── Compact prompt threshold check ──
    check_compact_threshold(messages);
}

// ─────────────────────────────────────────────────────────────────────────────
// Command handler helper functions
// ─────────────────────────────────────────────────────────────────────────────

#[allow(clippy::borrowed_box)]
async fn handle_save_command(
    messages: &[Message],
    current_agent_name: &str,
    current_mode: &Box<dyn ModeRuntime>,
    session_path: &std::path::Path,
) {
    let session = ChatSession {
        messages: messages.to_vec(),
        agent_name: current_agent_name.to_string(),
        mode: format!("{:?}", current_mode.kind()).to_lowercase(),
    };
    match serde_json::to_string_pretty(&session) {
        Ok(json) => match tokio::fs::write(session_path, &json).await {
            Ok(()) => eprintln!(
                "{}{}{}",
                ansi!("32"),
                tf(
                    "cli.chat.session_saved",
                    &[("path", &session_path.display().to_string())]
                ),
                ansi!("0")
            ),
            Err(e) => eprintln!(
                "{}{}{}",
                ansi!("31"),
                tf("cli.chat.session_save_failed", &[("error", &e.to_string())]),
                ansi!("0")
            ),
        },
        Err(e) => eprintln!(
            "{}{}{}",
            ansi!("31"),
            tf(
                "cli.chat.session_serialize_failed",
                &[("error", &e.to_string())]
            ),
            ansi!("0")
        ),
    }
}

async fn handle_load_command(
    session_path: &std::path::Path,
    messages: &mut Vec<Message>,
    current_agent: &mut Arc<dyn Agent>,
    current_agent_name: &mut String,
    current_mode: &mut Box<dyn ModeRuntime>,
    registry: &Arc<AgentRegistry>,
) {
    match tokio::fs::read_to_string(session_path).await {
        Ok(json) => match serde_json::from_str::<ChatSession>(&json) {
            Ok(session) => {
                let agent_valid = registry.get(&session.agent_name).is_some();
                if !agent_valid {
                    eprintln!(
                        "{}{}{}",
                        ansi!("33"),
                        tf(
                            "cli.chat.session_load_agent_warn",
                            &[("agent", &session.agent_name)]
                        ),
                        ansi!("0")
                    );
                }
                *messages = session.messages;
                if agent_valid && session.agent_name != *current_agent_name {
                    if let Some(new_agent) = registry.get(&session.agent_name) {
                        *current_agent = new_agent;
                        *current_agent_name = session.agent_name.clone();
                        let mode_str = match current_mode.kind() {
                            ModeKind::Ask => "ask",
                            ModeKind::Plan => "plan",
                            ModeKind::Edit => "edit",
                            ModeKind::FullAuto => "full_auto",
                            ModeKind::SafeGuard => "safeguard",
                        };
                        if let Ok(runtime) = resolve_mode_runtime(
                            mode_str,
                            Some(registry.clone()),
                            Some(current_agent_name.clone()),
                        ) {
                            *current_mode = runtime;
                        }
                    }
                }
                if !session.mode.is_empty() {
                    let canonical = session.mode.to_lowercase();
                    if let Ok(runtime) = resolve_mode_runtime(
                        &canonical,
                        Some(registry.clone()),
                        Some(current_agent_name.clone()),
                    ) {
                        *current_mode = runtime;
                        eprintln!(
                            "{}{}{}",
                            ansi!("32"),
                            tf("cli.chat.restored_mode", &[("mode", &canonical)]),
                            ansi!("0")
                        );
                    }
                }
                eprintln!(
                    "{}{}{}",
                    ansi!("32"),
                    tf(
                        "cli.chat.session_loaded",
                        &[
                            ("count", &messages.len().to_string()),
                            ("agent", current_agent_name),
                            ("mode", &format!("{:?}", current_mode.kind())),
                        ]
                    ),
                    ansi!("0")
                );
            }
            Err(e) => eprintln!(
                "{}{}{}",
                ansi!("31"),
                tf(
                    "cli.chat.session_parse_failed",
                    &[("error", &e.to_string())]
                ),
                ansi!("0")
            ),
        },
        Err(_) => eprintln!(
            "{}{}{}",
            ansi!("33"),
            tf(
                "cli.chat.session_not_found",
                &[("path", &session_path.display().to_string())]
            ),
            ansi!("0")
        ),
    }
}

fn display_skills() {
    let descriptor_list = crate::orchestration::tool::skill_registry()
        .and_then(|r| r.read().ok())
        .map(|guard| guard.list(false))
        .unwrap_or_default();
    if descriptor_list.is_empty() {
        eprintln!("{}", t("cli.chat.no_skills"));
    } else {
        eprintln!(
            "{}",
            tf(
                "cli.chat.skills_count",
                &[("count", &descriptor_list.len().to_string())]
            )
        );
        for s in &descriptor_list {
            eprintln!("  {:<25} score: {:.2}", s.name, s.score);
        }
    }
}

fn display_stats(messages: &[Message], token_tracker: &TokenTracker) {
    let agent_msgs = messages.iter().filter(|m| m.role == "assistant").count();
    let user_msgs = messages.iter().filter(|m| m.role == "user").count();
    let total_chars: usize = messages.iter().map(|m| m.content.len()).sum();
    eprintln!("{}", t("cli.chat.stats_header"));
    eprintln!(
        "{}",
        tf(
            "cli.chat.stats_messages",
            &[
                ("total", &messages.len().to_string()),
                ("user", &user_msgs.to_string()),
                ("assistant", &agent_msgs.to_string()),
            ]
        )
    );
    eprintln!(
        "{}",
        tf(
            "cli.chat.stats_total_chars",
            &[("count", &total_chars.to_string())]
        )
    );
    eprintln!(
        "{}",
        tf(
            "cli.chat.stats_avg_length",
            &[(
                "count",
                &(if !messages.is_empty() {
                    total_chars / messages.len()
                } else {
                    0
                })
                .to_string()
            )]
        )
    );
    eprint!("{}", token_tracker.display());
}

fn display_context(messages: &[Message]) {
    let total_chars: usize = messages.iter().map(|m| m.content.len()).sum();
    let est_tokens: usize = messages.iter().map(|m| estimate_tokens(&m.content)).sum();
    let system_msgs = messages.iter().filter(|m| m.role == "system").count();
    eprintln!("{}", t("cli.chat.context_header"));
    eprintln!(
        "{}",
        tf(
            "cli.chat.context_messages",
            &[
                ("total", &messages.len().to_string()),
                ("system", &system_msgs.to_string()),
                (
                    "user",
                    &messages
                        .iter()
                        .filter(|m| m.role == "user")
                        .count()
                        .to_string()
                ),
                (
                    "assistant",
                    &messages
                        .iter()
                        .filter(|m| m.role == "assistant")
                        .count()
                        .to_string()
                ),
            ]
        )
    );
    eprintln!(
        "{}",
        tf(
            "cli.chat.context_chars",
            &[
                ("count", &total_chars.to_string()),
                ("tokens", &est_tokens.to_string()),
            ]
        )
    );
    eprintln!(
        "{}",
        tf(
            "cli.chat.context_used_pct",
            &[(
                "pct",
                &format!("{:.1}", (est_tokens as f64 / 128_000.0 * 100.0).min(100.0))
            )]
        )
    );
    if messages.len() >= COMPACT_PROMPT_THRESHOLD {
        eprintln!(
            "{}",
            tf(
                "cli.chat.context_compact_tip",
                &[
                    ("open", ansi!("33")),
                    ("close", ansi!("0")),
                    ("current", &messages.len().to_string()),
                    ("threshold", &COMPACT_PROMPT_THRESHOLD.to_string()),
                ]
            )
        );
    }
}

fn display_models(current_agent: &Arc<dyn Agent>, current_agent_name: &str) {
    let models = current_agent.available_models();
    if models.is_empty() {
        eprintln!(
            "{}{}{}",
            ansi!("33"),
            tf("cli.chat.no_models", &[("agent", current_agent_name)]),
            ansi!("0")
        );
    } else {
        eprintln!(
            "{}{}{}:",
            ansi!("1"),
            tf("cli.chat.models_header", &[("agent", current_agent_name)]),
            ansi!("0")
        );
        for m in &models {
            let default_flag = if m.is_default { " (default)" } else { "" };
            eprintln!("  {:<30} {} {}{}", m.id, m.name, ansi!("90"), default_flag);
        }
    }
}

async fn execute_diff_command(cmd: &str) {
    // cmd is the part after '/', e.g. "diff" or "diff src/"
    let path_filter = cmd
        .strip_prefix("diff ")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let mut git_cmd = tokio::process::Command::new("git");
    git_cmd.arg("diff");
    if let Some(filter) = path_filter {
        git_cmd.arg("--").arg(filter);
    }
    match git_cmd.output().await {
        Ok(out) => {
            let diff = String::from_utf8_lossy(&out.stdout);
            if diff.trim().is_empty() {
                eprintln!(
                    "{}No changes to display.{} (stderr: {})",
                    ansi!("33"),
                    ansi!("0"),
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            } else {
                display_diff(&diff, None);
            }
        }
        Err(e) => eprintln!(
            "{}{}{}",
            ansi!("31"),
            tf("cli.chat.git_diff_failed", &[("reason", &e.to_string())]),
            ansi!("0")
        ),
    }
}

async fn execute_compact_command(messages: &mut Vec<Message>, current_agent: &Arc<dyn Agent>) {
    if messages.len() < 4 {
        eprintln!(
            "{}Conversation too short to compact.{}",
            ansi!("33"),
            ansi!("0")
        );
        return;
    }
    let keep_front = 1.min(messages.len());
    let keep_back = 2.min(messages.len().saturating_sub(keep_front));
    let compact_range = keep_front..(messages.len() - keep_back);
    let compact_count = compact_range.len();
    if compact_count == 0 {
        eprintln!("{}No messages to compact.{}", ansi!("33"), ansi!("0"));
        return;
    }

    eprintln!(
        "{}Summarizing {} messages with LLM...{}",
        ansi!("33"),
        compact_count,
        ansi!("0")
    );

    let to_compact: Vec<Message> = messages[compact_range.clone()].to_vec();
    let summarize_prompt = Message {
        role: "user".to_string(),
        content: format!(
            "Please provide a concise summary of the above conversation. \
             Focus on: what has been accomplished, what decisions were made, \
             what the current state of the project/task is, and what remains to be done. \
             This summary replaces {} conversation turns, so include enough detail \
             (file paths, important findings, key decisions) that the conversation \
             can continue seamlessly without losing context.",
            compact_count
        ),
    };

    let mut summarize_msgs = to_compact;
    summarize_msgs.push(summarize_prompt);

    let (summary_tx, mut summary_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let summary_sender = crate::agents::agent::StreamingSender::from(summary_tx);
    let agent_for_summary = Arc::clone(current_agent);
    let summarize_task = tokio::spawn(async move {
        agent_for_summary
            .chat(summarize_msgs, None, None, summary_sender)
            .await
    });

    let mut summary_text = String::new();
    while let Some(token) = summary_rx.recv().await {
        summary_text.push_str(&token);
    }

    if let Err(e) = summarize_task.await {
        eprintln!(
            "{}{}{}",
            ansi!("31"),
            tf(
                "cli.chat.summarization_failed",
                &[("reason", &e.to_string())]
            ),
            ansi!("0")
        );
        return;
    }

    messages.drain(compact_range);
    messages.insert(
        1,
        Message {
            role: "user".to_string(),
            content: format!(
                "[Conversation compacted: summary of previous {} messages]\n{}",
                compact_count,
                summary_text.trim()
            ),
        },
    );
    eprintln!(
        "{}Compacted {} messages. {} messages remaining.{}",
        ansi!("32"),
        compact_count,
        messages.len(),
        ansi!("0")
    );
}

async fn execute_commit_command(
    _messages: &[Message],
    current_agent: &Arc<dyn Agent>,
    stdin_rx: &mut mpsc::Receiver<String>,
) {
    let (diff_output, full_diff) = match collect_git_diffs().await {
        Some(pair) => pair,
        None => return,
    };

    eprintln!(
        "{}Changes:{} {}",
        ansi!("1"),
        ansi!("0"),
        diff_output.trim()
    );

    let suggested_msg = generate_commit_message(current_agent, &diff_output, &full_diff).await;

    eprintln!(
        "\r{}✓ Message generated{} {}",
        ansi!("32"),
        ansi!("0"),
        suggested_msg
    );
    eprint!(
        "  {}Press Enter to commit, type a custom message, or n/N to cancel: {} ",
        ansi!("90"),
        ansi!("0")
    );
    std::io::Write::flush(&mut std::io::stdout()).ok();

    let user_line = tokio::select! {
        line = stdin_rx.recv() => line.unwrap_or_default(),
        _ = signal::ctrl_c() => { eprintln!("\nCancelled."); return; }
    };
    let trimmed = user_line.trim().to_string();

    if trimmed.eq_ignore_ascii_case("n") || trimmed.eq_ignore_ascii_case("no") {
        eprintln!("{}Commit cancelled.{}", ansi!("33"), ansi!("0"));
        return;
    }

    let final_msg = if trimmed.is_empty() {
        suggested_msg
    } else {
        trimmed
    };

    stage_and_commit(&final_msg).await;
}

/// Run `git diff --stat` and `git diff` to collect change information.
/// Returns `None` if the diff failed or there is nothing to commit.
async fn collect_git_diffs() -> Option<(String, String)> {
    let diff_output = match tokio::process::Command::new("git")
        .args(["diff", "--stat"])
        .output()
        .await
    {
        Ok(out) => String::from_utf8_lossy(&out.stdout).to_string(),
        Err(e) => {
            eprintln!(
                "{}{}{}",
                ansi!("31"),
                tf("cli.chat.git_diff_failed", &[("reason", &e.to_string())]),
                ansi!("0")
            );
            return None;
        }
    };
    if diff_output.trim().is_empty() {
        eprintln!("{}Nothing to commit.{}", ansi!("33"), ansi!("0"));
        return None;
    }

    let full_diff = match tokio::process::Command::new("git")
        .arg("diff")
        .output()
        .await
    {
        Ok(out) => {
            let s = String::from_utf8_lossy(&out.stdout).to_string();
            if s.len() > 8000 {
                format!("{}...\n[truncated]", &s[..8000])
            } else {
                s
            }
        }
        Err(_) => String::new(),
    };

    Some((diff_output, full_diff))
}

/// Generate a commit message using the agent or a fallback heuristic.
async fn generate_commit_message(
    agent: &Arc<dyn Agent>,
    diff_output: &str,
    full_diff: &str,
) -> String {
    eprint!("{}Generating commit message...{}", ansi!("90"), ansi!("0"));
    std::io::Write::flush(&mut std::io::stdout()).ok();

    let prompt_msg = Message {
        role: "user".to_string(),
        content: format!(
            "Generate a single-line conventional commit message for these changes.\
             \nFormat: <type>(<scope>): <description>\
             \nExamples:\
             \n  feat(api): add user authentication endpoint\
             \n  fix(cache): resolve TTL race condition\
             \n  refactor(cli): simplify command dispatch\
             \n  docs(readme): update installation steps\
             \n\nReturn ONLY the commit message, nothing else.\n\n{}",
            if full_diff.is_empty() {
                diff_output
            } else {
                full_diff
            }
        ),
    };

    match chat_simple(agent, vec![prompt_msg], vec![]).await {
        Ok(m) => m,
        Err(e) => {
            eprintln!(
                "\r{}AI commit message failed: {} — using fallback{}",
                ansi!("31"),
                e,
                ansi!("0")
            );
            format!(
                "feat: {}",
                diff_output.lines().filter(|l| l.contains('|')).count()
            )
        }
    }
}

/// Stage all changes and commit with the given message.
async fn stage_and_commit(msg: &str) {
    let stage_ok = tokio::process::Command::new("git")
        .args(["add", "-A"])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !stage_ok {
        eprintln!("{}Failed to stage changes.{}", ansi!("31"), ansi!("0"));
        return;
    }

    match tokio::process::Command::new("git")
        .args(["commit", "-m", msg])
        .output()
        .await
    {
        Ok(out) if out.status.success() => {
            eprintln!(
                "{}✓ Committed{}{}",
                ansi!("32"),
                ansi!("0"),
                String::from_utf8_lossy(&out.stdout).trim()
            );
        }
        Ok(out) => eprintln!(
            "{}Commit failed: {}{}",
            ansi!("31"),
            String::from_utf8_lossy(&out.stderr).trim(),
            ansi!("0")
        ),
        Err(e) => eprintln!(
            "{}{}{}",
            ansi!("31"),
            tf("cli.chat.git_diff_failed", &[("reason", &e.to_string())]),
            ansi!("0")
        ),
    }
}

#[allow(clippy::borrowed_box)]
async fn execute_plan_command(
    messages: &mut Vec<Message>,
    current_agent: &Arc<dyn Agent>,
    registry: &Arc<AgentRegistry>,
    current_agent_name: &str,
    _current_mode: &Box<dyn ModeRuntime>,
) {
    if messages.is_empty() {
        eprintln!(
            "{}No conversation to derive a plan from.{}",
            ansi!("33"),
            ansi!("0")
        );
        return;
    }

    let plan_runtime = resolve_mode_runtime(
        "plan",
        Some(registry.clone()),
        Some(current_agent_name.to_string()),
    );
    match plan_runtime {
        Ok(plan_mode) => {
            eprint!(
                "{}Generating execution plan with Plan mode constraints...{}",
                ansi!("90"),
                ansi!("0")
            );
            std::io::Write::flush(&mut std::io::stdout()).ok();

            let max_calls = plan_mode.max_tool_calls();
            let allowed = plan_mode.allowed_tools();
            eprintln!(
                "{}[Plan Mode] max_tool_calls={}, allowed_tools={:?}{}",
                ansi!("90"),
                max_calls,
                allowed,
                ansi!("0")
            );

            let plan_principles = build_cli_principles();
            match run_agent_with_tools(
                current_agent,
                messages,
                plan_principles,
                Some(plan_mode.as_ref()),
            )
            .await
            {
                Ok((plan, _, _)) => {
                    eprintln!(
                        "\r{}── Execution Plan (Plan Mode) ──{}",
                        ansi!("1"),
                        ansi!("0")
                    );
                    eprintln!("{}", plan);
                }
                Err(e) => eprintln!(
                    "\r{}Plan generation failed: {}{}",
                    ansi!("31"),
                    e,
                    ansi!("0")
                ),
            }
        }
        Err(e) => {
            eprintln!(
                "{}Failed to create Plan runtime: {}. Falling back to simple chat.{}",
                ansi!("31"),
                e,
                ansi!("0")
            );
            let context: Vec<String> = messages
                .iter()
                .filter(|m| m.role != "system")
                .take(10)
                .map(|m| {
                    format!(
                        "{}: {}",
                        m.role,
                        m.content.chars().take(500).collect::<String>()
                    )
                })
                .collect();
            let context_str = context.join("\n---\n");
            let plan_prompt_msg = Message {
                role: "user".to_string(),
                content: format!(
                    "Based on this conversation, create a structured execution plan.\
                     \nList specific steps with file paths where relevant.\
                     \nFormat as a numbered list.\n\nConversation:\n{}",
                    context_str
                ),
            };
            match chat_simple(current_agent, vec![plan_prompt_msg], vec![]).await {
                Ok(plan) => {
                    eprintln!(
                        "\r{}── Execution Plan (fallback) ──{}",
                        ansi!("1"),
                        ansi!("0")
                    );
                    eprintln!("{}", plan);
                }
                Err(e) => eprintln!(
                    "\r{}Plan generation failed: {}{}",
                    ansi!("31"),
                    e,
                    ansi!("0")
                ),
            }
        }
    }
}

async fn execute_review_command(current_agent: &Arc<dyn Agent>) {
    let detailed = match tokio::process::Command::new("git")
        .args(["diff"])
        .output()
        .await
    {
        Ok(out) => String::from_utf8_lossy(&out.stdout).to_string(),
        Err(_) => String::new(),
    };
    if detailed.trim().is_empty() {
        eprintln!("{}No changes to review.{}", ansi!("33"), ansi!("0"));
        return;
    }

    let stat_lines: Vec<&str> = detailed
        .lines()
        .filter(|l| {
            l.contains('|')
                && !l.starts_with("diff ")
                && !l.starts_with("index ")
                && !l.starts_with("---")
                && !l.starts_with("+++")
                && !l.starts_with("@@")
        })
        .collect();
    if !stat_lines.is_empty() {
        eprintln!(
            "{}── Changes ({} file(s)) ──{}",
            ansi!("1"),
            stat_lines.len(),
            ansi!("0")
        );
        for line in &stat_lines {
            eprintln!("  {}", line);
        }
        eprintln!();
    }

    eprint!("{}Reviewing changes with AI...{}", ansi!("90"), ansi!("0"));
    std::io::Write::flush(&mut std::io::stdout()).ok();

    let truncated_diff = if detailed.len() > 12000 {
        format!(
            "{}...\n[truncated: {} total bytes]",
            &detailed[..12000],
            detailed.len()
        )
    } else {
        detailed.clone()
    };

    let review_prompt = Message {
        role: "user".to_string(),
        content: format!(
            "Review this git diff for bugs, security issues, code quality, and improvement suggestions.\
             \nBe concise but specific. Point to exact lines where issues exist.\
             \nIf the code looks good, say so briefly.\n\n```diff\n{}\n```",
            truncated_diff
        ),
    };

    match chat_simple(current_agent, vec![review_prompt], vec![]).await {
        Ok(review) => {
            eprintln!("\r{}── AI Code Review ──{}", ansi!("1"), ansi!("0"));
            eprintln!("{}", review);
        }
        Err(e) => {
            eprintln!(
                "\r{}{}{}",
                ansi!("31"),
                tf("cli.chat.ai_review_failed", &[("reason", &e.to_string())]),
                ansi!("0")
            );
            display_diff(&detailed, Some(60));
        }
    }
}

async fn execute_find_path_command(find_cmd: &str) {
    let pattern = find_cmd
        .strip_prefix("find_path ")
        .or_else(|| find_cmd.strip_prefix("find "))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    match pattern {
        Some(pattern) => {
            let args = serde_json::json!({"pattern": pattern});
            match execute_simple_tool("search_files", &args).await {
                Ok(result) => eprintln!("{}", result),
                Err(e) => eprintln!("{}Error: {}{}", ansi!("31"), e, ansi!("0")),
            }
        }
        None => eprintln!("{}", t("cli.chat.find_path_usage")),
    }
}

async fn execute_mode_command(
    mode_cmd: &str,
    current_mode: &mut Box<dyn ModeRuntime>,
    registry: &Arc<AgentRegistry>,
    current_agent_name: &str,
) {
    let rest = mode_cmd.strip_prefix("mode").unwrap_or("");
    let name = if rest.is_empty() || rest == " " {
        ""
    } else {
        rest.trim()
    };
    if name.is_empty() {
        eprintln!("{}", t("cli.chat.available_modes"));
        eprintln!(
            "{}",
            tf(
                "cli.chat.current_mode",
                &[("mode", &format!("{:?}", current_mode.kind()))]
            )
        );
        eprintln!("{}", t("cli.chat.usage_mode"));
    } else {
        let canonical = match name.to_lowercase().as_str() {
            "edit" => "edit",
            "ask" => "ask",
            "plan" => "plan",
            "safeguard" | "safe_guard" => "safeguard",
            "full_auto" | "fullauto" => "full_auto",
            _ => {
                eprintln!(
                    "{}{}{}",
                    ansi!("31"),
                    tf("cli.chat.unknown_mode", &[("mode", name)]),
                    ansi!("0")
                );
                return;
            }
        };
        match resolve_mode_runtime(
            canonical,
            Some(registry.clone()),
            Some(current_agent_name.to_string()),
        ) {
            Ok(runtime) => {
                *current_mode = runtime;
                eprintln!(
                    "{}{}{}",
                    ansi!("32"),
                    tf("cli.chat.switched_mode", &[("mode", canonical)]),
                    ansi!("0")
                );
                // Persist mode to config for next session
                let config_path = std::path::Path::new("goon-cli-mode.json");
                if let Ok(content) = std::fs::read_to_string(config_path) {
                    if let Ok(mut state) = serde_json::from_str::<serde_json::Value>(&content) {
                        if let Some(obj) = state.as_object_mut() {
                            obj.insert("mode".to_string(), serde_json::json!(canonical));
                            if let Ok(json) = serde_json::to_string_pretty(&state) {
                                let _ = std::fs::write(config_path, json);
                            }
                        }
                    }
                } else {
                    let state = serde_json::json!({"mode": canonical});
                    if let Ok(json) = serde_json::to_string_pretty(&state) {
                        let _ = std::fs::write(config_path, json);
                    }
                }
                match current_mode.kind() {
                    ModeKind::SafeGuard => eprintln!(
                        "{}{}{}",
                        ansi!("90"),
                        t("cli.chat.mode_safeguard_desc"),
                        ansi!("0")
                    ),
                    ModeKind::FullAuto => eprintln!(
                        "{}{}{}",
                        ansi!("33"),
                        t("cli.chat.mode_full_auto_desc"),
                        ansi!("0")
                    ),
                    ModeKind::Edit => eprintln!(
                        "{}{}{}",
                        ansi!("90"),
                        t("cli.chat.mode_edit_desc"),
                        ansi!("0")
                    ),
                    _ => {}
                }
            }
            Err(e) => eprintln!(
                "{}{}{}",
                ansi!("31"),
                tf("cli.chat.mode_switch_failed", &[("reason", &e.to_string())]),
                ansi!("0")
            ),
        }
    }
}

#[allow(clippy::borrowed_box)]
async fn execute_retry_command(
    messages: &mut Vec<Message>,
    current_agent: &Arc<dyn Agent>,
    current_mode: &Box<dyn ModeRuntime>,
    token_tracker: &mut TokenTracker,
) {
    if messages.len() < 2 {
        eprintln!(
            "{}{}{}",
            ansi!("33"),
            t("cli.chat.no_messages_retry"),
            ansi!("0")
        );
        return;
    }
    let last_user_idx = messages.iter().rposition(|m| m.role == "user");
    match last_user_idx {
        Some(idx) => {
            let last_user_msg = messages[idx].content.clone();
            messages.truncate(idx + 1);
            let preview: String = last_user_msg.chars().take(60).collect();
            eprintln!(
                "{}{}{}",
                ansi!("33"),
                tf("cli.chat.retrying_message", &[("preview", &preview)]),
                ansi!("0")
            );
            let principles = build_cli_principles();
            match run_agent_with_tools(
                current_agent,
                messages,
                principles,
                Some(current_mode.as_ref()),
            )
            .await
            {
                Ok((resp, prompt_tokens, completion_tokens)) => {
                    token_tracker.record_usage(prompt_tokens, completion_tokens);
                    if !resp.trim().is_empty() {
                        eprintln!(
                            "{}{}{}",
                            ansi!("90"),
                            tf(
                                "cli.chat.turn_complete",
                                &[("tokens", &(prompt_tokens + completion_tokens).to_string())]
                            ),
                            ansi!("0")
                        );
                    }
                }
                Err(e) => eprintln!(
                    "\n{}{}{}",
                    ansi!("31"),
                    tf("cli.chat.retry_failed", &[("reason", &e.to_string())]),
                    ansi!("0")
                ),
            }
        }
        None => eprintln!(
            "{}{}{}",
            ansi!("33"),
            t("cli.chat.no_user_message_retry"),
            ansi!("0")
        ),
    }
}

async fn execute_switch_agent(
    model_cmd: &str,
    current_agent: &mut Arc<dyn Agent>,
    current_agent_name: &mut String,
    current_mode: &mut Box<dyn ModeRuntime>,
    registry: &Arc<AgentRegistry>,
) {
    let rest = model_cmd.strip_prefix("model").unwrap_or("");
    let name = if rest.is_empty() || rest == " " {
        ""
    } else {
        rest.trim()
    };
    if name.is_empty() {
        let names = registry.names();
        eprintln!(
            "{}",
            tf("cli.chat.available_agents", &[("names", &names.join(", "))])
        );
        eprintln!(
            "{}",
            tf("cli.chat.current_agent", &[("name", current_agent_name)])
        );
        eprintln!("{}", t("cli.chat.usage_model"));
    } else if let Some(new_agent) = registry.get(name) {
        *current_agent = new_agent;
        *current_agent_name = name.to_string();
        let mode_str = match current_mode.kind() {
            ModeKind::Ask => "ask",
            ModeKind::Plan => "plan",
            ModeKind::Edit => "edit",
            ModeKind::FullAuto => "full_auto",
            ModeKind::SafeGuard => "safeguard",
        };
        if let Ok(runtime) = resolve_mode_runtime(
            mode_str,
            Some(registry.clone()),
            Some(current_agent_name.clone()),
        ) {
            *current_mode = runtime;
        }
        eprintln!(
            "{}{}{}",
            ansi!("32"),
            tf("cli.chat.switched_agent", &[("name", name)]),
            ansi!("0")
        );
    } else {
        let names = registry.names();
        eprintln!(
            "{}{}{}",
            ansi!("31"),
            tf(
                "cli.chat.agent_not_found",
                &[("name", name), ("names", &names.join(", "))]
            ),
            ansi!("0")
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helper functions for session management
// ─────────────────────────────────────────────────────────────────────────────

/// Auto-save the session after each turn.
#[allow(clippy::borrowed_box)]
fn auto_save_turn(
    messages: &[Message],
    current_agent_name: &str,
    current_mode: &Box<dyn ModeRuntime>,
    session_path: &std::path::Path,
) {
    if messages.is_empty() || SAVE_IN_FLIGHT.load(Ordering::Acquire) {
        return;
    }
    SAVE_IN_FLIGHT.store(true, Ordering::Release);
    let session = ChatSession {
        messages: messages.to_vec(),
        agent_name: current_agent_name.to_string(),
        mode: format!("{:?}", current_mode.kind()).to_lowercase(),
    };
    let json = serde_json::to_string(&session).unwrap_or_default();
    let path = session_path.to_path_buf();
    let guard = AutoSaveGuard;
    tokio::spawn(async move {
        if let Err(e) = tokio::fs::write(&path, &json).await {
            tracing::warn!("Failed to auto-save session: {e}");
        }
        drop(guard);
    });
}

/// Check conversation length and auto-compact if needed (SlidingWindow).
/// Keeps the last AUTO_COMPACT_KEEP messages when threshold is exceeded.
fn check_compact_threshold(messages: &mut Vec<Message>) {
    let msg_count = messages.len();
    if (COMPACT_PROMPT_THRESHOLD..AUTO_COMPACT_THRESHOLD).contains(&msg_count) {
        eprintln!("{}{}{}", ansi!("33"), t("cli.chat.tip_compact"), ansi!("0"));
    }
    if msg_count >= AUTO_COMPACT_THRESHOLD {
        let keep = AUTO_COMPACT_KEEP;
        let remove_count = msg_count.saturating_sub(keep);
        // Keep the most recent `keep` messages (drop oldest)
        messages.drain(..remove_count);
        eprintln!(
            "{}{}{}",
            ansi!("32"),
            tf(
                "cli.chat.conversation_auto_compacted",
                &[
                    ("removed", &remove_count.to_string()),
                    ("remaining", &messages.len().to_string()),
                ]
            ),
            ansi!("0")
        );
    }
}

/// Save the session on exit.
#[allow(clippy::borrowed_box)]
async fn save_session_on_exit(
    messages: &[Message],
    current_agent_name: &str,
    current_mode: &Box<dyn ModeRuntime>,
    session_path: &std::path::Path,
) {
    if messages.is_empty() {
        return;
    }
    if SAVE_IN_FLIGHT.load(Ordering::Acquire) {
        tokio::select! {
            _ = save_notify().notified() => {}
            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
        }
    }
    let session = ChatSession {
        messages: messages.to_vec(),
        agent_name: current_agent_name.to_string(),
        mode: format!("{:?}", current_mode.kind()).to_lowercase(),
    };
    let json = serde_json::to_string(&session).unwrap_or_default();
    if let Err(e) = tokio::fs::write(session_path, &json).await {
        tracing::warn!("Failed to save session on exit: {e}");
    } else {
        eprintln!("Session auto-saved");
    }
}

/// Run a single agent turn: agent chat → tool execution → followup.
/// Returns the response text and estimated token usage.
///
/// `principles` is passed in (pre-computed by caller) to avoid rebuilding
/// the tool/skill list twice per round (once for agent chat, once for follow-up).
/// The tool registry is static, so the list never changes between calls.
async fn run_agent_with_tools(
    agent: &Arc<dyn Agent>,
    messages: &mut Vec<Message>,
    principles: Vec<String>,
    mode_runtime: Option<&dyn ModeRuntime>,
) -> Result<(String, usize, usize)> {
    // ── Estimate prompt tokens from existing messages using CJK-aware estimator ──
    let estimated_prompt_tokens: usize = messages.iter().map(|m| estimate_tokens(&m.content)).sum();

    // ── Phase 1: Agent streaming with Ctrl+C interrupt + reasoning + markdown ──
    let (mut response, tool_calls) =
        run_agent_streaming_phase(agent, messages, &principles).await?;

    // ── Phase 2 (inline): Filter/block tool calls by mode constraints + SafeGuard ──
    let filtered_calls = filter_tool_calls_by_mode(&tool_calls, mode_runtime);
    let (filtered_calls, early_exit_tokens) =
        safeguard_approval(&filtered_calls, mode_runtime, &response)?;
    if let Some(tokens) = early_exit_tokens {
        let estimated_completion_tokens = estimate_tokens(&response);
        return Ok((response, tokens, estimated_completion_tokens));
    }

    // ── Phase 3: Tool execution with FuturesUnordered + semaphore ──
    let (tool_results, has_failure, followup_round_executed) =
        run_tool_execution_phase(filtered_calls).await;

    // ── Phase 4: Send tool results back as follow-up message ──
    if !tool_results.is_empty() {
        response = run_followup_phase(
            agent,
            messages,
            &principles,
            &tool_results,
            has_failure,
            &response,
        )
        .await;
    }

    // ── Append assistant response to history ──
    if !response.is_empty() {
        let last_is_assistant = messages
            .last()
            .map(|m| m.role == "assistant")
            .unwrap_or(false);
        if !last_is_assistant {
            messages.push(Message {
                role: "assistant".to_string(),
                content: response.clone(),
            });
        }
    }

    let autonomy_contract =
        terminal_chat_contract_snapshot(tool_calls.len(), followup_round_executed, &response);
    debug!(
        target: "go_on::cli::chat",
        autonomy_contract = %autonomy_contract,
        "terminal chat turn completed"
    );

    let estimated_completion_tokens = estimate_tokens(&response);
    Ok((
        response,
        estimated_prompt_tokens,
        estimated_completion_tokens,
    ))
}

/// Phase 1: Stream the agent response with progressive markdown rendering,
/// reasoning markers, tool call notifications, and Ctrl+C interrupt handling.
/// Returns the collected response text and any tool calls emitted by the agent.
async fn run_agent_streaming_phase(
    agent: &Arc<dyn Agent>,
    messages: &[Message],
    principles: &[String],
) -> Result<(String, Vec<(String, String)>)> {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let sender = StreamingSender::from(tx);
    let msgs = messages.to_vec();
    let initial_principles = if principles.is_empty() {
        None
    } else {
        Some(principles.to_vec())
    };

    // ── Cancellation support for Ctrl+C ──
    let agent_ref = Arc::clone(agent);
    let chat_task =
        tokio::spawn(async move { agent_ref.chat(msgs, initial_principles, None, sender).await });

    // Use a forwarding channel: progressive display loop sends all tokens
    // to the shared `collect_agent_responses` for final classification.
    let (fwd_tx, fwd_rx) = mpsc::unbounded_channel::<String>();

    let mut renderer = StreamMarkdownRenderer::new();
    let mut in_reasoning = false;
    let mut _thinking_buffer = String::new();

    // ── Progressive streaming display with interrupt support ──
    loop {
        // Re-arm Ctrl+C each iteration: signal::ctrl_c() is a one-shot future.
        // Without this, the second Ctrl+C would be ignored.
        let ctrl_c = signal::ctrl_c();
        tokio::pin!(ctrl_c);
        tokio::select! {
            token = rx.recv() => {
                match token {
                    Some(token) => {
                        // Forward ALL tokens to the shared collector
                        let _ = fwd_tx.send(token.clone());

                        // Tool call notification
                        if let Some((tool_name, _)) = parse_tool_call_token(&token) {
                            eprintln!("{}🔧 [Tool call: {tool_name}]{}", ansi!("33"), ansi!("0"));
                            continue;
                        }

                        // Reasoning content markers
                        if token == REASONING_START {
                            in_reasoning = true;
                            continue;
                        }
                        if token == REASONING_END {
                            in_reasoning = false;
                            continue;
                        }

                        // __thinking__ prefixed tokens
                        if let Some(think) = token.strip_prefix(TOKEN_THINKING_PREFIX) {
                            eprint!("{}💭 {}{}", ansi!("90"), think, ansi!("0"));
                            _thinking_buffer.push_str(think);
                            continue;
                        }

                        // Skip finish_reason and usage telemetry tokens
                        if token.starts_with(TOKEN_FINISH_REASON_PREFIX)
                            || token.starts_with(TOKEN_USAGE_PREFIX)
                        {
                            continue;
                        }

                        if in_reasoning {
                            eprint!("{}💭 {}{}", ansi!("90"), token, ansi!("0"));
                            _thinking_buffer.push_str(&token);
                        } else {
                            renderer.feed(&token);
                            let (formatted, _) = renderer.flush();
                            if !formatted.is_empty() {
                                eprint!("{}", formatted);
                            }
                        }
                        std::io::Write::flush(&mut std::io::stdout()).ok();
                    }
                    None => break,
                }
            }
            _ = &mut ctrl_c => {
                eprintln!(
                    "\n{}Interrupted agent response. Use /clear to reset.{} ({})",
                    ansi!("33"), ansi!("0"),
                    if chat_task.is_finished() { "done" } else { "aborting" }
                );
                chat_task.abort();
                break;
            }
        }
    }

    // Drop the forwarding sender so the collector's receiver closes cleanly
    drop(fwd_tx);

    // Await the agent task
    match chat_task.await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => warn!("Agent chat failed: {e}"),
        Err(e) => {
            if e.is_cancelled() {
                debug!("Agent chat cancelled by user");
            } else {
                warn!("Agent chat task panicked: {e}");
            }
        }
    }

    // ── Collect the full response via shared core ──
    let CollectedResponse {
        response,
        reasoning: _reasoning_text,
        tool_calls,
    } = collect_agent_responses(fwd_rx).await.unwrap_or_else(|e| {
        warn!("collect_agent_responses failed: {e}");
        CollectedResponse {
            response: renderer.take_raw_response(),
            reasoning: String::new(),
            tool_calls: Vec::new(),
        }
    });

    // ── Flush remaining renderer output ──
    {
        let (remaining, _) = renderer.flush();
        if !remaining.is_empty() {
            let n = remaining.lines().count();
            for _ in 0..n {
                eprint!("\x1B[F\x1B[K");
            }
            eprintln!("{}", remaining);
        }
    }

    Ok((response, tool_calls))
}

/// Filter tool calls based on mode constraints (allowed tools, max_calls).
/// Returns the filtered list with blocked tools warned to stderr.
fn filter_tool_calls_by_mode(
    tool_calls: &[(String, String)],
    mode_runtime: Option<&dyn ModeRuntime>,
) -> Vec<(String, String)> {
    let max_calls = mode_runtime.map(|m| m.max_tool_calls()).unwrap_or(20);
    let allowed_tools: Vec<String> = mode_runtime.map(|m| m.allowed_tools()).unwrap_or_else(|| {
        // 默认允许所有已注册的工具
        tool_registry()
            .names()
            .iter()
            .map(|n| n.to_string())
            .collect()
    });

    let filtered_calls: Vec<(String, String)> = tool_calls
        .iter()
        .filter(|(name, _)| {
            if allowed_tools.contains(name) {
                true
            } else {
                eprintln!(
                    "{}{}{}",
                    ansi!("33"),
                    tf(
                        "cli.chat.tool_blocked_by_mode",
                        &[
                            ("tool_name", name),
                            ("allowed", &format!("{:?}", allowed_tools))
                        ]
                    ),
                    ansi!("0")
                );
                false
            }
        })
        .take(max_calls)
        .cloned()
        .collect();

    if filtered_calls.len() < tool_calls.len() {
        let blocked = tool_calls.len() - filtered_calls.len();
        let mode_str = mode_runtime
            .map(|m| m.kind())
            .map(|k| format!("{:?}", k))
            .unwrap_or_default();
        eprintln!(
            "{}{}{}",
            ansi!("33"),
            tf(
                "cli.chat.tool_call_blocked_by_mode",
                &[
                    ("blocked", &blocked.to_string()),
                    ("mode", &mode_str),
                    ("max", &max_calls.to_string())
                ]
            ),
            ansi!("0")
        );
    }

    filtered_calls
}

/// SafeGuard mode: interactive approval of high-risk operations.
/// Returns `(filtered_calls, Option<early_exit_prompt_tokens>)`.
/// If the user cancels, the second element contains the prompt token count for an early return.
fn safeguard_approval<'a>(
    filtered_calls: &'a [(String, String)],
    mode_runtime: Option<&dyn ModeRuntime>,
    _response: &str,
) -> SafeguardApprovalResult<'a> {
    let mode_kind = mode_runtime.map(|m| m.kind());
    let is_safeguard = matches!(mode_kind, Some(ModeKind::SafeGuard));
    let is_high_risk = if is_safeguard {
        mode_runtime
            .map(|m| {
                m.is_high_risk_operation(
                    &filtered_calls
                        .iter()
                        .map(|(n, a)| format!("{}: {}", n, a))
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            })
            .unwrap_or(false)
    } else {
        false
    };

    if is_safeguard && is_high_risk {
        eprintln!(
            "{}🔒 SafeGuard: High-risk operation detected. Review the planned tool calls:{} {}",
            ansi!("31"),
            ansi!("0"),
            filtered_calls
                .iter()
                .map(|(n, a)| format!("  ⚡ {}({})", n, a))
                .collect::<Vec<_>>()
                .join("\n")
        );
        eprint!(
            "{}Proceed with execution? [y/N]{} ",
            ansi!("33"),
            ansi!("0")
        );
        std::io::Write::flush(&mut std::io::stdout()).ok();
        let mut input = String::new();
        let _ = std::io::stdin().read_line(&mut input);
        if !input.trim().eq_ignore_ascii_case("y") {
            eprintln!(
                "{}SafeGuard: Operation cancelled by user.{}",
                ansi!("33"),
                ansi!("0")
            );
            return Ok((filtered_calls, Some(0)));
        }
    }

    Ok((filtered_calls, None))
}

/// Phase 2: Execute tools with progressive streaming via FuturesUnordered + Semaphore.
/// Returns (tool_results, has_failure, followup_round_executed).
async fn run_tool_execution_phase(
    filtered_calls: &[(String, String)],
) -> (Vec<String>, bool, bool) {
    let mut followup_round_executed = false;
    if filtered_calls.is_empty() {
        return (Vec::new(), false, followup_round_executed);
    }

    // ── Skill dedup: when AI calls multiple skills simultaneously, auto-select
    //    the one with the highest score and drop the rest. Non-skill tools are
    //    preserved. This mirrors the same logic in ACP's run_agent_collecting.
    let tool_calls = {
        let skill_names: Vec<&str> = filtered_calls
            .iter()
            .filter(|(name, _)| {
                // Only consider names that are registered as skills (not regular tools)
                crate::orchestration::tool::skill_registry()
                    .and_then(|r| r.read().ok())
                    .map(|guard| guard.get(name).is_some())
                    .unwrap_or(false)
            })
            .map(|(name, _)| name.as_str())
            .collect();
        if skill_names.len() > 1 {
            let best = {
                let reg = crate::orchestration::tool::skill_registry().and_then(|r| r.read().ok());
                reg.as_ref().and_then(|guard| {
                    skill_names
                        .iter()
                        .filter_map(|name| {
                            let score = guard.score_of(name).unwrap_or(0.5);
                            guard.get(name).map(|_| (name.to_string(), score))
                        })
                        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                })
            };
            if let Some((best_name, _)) = best {
                warn!(
                    "skill dedup: AI called {} skills ({}), auto-selecting '{}'",
                    skill_names.len(),
                    skill_names.join(", "),
                    best_name
                );
                eprintln!(
                    "  {}skill dedup: {} skills called ({}), auto-selected '{}'{}",
                    ansi!("33"),
                    skill_names.len(),
                    skill_names.join(", "),
                    best_name,
                    ansi!("0")
                );
                filtered_calls
                    .iter()
                    .filter(|(name, _)| {
                        // Keep all non-skill tools, and only the best skill
                        crate::orchestration::tool::skill_registry()
                            .and_then(|r| r.read().ok())
                            .map(|guard| {
                                if guard.get(name).is_some() {
                                    *name == best_name
                                } else {
                                    true
                                }
                            })
                            .unwrap_or(true)
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            } else {
                filtered_calls.to_vec()
            }
        } else {
            filtered_calls.to_vec()
        }
    };

    eprintln!("{}── Tool execution ──{}", ansi!("33"), ansi!("0"));

    let exec_result = execute_tools_concurrent(
        &tool_calls,
        tool_registry(),
        &ToolExecConfig {
            max_concurrency: MAX_CONCURRENT_TOOLS,
            circuit_breaker_limit: 0, // CLI handles failures inline
            operation_mode: "ask".to_string(),
            governance_required: false,
            is_safeguard: false,
            acp_session_id: None,
        },
        None, // no SSE progress in CLI
        "",
        0,
    )
    .await;

    let mut tool_results: Vec<String> = Vec::new();
    let mut has_failure = false;

    for item in &exec_result.tool_results {
        let tool_name = &item.tool_name;
        if item.success {
            // The executor returns formatted output; for CLI we need the raw
            // result text for terminal display. Re-extract from ToolOutput.
            let raw_text = item
                .output
                .result
                .as_ref()
                .and_then(|r| {
                    if let Some(s) = r.as_str() {
                        if !s.is_empty() {
                            return Some(s.to_string());
                        }
                    }
                    None
                })
                .unwrap_or_else(|| format!("{:?}", item.output));

            let display = if raw_text.len() > 500 {
                let end = raw_text
                    .char_indices()
                    .nth(500)
                    .map(|(i, _)| i)
                    .unwrap_or(raw_text.len());
                format!(
                    "{}...\n[{} chars truncated]  ({:.1}s)",
                    &raw_text[..end],
                    raw_text.len(),
                    item.duration_ms as f32 / 1000.0
                )
            } else {
                format!("{}  ({:.1}s)", raw_text, item.duration_ms as f32 / 1000.0)
            };
            eprintln!("    {}✓{} {}", ansi!("32"), ansi!("0"), display);

            let result_for_llm = if raw_text.len() > MAX_TOOL_RESULT_CHARS {
                tracing::warn!(
                    tool_name = %tool_name,
                    total_chars = raw_text.len(),
                    max_chars = MAX_TOOL_RESULT_CHARS,
                    "Tool result truncated for LLM"
                );
                let trunc_end = raw_text
                    .char_indices()
                    .nth(MAX_TOOL_RESULT_CHARS)
                    .map(|(i, _)| i)
                    .unwrap_or(raw_text.len());
                format!(
                    "{}...\n[truncated: {} total chars, showing first {}]",
                    &raw_text[..trunc_end],
                    raw_text.len(),
                    MAX_TOOL_RESULT_CHARS
                )
            } else {
                raw_text.clone()
            };
            tool_results.push(build_tool_result_block(tool_name, &result_for_llm, false));
        } else {
            has_failure = true;
            let err_text = item
                .output
                .error
                .as_deref()
                .unwrap_or("tool execution failed");
            eprintln!(
                "    {}✗ Error: {}{}  ({:.1}s)",
                ansi!("31"),
                err_text,
                ansi!("0"),
                item.duration_ms as f32 / 1000.0
            );
            tool_results.push(build_tool_result_block(tool_name, err_text, true));
        }
    }

    followup_round_executed = true;
    (tool_results, has_failure, followup_round_executed)
}

/// Phase 3: Send tool results back to agent as a follow-up message,
/// stream the follow-up response with markdown rendering + Ctrl+C interrupt.
///
/// Capabilities (matching ACP's `run_followup_after_tool_observation`):
/// - Timeout wrapping via `run_with_optional_timeout` (default 60s)
/// - Tool result count limited to `MAX_TOOLS_IN_FOLLOWUP` (8)
/// - Skill dedup is already handled in Phase 2 (`run_tool_execution_phase`)
/// - Streaming rendering with Ctrl+C interrupt
async fn run_followup_phase(
    agent: &Arc<dyn Agent>,
    messages: &mut Vec<Message>,
    principles: &[String],
    tool_results: &[String],
    has_failure: bool,
    response: &str,
) -> String {
    // ── Limit tool results to prevent message bloat (mirrors ACP max_tools_per_round) ──
    let limited_results: Vec<&String> = tool_results.iter().take(MAX_TOOLS_IN_FOLLOWUP).collect();
    let results_for_message: Vec<String> = limited_results.iter().map(|s| (*s).clone()).collect();
    if tool_results.len() > MAX_TOOLS_IN_FOLLOWUP {
        warn!(
            "Tool results truncated for follow-up: {} total, showing {}",
            tool_results.len(),
            MAX_TOOLS_IN_FOLLOWUP
        );
        eprintln!(
            "  {}⚠  Tool results truncated: {} total, showing first {}{}",
            ansi!("33"),
            tool_results.len(),
            MAX_TOOLS_IN_FOLLOWUP,
            ansi!("0")
        );
    }

    messages.push(Message {
        role: "assistant".to_string(),
        content: response.to_string(),
    });
    messages.push(Message {
        role: "user".to_string(),
        content: build_tool_execution_followup_message(&results_for_message, has_failure),
    });

    eprint!("{}── Agent follow-up ──{}\n🤖 ", ansi!("33"), ansi!("0"));
    std::io::Write::flush(&mut std::io::stdout()).ok();

    // ── Set up streaming channel and timeout ──
    let (tx2, mut rx2) = mpsc::unbounded_channel::<String>();
    let sender2 = StreamingSender::from(tx2);
    let msgs2 = messages.clone();
    let agent_ref2 = Arc::clone(agent);
    let followup_principles = if principles.is_empty() {
        None
    } else {
        Some(principles.to_vec())
    };

    let followup_task = tokio::spawn(async move {
        agent_ref2
            .chat(msgs2, followup_principles, None, sender2)
            .await
    });

    // ── Collect streaming tokens with timeout ──
    let timeout_duration = Duration::from_secs(DEFAULT_FOLLOWUP_TIMEOUT_SECS);
    let collect = async {
        let mut followup_renderer = StreamMarkdownRenderer::new();
        let mut in_reasoning2 = false;
        loop {
            tokio::select! {
                token = rx2.recv() => {
                    match token {
                        Some(token) => {
                            if token == REASONING_START {
                                in_reasoning2 = true;
                                eprint!("{}", ansi!("90"));
                                continue;
                            }
                            if token == REASONING_END {
                                in_reasoning2 = false;
                                eprint!("{}", ansi!("0"));
                                eprintln!();
                                continue;
                            }
                            // Tool call notification (same as primary phase)
                            if let Some((tool_name, _)) = parse_tool_call_token(&token) {
                                eprintln!("{}🔧 [Tool call: {tool_name}]{}", ansi!("33"), ansi!("0"));
                                continue;
                            }
                            if let Some(think) = token.strip_prefix(TOKEN_THINKING_PREFIX) {
                                eprint!("{}💭 {}{}", ansi!("90"), think, ansi!("0"));
                                continue;
                            }
                            // Skip finish_reason and usage telemetry tokens
                            if token.starts_with(TOKEN_FINISH_REASON_PREFIX)
                                || token.starts_with(TOKEN_USAGE_PREFIX)
                            {
                                continue;
                            }
                            if in_reasoning2 {
                                eprint!("{}{}{}", ansi!("90"), token, ansi!("0"));
                            } else {
                                followup_renderer.feed(&token);
                                let (formatted, _) = followup_renderer.flush();
                                if !formatted.is_empty() {
                                    eprint!("{}", formatted);
                                }
                            }
                            std::io::Write::flush(&mut std::io::stdout()).ok();
                        }
                        None => break,
                    }
                }
                _ = signal::ctrl_c() => {
                    eprintln!(
                        "\n{}Interrupted follow-up response.{}  [P3]",
                        ansi!("33"), ansi!("0")
                    );
                    followup_task.abort();
                    break;
                }
            }
        }

        if let Err(e) = followup_task.await {
            warn!("Agent followup task failed: {e}");
        }

        let rendered_final = {
            let (remaining, _) = followup_renderer.flush();
            if !remaining.is_empty() {
                let n = remaining.lines().count();
                for _ in 0..n {
                    eprint!("\x1B[F\x1B[K");
                }
                eprintln!("{}", remaining);
                remaining
            } else {
                followup_renderer.take_raw_response()
            }
        };

        Ok::<String, anyhow::Error>(rendered_final)
    };

    let result = run_with_optional_timeout(Some(timeout_duration), collect, |duration| {
        anyhow::anyhow!(
            "Agent follow-up timed out after {}s",
            duration.as_secs().max(1)
        )
    })
    .await;

    match result {
        Ok(rendered_final) if !rendered_final.trim().is_empty() => {
            crate::acp::helpers::autonomy_metrics::record_tool_followup_success();
            rendered_final
        }
        Ok(_) => {
            crate::acp::helpers::autonomy_metrics::record_tool_followup_fallback();
            response.to_string()
        }
        Err(e) => {
            warn!("Agent follow-up failed or timed out: {e}");
            eprintln!("{}⚠  Follow-up: {}{}  [P3]", ansi!("33"), e, ansi!("0"));
            crate::acp::helpers::autonomy_metrics::record_tool_followup_fallback();
            response.to_string()
        }
    }
}

/// Execute a tool by name and arguments, returning the result as a string.
///
/// # Governance
///
/// Before executing, a lightweight governance gate (`governance::status::quick_check_tool`)
/// validates the tool name and arguments against minimal safety policies.
/// In a stricter deployment (e.g. a managed service), operations should additionally
/// be routed through the full `SecurityGovernor` / `HarnessBus` pipeline.
async fn execute_simple_tool(name: &str, args: &Value) -> Result<String> {
    // ── Map aliases to canonical ToolRegistry names FIRST ──
    // Aliases must be resolved before the governance gate so that
    // names like "bash", "grep", "run" are checked under their
    // canonical form and not rejected by the gate's allowlist.
    // Registered names (including registry aliases like "bash") are used
    // as-is; the CLI-specific aliases below only apply to unregistered names
    // so the registered content `grep` tool is not shadowed by the filename
    // `search_files` alias.
    let canonical_name = if tool_registry().get(name).is_some() {
        name
    } else {
        match name {
            "read" => "read_file",
            "write" | "create" => "write_file",
            "search" | "grep" => "search_files",
            "ls" => "list_directory",
            "run" => "shell_exec",
            other => other,
        }
    };

    // ── Governance gate: validate canonicalized tool + arguments ──
    if let Err(reason) = governance_gate(canonical_name, args) {
        return Err(anyhow::anyhow!("governance denied: {reason}"));
    }

    // ── Normalize payload field names to what the canonical tools expect ──
    let mut payload = args.clone();
    if let Some(v) = payload.get("file_path").and_then(|v| v.as_str()) {
        payload["path"] = json!(v);
    }
    if let Some(v) = payload.get("query").and_then(|v| v.as_str()) {
        payload["pattern"] = json!(v);
    }
    if let Some(v) = payload.get("directory").and_then(|v| v.as_str()) {
        if canonical_name == "list_directory" {
            payload["path"] = json!(v);
        } else {
            payload["directory"] = json!(v);
        }
    }
    if let Some(v) = payload.get("cmd").and_then(|v| v.as_str()) {
        payload["command"] = json!(v);
    }

    // ── Validate required tool arguments before execution ──
    // Catches missing required parameters early and provides a clear error
    // message, rather than letting the tool implementation produce a generic
    // error that may confuse the LLM into repeating the same mistake.
    if let Err(validation_err) =
        crate::shared::tool_descriptors::validate_required_arguments(canonical_name, &payload)
    {
        return Err(anyhow::anyhow!(
            "{}: {}. Please check the tool schema and retry.",
            canonical_name,
            validation_err
        ));
    }

    // ── Resolve base dir for path traversal protection ──
    let allowed_base_dir = std::env::current_dir().ok();

    // ── Build ToolInput envelope and execute via ToolRegistry ──
    let input = ToolInput {
        task_id: "execute_simple_tool".to_string(),
        phase: "act".to_string(),
        agent_role: "coder".to_string(),
        objective: format!("Execute tool: {canonical_name}"),
        constraints: None,
        evidence: None,
        payload,
        allowed_base_dir,
    };

    let canonical_owned = canonical_name.to_string();

    // Use the profile's timeout budget when available, otherwise default to
    // 300 seconds (matching the most generous old bash timeout).
    let timeout_ms = tool_registry()
        .profile(&canonical_owned)
        .map(|p| p.timeout_budget_ms)
        .unwrap_or(300_000);
    let timeout_dur = Duration::from_millis(timeout_ms.max(5_000));

    // Use the async execution path directly — this is already inside an async fn.
    // Tools with sync implementations will automatically use spawn_blocking via
    // the default Tool::run_async implementation. Tools with native async
    // implementations (e.g. SkillExecuteTool) run without any blocking at all.
    // This eliminates the nested spawn_blocking + block_on anti-pattern.
    let output = timeout(timeout_dur, async {
        tool_registry()
            .run_with_fallback_async(&canonical_owned, &input)
            .await
    })
    .await
    .map_err(|_| anyhow::anyhow!("tool '{name}' timed out after {}ms", timeout_ms))??;

    if !output.success {
        return Err(anyhow::anyhow!(
            "{}",
            output
                .error
                .unwrap_or_else(|| "tool execution failed".to_string())
        ));
    }

    // ── Format ToolOutput into string ──
    // Use per-tool formatting for tools with non-trivial output structure,
    // fall back to pretty-printed JSON for all other tools.
    format_tool_output(canonical_name, &output)
}

/// Format a tool's output result into a human-readable string.
///
/// Only a few core tools have custom formatting; all others use pretty-printed JSON.
/// The ToolOutput.result is a JSON Value that each tool produces with consistent field names.
fn format_tool_output(tool_name: &str, output: &ToolOutput) -> Result<String> {
    let r = match output.result.as_ref() {
        Some(val) => val,
        None => {
            // Tool returned success but no result — return the error message if any,
            // otherwise a simple success acknowledgement.
            return Ok(output.error.as_deref().unwrap_or("success").to_string());
        }
    };

    match tool_name {
        // ── Read file: just the content ──
        "read_file" => Ok(r["content"].as_str().unwrap_or("").to_string()),

        // ── Write file: path summary ──
        "write_file" => {
            let path = r["path"].as_str().unwrap_or("unknown");
            Ok(format!("wrote file: {path}"))
        }

        // ── Search files: compact file list ──
        "search_files" => {
            let files: Vec<&str> = r["files"]
                .as_array()
                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            if files.is_empty() {
                Ok("No files matching pattern".to_string())
            } else {
                Ok(files.join("\n"))
            }
        }

        // ── List directory: formatted entries ──
        "list_directory" => {
            let mut entries: Vec<String> = r["entries"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(|e| {
                            let name = e["name"].as_str().unwrap_or("unknown");
                            let is_dir = e["is_directory"].as_bool().unwrap_or(false);
                            format!(" [{}] {name}", if is_dir { "dir" } else { "file" })
                        })
                        .collect()
                })
                .unwrap_or_default();
            entries.sort();
            Ok(entries.join("\n"))
        }

        // ── Shell exec: stdout + stderr + exit code ──
        "shell_exec" => format_cmd_output(r),

        // ── Apply patch: status + output ──
        "apply_patch" => {
            let applied = r["applied"].as_bool().unwrap_or(false);
            let checked = r["checked"].as_bool().unwrap_or(false);
            let mut buf = if applied {
                "patch applied successfully".to_string()
            } else if checked {
                "patch check completed".to_string()
            } else {
                String::new()
            };
            append_cmd_result(&mut buf, r);
            Ok(buf)
        }

        // ── Run tests: output + exit code + command ──
        "run_tests" => format_run_tests_output(r),

        // ── Git diff: diff content ──
        "inspect_git_diff" => format_inspect_git_diff_output(r),

        // ── Cargo check: structured errors/warnings ──
        "cargo_check" => format_cargo_check_output(r),

        // ── Generic fallback: pretty-printed JSON ──
        _ => Ok(serde_json::to_string_pretty(r).unwrap_or_default()),
    }
}

/// Format the output of a `run_tests` tool call.
fn format_run_tests_output(r: &serde_json::Value) -> Result<String> {
    use std::fmt::Write;
    let mut buf = match r["filter"].as_str() {
        Some(f) if !f.is_empty() => format!("filter: {f}"),
        _ => String::new(),
    };
    append_stdouterr(&mut buf, r);
    if let Some(code) = r["exit_code"].as_i64() {
        if !buf.is_empty() {
            buf.push('\n');
        }
        let _ = write!(buf, "exit code: {code}");
    }
    if let Some(cmd) = r["command"].as_str() {
        let _ = write!(buf, "\ncommand: {cmd}");
    }
    Ok(buf)
}

/// Format the output of an `inspect_git_diff` tool call.
fn format_inspect_git_diff_output(r: &serde_json::Value) -> Result<String> {
    let diff = r["diff"].as_str().unwrap_or("");
    let staged = r["staged"].as_bool().unwrap_or(false);
    let mut buf = if staged {
        "(staged diff)".to_string()
    } else {
        "(unstaged diff)".to_string()
    };
    if !diff.is_empty() {
        buf.push('\n');
        buf.push_str(diff);
    }
    if let Some(stderr) = r["stderr"].as_str() {
        if !stderr.is_empty() {
            buf.push('\n');
            buf.push_str(stderr);
        }
    }
    Ok(buf)
}

/// Format the output of a `cargo_check` tool call.
fn format_cargo_check_output(r: &serde_json::Value) -> Result<String> {
    use std::fmt::Write;
    let error_count = r["error_count"].as_u64().unwrap_or(0);
    let warning_count = r["warning_count"].as_u64().unwrap_or(0);
    let mut buf = format!("cargo check: {error_count} errors, {warning_count} warnings\n");
    if let Some(errors) = r["errors"].as_array() {
        for e in errors {
            if let Some(rendered) = e["rendered"].as_str() {
                let _ = write!(buf, "\n── ERROR ──\n{rendered}");
            }
        }
    }
    if let Some(warnings) = r["warnings"].as_array() {
        for w in warnings {
            if let Some(rendered) = w["rendered"].as_str() {
                let _ = write!(buf, "\n── WARNING ──\n{rendered}");
            }
        }
    }
    if let Some(code) = r["exit_code"].as_i64() {
        let _ = write!(buf, "\nexit code: {code}");
    }
    Ok(buf)
}

/// Cached build of CLI principles — rebuilt only when skills change.
/// Uses a generation counter so that principles are re-built only when
/// the skill registry content changes (detected via total skill count).
fn build_cli_principles() -> Vec<String> {
    static CACHED: std::sync::OnceLock<std::sync::RwLock<(Vec<String>, usize)>> =
        std::sync::OnceLock::new();
    let cache = CACHED.get_or_init(|| std::sync::RwLock::new((Vec::new(), usize::MAX)));

    // Detect skill registry change by current skill count
    let current_skill_count = crate::orchestration::tool::skill_registry()
        .and_then(|r| r.read().ok())
        .map(|g| g.list(false).len())
        .unwrap_or(0);

    if let Ok(guard) = cache.read() {
        if guard.1 == current_skill_count && !guard.0.is_empty() {
            return guard.0.clone();
        }
    }

    // Cache miss or expired — rebuild under write lock
    let mut principles = vec![
        "You are a helpful AI coding assistant with access to tools.".to_string(),
        "You can use the following tools via __tool_call__:tool_name:json_args protocol:"
            .to_string(),
    ];

    let tool_names = tool_registry().all_names();
    if !tool_names.is_empty() {
        principles.push(format!("  Built-in tools: {}", tool_names.join(", ")));
    }

    if let Some(registry) = crate::orchestration::tool::skill_registry() {
        if let Ok(guard2) = registry.read() {
            let skill_list = guard2.list(false);
            if !skill_list.is_empty() {
                let skill_names: Vec<String> = skill_list
                    .iter()
                    .map(|d| format!("{}: {}", d.name, d.description))
                    .collect();
                principles.push(format!("  Registered skills: {}", skill_names.join("; ")));
                principles.push(
                    "Use skill_execute tool with skill_name and input to invoke a skill."
                        .to_string(),
                );
            }
        }
    }

    if let Ok(mut guard) = cache.write() {
        guard.0 = principles.clone();
        guard.1 = current_skill_count;
    }
    principles
}

/// Display a git diff with ANSI color highlighting, optionally limited to `max_lines`.
fn display_diff(diff: &str, max_lines: Option<usize>) {
    let iter: Box<dyn Iterator<Item = &str>> = match max_lines {
        Some(n) => Box::new(diff.lines().take(n)),
        None => Box::new(diff.lines()),
    };
    for line in iter {
        if line.starts_with('+') && !line.starts_with("+++") {
            eprintln!("{}{}{}", ansi!("32"), line, ansi!("0"));
        } else if line.starts_with('-') && !line.starts_with("---") {
            eprintln!("{}{}{}", ansi!("31"), line, ansi!("0"));
        } else if line.starts_with("@@") {
            eprintln!("{}{}{}", ansi!("36"), line, ansi!("0"));
        } else {
            eprintln!("{}", line);
        }
    }
}

/// Append stdout + stderr to a buffer, separated by newline.
fn append_stdouterr(buf: &mut String, r: &serde_json::Value) {
    if let Some(stdout) = r["stdout"].as_str() {
        if !stdout.is_empty() {
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(stdout);
        }
    }
    if let Some(stderr) = r["stderr"].as_str() {
        if !stderr.is_empty() {
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(stderr);
        }
    }
}

/// Append stdout, stderr, and exit code to a buffer.
fn append_cmd_result(buf: &mut String, r: &serde_json::Value) {
    use std::fmt::Write;
    append_stdouterr(buf, r);
    if let Some(code) = r["exit_code"].as_i64() {
        if !buf.is_empty() {
            buf.push('\n');
        }
        let _ = write!(buf, "exit code: {code}");
    }
}

/// Send a simple prompt to the agent and collect the full response as a string.
///
/// Unlike `run_agent_with_tools`, this returns only the text response without
/// tool execution. Ideal for AI-powered commands like `/commit`, `/plan`, `/review`.
async fn chat_simple(
    agent: &Arc<dyn Agent>,
    prompt: Vec<Message>,
    principles: Vec<String>,
) -> Result<String> {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let sender = StreamingSender::from(tx);
    let principles_opt = if principles.is_empty() {
        None
    } else {
        Some(principles)
    };

    let agent_ref = Arc::clone(agent);
    tokio::spawn(async move { agent_ref.chat(prompt, principles_opt, None, sender).await });

    let mut response = String::new();
    while let Some(token) = rx.recv().await {
        // Skip reasoning markers and tool calls for simple chat
        if token == REASONING_START || token == REASONING_END {
            continue;
        }
        if parse_tool_call_token(&token).is_some() {
            continue;
        }
        // Strip __thinking__ prefix from reasoning tokens
        if let Some(think) = token.strip_prefix(TOKEN_THINKING_PREFIX) {
            eprintln!("{}💭 {}{}", ansi!("90"), think, ansi!("0"));
            continue;
        }
        // Skip finish_reason and usage telemetry tokens
        if token.starts_with(TOKEN_FINISH_REASON_PREFIX) || token.starts_with(TOKEN_USAGE_PREFIX) {
            continue;
        }
        response.push_str(&token);
    }
    Ok(response.trim().to_string())
}

/// Format a command execution output (stdout + stderr + exit code) into a string.
fn format_cmd_output(r: &serde_json::Value) -> Result<String> {
    let mut buf = String::new();
    append_cmd_result(&mut buf, r);
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── execute_simple_tool security ──────────────────────────────────

    /// Verify that `execute_simple_tool` rejects path traversal via the
    /// `name` field (read_file with traversal path).
    #[tokio::test]
    async fn test_execute_simple_tool_rejects_traversal() {
        let result =
            execute_simple_tool("read_file", &json!({"path": "../../../etc/passwd"})).await;
        assert!(
            result.is_err(),
            "read_file with traversal path should be rejected, got: {:?}",
            result
        );
        let err = result.unwrap_err().to_string();
        // Error can be either "canonicalization" or "traversal" depending
        // on whether /etc/passwd resolves before the base-dir check.
        assert!(
            err.contains("canonicalization")
                || err.contains("outside")
                || err.contains("denied")
                || err.contains("traversal"),
            "error should mention canonicalization, traversal, or denied, got: {}",
            err
        );
    }

    /// Verify that executing a simple tool with missing arguments returns
    /// a descriptive error.
    #[tokio::test]
    async fn test_execute_simple_tool_missing_arguments() {
        let err = execute_simple_tool("read_file", &json!({}))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("missing")
                || err.contains("required")
                || err.contains("requires")
                || err.contains("missing_path"),
            "error should mention missing/required field, got: {err}"
        );

        let err = execute_simple_tool("write_file", &json!({"path": "test.txt"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("missing")
                || err.contains("required")
                || err.contains("requires")
                || err.contains("missing_content"),
            "error should mention missing/required field, got: {err}"
        );
    }

    /// Verify that unknown tool names produce a descriptive error.
    #[tokio::test]
    async fn test_execute_simple_tool_unknown_tool() {
        let result = execute_simple_tool("nonexistent_tool", &json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string().to_lowercase();
        assert!(
            err.contains("not registered")
                || err.contains("unknown")
                || err.contains("governance")
                || err.contains("not found"),
            "error should indicate unknown tool, got: {err}"
        );
    }
}
