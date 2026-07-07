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

use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

use anyhow::Result;
use serde_json::{json, Value};
use tokio::signal;
use tokio::sync::{mpsc, Notify, Semaphore};
use tokio::time::{timeout, Duration};
use tracing::{debug, warn};

use crate::acp::helpers::autonomy::terminal_chat_contract_snapshot;
use crate::agents::agent::{Agent, AgentRegistry, Message, StreamingSender};
use crate::cli::markdown_renderer::StreamMarkdownRenderer;
use crate::config::AppConfig;

use crate::governance::status::quick_check_tool as governance_gate;
use crate::intelligence::capability_graph::CapabilityGraph;
use crate::orchestration::autonomy_runtime::{
    build_tool_execution_followup_message, build_tool_result_block, parse_tool_call_token,
    REASONING_END, REASONING_START,
};
use crate::orchestration::tool::{ToolInput, ToolOutput, ToolRegistry};

/// Maximum number of characters from a tool result sent to the LLM.
const MAX_TOOL_RESULT_CHARS: usize = 100_000;

/// Maximum number of concurrent tool executions.
/// Prevents resource exhaustion when the agent emits many parallel tool calls.
const MAX_CONCURRENT_TOOLS: usize = 10;
/// Session file name for conversation persistence (inside .goon/ directory).
const SESSION_FILE: &str = ".goon/chat-session.json";

/// Threshold at which we prompt the user to compact the conversation.
const COMPACT_PROMPT_THRESHOLD: usize = 30;

/// Threshold at which we automatically compact (requires user consent).
const AUTO_COMPACT_THRESHOLD: usize = 60;

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

/// Cached ToolRegistry — created once to avoid recreating ~100 tools per call.
static TOOL_REGISTRY: OnceLock<ToolRegistry> = OnceLock::new();

/// Returns the global ToolRegistry reference.
fn tool_registry() -> &'static ToolRegistry {
    TOOL_REGISTRY.get_or_init(ToolRegistry::default)
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
  /commit      Git commit with staged changes
  /review      Review current git diff before committing
  /plan        Show structured execution plan
  /find_path   Search for files by name glob

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
}

/// Estimate token count from text using a hybrid approach:
/// - CJK characters (East Asian) count as ~1.5 tokens each
/// - ASCII words count as ~1.3 tokens each
/// - Numbers/symbols count as ~0.5 tokens each
///
/// This is significantly more accurate than the naive `chars/4` approach.
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let mut cjk_chars = 0usize;
    let mut ascii_chars = 0usize;
    for ch in text.chars() {
        match ch {
            '\u{4E00}'..='\u{9FFF}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{2F800}'..='\u{2FA1F}' => cjk_chars += 1,
            _ => ascii_chars += 1,
        }
    }
    // CJK ~1.5 tokens/char, ASCII ~0.25 tokens/char (4 chars/token)
    (cjk_chars.saturating_mul(15) / 10) + (ascii_chars / 4)
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
        eprintln!("No AI agents configured. Run `go-on --init` to set up a provider first.");
        return Ok(());
    }

    // ── Initialize runtime components ──
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
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

    let mut current_agent_name = primary.clone();
    let mut current_agent = registry
        .get(&current_agent_name)
        .ok_or_else(|| anyhow::anyhow!("Agent '{}' not found in registry", current_agent_name))?;

    // ── Print banner ──
    let version = env!("CARGO_PKG_VERSION");
    eprintln!("╔════════════════════════════════════════════════════════════════╗");
    eprintln!("║            go-on terminal chat v{:<46} ║", version);
    eprintln!("╠════════════════════════════════════════════════════════════════╣");
    eprintln!("║  Agent: {:<60} ║", current_agent_name);
    eprintln!("║  Commands: /help /quit /clear /save /load /cost /compact    ║");
    eprintln!("║   /diff /commit /plan /model /context /tools /skills /stats /find_path  ║");
    eprintln!("╚════════════════════════════════════════════════════════════════╝");
    eprintln!();

    let mut messages: Vec<Message> = Vec::new();
    // ── Inject initial system message (built once per session) ──
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

    let mut token_tracker = TokenTracker::default();

    // ── Dedicated stdin channel — spawn once, reuse to avoid per-iteration spawn_blocking ──
    let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<String>();
    tokio::task::spawn_blocking(move || {
        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            line.clear();
            match stdin.read_line(&mut line) {
                Ok(_) => {
                    let trimmed = line.trim().to_string();
                    if stdin_tx.send(trimmed).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // ── Session persistence in .goon/ directory ──
    let session_path = std::path::PathBuf::from(SESSION_FILE);
    if let Some(parent) = session_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // ── Main chat loop with interrupt handling ──
    loop {
        eprint!("🟢 {} > ", current_agent_name);
        std::io::Write::flush(&mut std::io::stdout()).ok();

        // ── Read user input with Ctrl+C handling ──
        let line = tokio::select! {
            line = stdin_rx.recv() => {
                match line {
                    Some(l) => l,
                    None => break,
                }
            }
            _ = signal::ctrl_c() => {
                eprintln!("\n{}Interrupted. Type /quit to exit, or continue typing.{}", ansi!("33"), ansi!("0"));
                continue;
            }
        };

        if line.is_empty() {
            continue;
        }

        // ── Built-in commands ──
        if let Some(cmd) = line.strip_prefix('/') {
            match cmd {
                "quit" | "exit" | "q" => break,
                "help" | "h" => {
                    eprint!("{}", HELP_TEXT);
                    continue;
                }
                "clear" => {
                    messages.clear();
                    eprintln!("{}Conversation cleared.{}", ansi!("32"), ansi!("0"));
                    continue;
                }
                "save" => {
                    let session = ChatSession {
                        messages: messages.clone(),
                        agent_name: current_agent_name.clone(),
                    };
                    let json = serde_json::to_string_pretty(&session);
                    match json {
                        Ok(json) => {
                            if let Err(e) = tokio::fs::write(&session_path, &json).await {
                                eprintln!(
                                    "{}Failed to write session: {}{}",
                                    ansi!("31"),
                                    e,
                                    ansi!("0")
                                );
                            } else {
                                eprintln!(
                                    "{}Session saved to {}{}",
                                    ansi!("32"),
                                    session_path.display(),
                                    ansi!("0")
                                );
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "{}Failed to serialize session: {}{}",
                                ansi!("31"),
                                e,
                                ansi!("0")
                            );
                        }
                    }
                    continue;
                }
                "load" => {
                    match tokio::fs::read_to_string(&session_path).await {
                        Ok(json) => match serde_json::from_str::<ChatSession>(&json) {
                            Ok(session) => {
                                // Validate the saved agent name is still in the registry
                                let agent_valid = registry.get(&session.agent_name).is_some();
                                if !agent_valid {
                                    eprintln!(
                                        "{}Warning: agent '{}' from session no longer available. Using current agent '{}'.{}",
                                        ansi!("33"),
                                        session.agent_name,
                                        current_agent_name,
                                        ansi!("0")
                                    );
                                }
                                messages = session.messages;
                                if agent_valid && session.agent_name != current_agent_name {
                                    if let Some(new_agent) = registry.get(&session.agent_name) {
                                        current_agent = new_agent;
                                        current_agent_name = session.agent_name.clone();
                                    }
                                }
                                eprintln!(
                                    "{}Session loaded: {} messages from '{}'{}",
                                    ansi!("32"),
                                    messages.len(),
                                    current_agent_name,
                                    ansi!("0")
                                );
                            }
                            Err(e) => {
                                eprintln!(
                                    "{}Failed to parse session: {}{}",
                                    ansi!("31"),
                                    e,
                                    ansi!("0")
                                );
                            }
                        },
                        Err(_) => {
                            eprintln!(
                                "{}No saved session found at {}{}",
                                ansi!("33"),
                                session_path.display(),
                                ansi!("0")
                            );
                        }
                    }
                    continue;
                }
                "agents" => {
                    for name in &agent_names {
                        eprintln!("  {name}");
                    }
                    eprintln!(
                        "Current: {}, use /model <name> to switch",
                        current_agent_name
                    );
                    continue;
                }
                "tools" => {
                    let registry = tool_registry();
                    let names = registry.all_names();
                    eprintln!("Available tools ({}):", names.len());
                    for name in names {
                        if let Some(profile) = registry.profile(name) {
                            eprintln!("  {:<25} [{}]", name, profile.capability);
                        } else {
                            eprintln!("  {name}");
                        }
                    }
                    continue;
                }
                "skills" => {
                    // List skills from the global skill registry if available
                    let descriptor_list = crate::orchestration::tool::skill_registry()
                        .and_then(|r| r.read().ok())
                        .map(|guard| guard.list())
                        .unwrap_or_default();
                    if descriptor_list.is_empty() {
                        eprintln!("No skills registered.");
                    } else {
                        eprintln!("Registered skills ({}):", descriptor_list.len());
                        for s in &descriptor_list {
                            eprintln!("  {:<25} score: {:.2}", s.name, s.score);
                        }
                    }
                    continue;
                }
                "stats" => {
                    let agent_msgs = messages.iter().filter(|m| m.role == "assistant").count();
                    let user_msgs = messages.iter().filter(|m| m.role == "user").count();
                    let total_chars: usize = messages.iter().map(|m| m.content.len()).sum();
                    eprintln!("Conversation stats:");
                    eprintln!(
                        "  Messages: {} total ({} user, {} assistant)",
                        messages.len(),
                        user_msgs,
                        agent_msgs
                    );
                    eprintln!("  Total characters: {}", total_chars);
                    eprintln!(
                        "  Avg length: {} chars",
                        if !messages.is_empty() {
                            total_chars / messages.len()
                        } else {
                            0
                        }
                    );
                    eprint!("{}", token_tracker.display());
                    continue;
                }
                "cost" => {
                    eprint!("{}", token_tracker.display());
                    continue;
                }
                "context" => {
                    let total_chars: usize = messages.iter().map(|m| m.content.len()).sum();
                    let est_tokens: usize =
                        messages.iter().map(|m| estimate_tokens(&m.content)).sum();
                    let system_msgs = messages.iter().filter(|m| m.role == "system").count();
                    eprintln!("Context window:");
                    eprintln!(
                        "  Messages: {} ({} system, {} user, {} assistant)",
                        messages.len(),
                        system_msgs,
                        messages.iter().filter(|m| m.role == "user").count(),
                        messages.iter().filter(|m| m.role == "assistant").count()
                    );
                    eprintln!(
                        "  Characters: {} (est. ~{} tokens, CJK-aware)",
                        total_chars, est_tokens
                    );
                    eprintln!(
                        "  Est. context used: {:.1}% of 128K window",
                        (est_tokens as f64 / 128_000.0 * 100.0).min(100.0)
                    );
                    if messages.len() >= COMPACT_PROMPT_THRESHOLD {
                        eprintln!(
                            "  {}Tip: Use /compact to reduce context usage.{}  ({}/{} msgs)",
                            ansi!("33"),
                            ansi!("0"),
                            messages.len(),
                            COMPACT_PROMPT_THRESHOLD
                        );
                    }
                    continue;
                }
                "compact" => {
                    if messages.len() < 4 {
                        eprintln!(
                            "{}Conversation too short to compact.{}",
                            ansi!("33"),
                            ansi!("0")
                        );
                        continue;
                    }
                    // Keep the first message (system context) and last 2 exchanges,
                    // summarize everything in between.
                    let keep_front = 1.min(messages.len());
                    let keep_back = 2.min(messages.len().saturating_sub(keep_front));
                    let compact_range = keep_front..(messages.len() - keep_back);
                    let compact_count = compact_range.len();
                    if compact_count == 0 {
                        eprintln!("{}No messages to compact.{}", ansi!("33"), ansi!("0"));
                        continue;
                    }

                    eprintln!(
                        "{}Summarizing {} messages with LLM...{}",
                        ansi!("33"),
                        compact_count,
                        ansi!("0")
                    );

                    // Collect the messages to summarize
                    let to_compact: Vec<Message> = messages[compact_range.clone()].to_vec();

                    // Build a summarization prompt
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

                    // Build messages for the summarization call
                    let mut summarize_msgs = to_compact;
                    summarize_msgs.push(summarize_prompt);

                    // Call agent to generate summary via streaming
                    let (summary_tx, mut summary_rx) = tokio::sync::mpsc::channel::<String>(2048);
                    let summary_sender = crate::agents::agent::StreamingSender::from(summary_tx);
                    let agent_for_summary = Arc::clone(&current_agent);
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
                        eprintln!("{}Summarization failed: {}{}", ansi!("31"), e, ansi!("0"));
                        continue;
                    }

                    // Drain the compacted range and insert the summary
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
                    continue;
                }
                cmd if cmd == "diff" || cmd.starts_with("diff ") => {
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
                        Err(e) => {
                            eprintln!("{}Git diff failed: {}{}", ansi!("31"), e, ansi!("0"));
                        }
                    }
                    continue;
                }
                "commit" => {
                    eprintln!("Preparing commit...");
                    // Single git diff call for both status and stat context
                    let diff_output = match tokio::process::Command::new("git")
                        .args(["diff", "--stat"])
                        .output()
                        .await
                    {
                        Ok(out) => String::from_utf8_lossy(&out.stdout).to_string(),
                        Err(e) => {
                            eprintln!("{}Git diff failed: {}{}", ansi!("31"), e, ansi!("0"));
                            continue;
                        }
                    };
                    if diff_output.trim().is_empty() {
                        eprintln!("{}Nothing to commit.{}", ansi!("33"), ansi!("0"));
                        continue;
                    }
                    eprintln!("Changes:\n{}", diff_output);

                    // Stage all changes
                    let add_output = tokio::process::Command::new("git")
                        .args(["add", "-A"])
                        .output()
                        .await;
                    match add_output {
                        Ok(out) if !out.status.success() => {
                            eprintln!(
                                "{}Git add failed: {}{}",
                                ansi!("31"),
                                String::from_utf8_lossy(&out.stderr),
                                ansi!("0")
                            );
                            continue;
                        }
                        Err(e) => {
                            eprintln!("{}Git add failed: {}{}", ansi!("31"), e, ansi!("0"));
                            continue;
                        }
                        _ => {}
                    }

                    // Build a meaningful commit message from the diff stats
                    let files_changed: Vec<&str> =
                        diff_output.lines().filter(|l| l.contains('|')).collect();
                    let summary = if files_changed.len() <= 3 {
                        files_changed
                            .iter()
                            .map(|l| l.split('|').next().unwrap_or("").trim())
                            .collect::<Vec<_>>()
                            .join(", ")
                    } else {
                        format!("{} files", files_changed.len())
                    };
                    if summary.is_empty() {
                        eprintln!("{}Nothing to commit.{}", ansi!("33"), ansi!("0"));
                        continue;
                    }
                    let msg = format!("feat: {}", summary);

                    match tokio::process::Command::new("git")
                        .args(["commit", "-m", &msg])
                        .output()
                        .await
                    {
                        Ok(out) if out.status.success() => {
                            let stdout = String::from_utf8_lossy(&out.stdout);
                            eprintln!("{}✓{}{}", ansi!("32"), ansi!("0"), stdout.trim());
                        }
                        Ok(out) => {
                            eprintln!(
                                "{}Commit failed: {}{}",
                                ansi!("31"),
                                String::from_utf8_lossy(&out.stderr),
                                ansi!("0")
                            );
                        }
                        Err(e) => {
                            eprintln!("{}Git commit failed: {}{}", ansi!("31"), e, ansi!("0"));
                        }
                    }
                    continue;
                }
                "plan" => {
                    if messages.is_empty() {
                        eprintln!(
                            "{}No conversation to derive a plan from.{}",
                            ansi!("33"),
                            ansi!("0")
                        );
                        continue;
                    }
                    // Show a summary of the task context and suggest asking the agent
                    let last_user = messages.iter().rev().find(|m| m.role == "user");
                    let assistant_msgs = messages.iter().filter(|m| m.role == "assistant").count();
                    if let Some(msg) = last_user {
                        let preview: String = msg.content.chars().take(200).collect();
                        eprintln!(
                            "{}── Task Context ({} assistant messages) ──{}",
                            ansi!("1"),
                            assistant_msgs,
                            ansi!("0")
                        );
                        eprintln!("Latest request: {}", preview);
                        if msg.content.len() > 200 {
                            eprintln!("... ({} more chars)", msg.content.len() - 200);
                        }
                        eprintln!();
                        eprintln!("To get a structured execution plan, ask the agent:\n  \"Create a step-by-step plan for this task.\"");
                    }
                    continue;
                }
                find_cmd if find_cmd.starts_with("find_path") || find_cmd.starts_with("find ") => {
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
                        None => {
                            eprintln!("Usage: /find_path <glob>  (e.g. /find **/*.rs)");
                        }
                    }
                    continue;
                }
                "review" => {
                    // Single git diff call — extract stat from the same output
                    // instead of running two separate processes.
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
                        continue;
                    }
                    // Extract stat from the full diff: lines matching "file | N ++"
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
                        eprintln!("{}── Changes to review ──{}", ansi!("1"), ansi!("0"));
                        for line in &stat_lines {
                            eprintln!("  {}", line);
                        }
                        eprintln!();
                    }
                    let total_lines = detailed.lines().count();
                    display_diff(&detailed, Some(60));
                    if total_lines > 60 {
                        eprintln!(
                            "{}... ({} more lines){}  Use /diff for full view",
                            ansi!("90"),
                            total_lines - 60,
                            ansi!("0")
                        );
                    }
                    eprintln!(
                        "{}Use /commit to stage and commit these changes.{}",
                        ansi!("33"),
                        ansi!("0")
                    );
                    continue;
                }
                model_cmd if model_cmd.starts_with("model") => {
                    let rest = model_cmd.strip_prefix("model").unwrap_or("");
                    let name = if rest.is_empty() || rest == " " {
                        ""
                    } else {
                        rest.trim()
                    };
                    if name.is_empty() {
                        eprintln!("Available agents: {}", agent_names.join(", "));
                        eprintln!("Current: {}", current_agent_name);
                        eprintln!("Usage: /model <agent_name>");
                    } else if let Some(new_agent) = registry.get(name) {
                        current_agent = new_agent;
                        current_agent_name = name.to_string();
                        eprintln!("{}Switched to agent: {}{}", ansi!("32"), name, ansi!("0"));
                    } else {
                        eprintln!(
                            "{}Agent '{}' not found. Available: {}{}",
                            ansi!("31"),
                            name,
                            agent_names.join(", "),
                            ansi!("0")
                        );
                    }
                    continue;
                }
                _ => {
                    eprintln!("Unknown command: /{cmd}. Type /help.");
                    continue;
                }
            }
        }

        // ── Send user message ──
        messages.push(Message {
            role: "user".to_string(),
            content: line,
        });

        eprint!("{}🤖 {}", ansi!("1"), ansi!("0"));
        std::io::Write::flush(&mut std::io::stdout()).ok();

        // ── Build principles once per turn (tool registry is static, skills rarely change) ──
        let principles = build_cli_principles();

        // ── Run agent with tool execution loop ──
        match run_agent_with_tools(&current_agent, &mut messages, principles).await {
            Ok((resp, prompt_tokens, completion_tokens)) => {
                token_tracker.record_usage(prompt_tokens, completion_tokens);
                if !resp.trim().is_empty() {
                    eprintln!(
                        "{}── Turn complete (est. {} tokens) ──{}",
                        ansi!("90"),
                        prompt_tokens + completion_tokens,
                        ansi!("0")
                    );
                }
            }
            Err(e) => {
                eprintln!("\n{}⚠️  Error: {}{}", ansi!("31"), e, ansi!("0"));
            }
        }

        // ── Auto-save session every turn (non-blocking, debounced, ChatSession format) ──
        if !messages.is_empty() && !SAVE_IN_FLIGHT.load(Ordering::Acquire) {
            SAVE_IN_FLIGHT.store(true, Ordering::Release);
            let session = ChatSession {
                messages: messages.clone(),
                agent_name: current_agent_name.clone(),
            };
            let json = serde_json::to_string(&session).unwrap_or_default();
            let path = session_path.clone();
            let guard = AutoSaveGuard;
            // Fire-and-forget auto-save — the guard notifies waiters on drop
            tokio::spawn(async move {
                if let Err(e) = tokio::fs::write(&path, &json).await {
                    tracing::warn!("Failed to auto-save session: {e}");
                }
                drop(guard);
            });
        }

        // ── Prompt to compact if conversation is long ──
        let msg_count = messages.len();
        if msg_count >= AUTO_COMPACT_THRESHOLD {
            eprintln!(
                "{}⚠️  Conversation is very long ({} msgs). Type /compact to summarize.{}  (Tip: /compact reduces context usage)",
                ansi!("31"),
                msg_count,
                ansi!("0")
            );
        } else if msg_count >= COMPACT_PROMPT_THRESHOLD {
            eprintln!(
                "{}💡 Tip: Use /compact to summarize old messages and free context.{}  ({}/{} msgs)",
                ansi!("33"),
                ansi!("0"),
                msg_count,
                COMPACT_PROMPT_THRESHOLD
            );
        }
    }

    // ── Save session on clean exit (ChatSession format) ──
    // Wait for any in-flight auto-save to complete before writing
    // the final snapshot, preventing concurrent-write data loss.
    if !messages.is_empty() {
        // Wait for in-flight save to complete with bounded timeout
        if SAVE_IN_FLIGHT.load(Ordering::Acquire) {
            tokio::select! {
                _ = save_notify().notified() => { /* save completed */ }
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => { /* timeout */ }
            }
        }
        let session = ChatSession {
            messages: messages.clone(),
            agent_name: current_agent_name.clone(),
        };
        let json = serde_json::to_string(&session).unwrap_or_default();
        if let Err(e) = tokio::fs::write(&session_path, &json).await {
            tracing::warn!("Failed to save session on exit: {e}");
        } else {
            eprintln!("Session auto-saved");
        }
    }

    eprintln!("Goodbye!");
    Ok(())
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
    principles: Option<Vec<String>>,
) -> Result<(String, usize, usize)> {
    let principles = principles.unwrap_or_default();
    // ── Estimate prompt tokens from existing messages using CJK-aware estimator ──
    let estimated_prompt_tokens: usize = messages.iter().map(|m| estimate_tokens(&m.content)).sum();
    let (tx, mut rx) = mpsc::channel::<String>(2048);
    let sender = StreamingSender::from(tx);
    let msgs = messages.clone();
    let initial_principles = if principles.is_empty() {
        None
    } else {
        Some(principles.clone())
    };

    // ── Cancellation support for Ctrl+C ──
    // The JoinHandle::abort() cancels the spawned task when the user presses Ctrl+C.
    // This prevents zombie agent tasks from accumulating in the background.
    let agent_ref = Arc::clone(agent);
    let chat_task =
        tokio::spawn(async move { agent_ref.chat(msgs, initial_principles, None, sender).await });

    let mut renderer = StreamMarkdownRenderer::new();
    let mut tool_calls: Vec<(String, String)> = Vec::new();
    let mut in_reasoning = false;

    // ── Streaming output with interrupt support ──
    loop {
        tokio::select! {
            token = rx.recv() => {
                match token {
                    Some(token) => {
                        // Tool call detection (agents emit __tool_call__:tool_name:args)
                        if let Some((tool_name, tool_args)) = parse_tool_call_token(&token) {
                            tool_calls.push((tool_name.to_string(), tool_args.to_string()));
                            eprintln!("{}🔧 [Tool call: {tool_name}]{}", ansi!("33"), ansi!("0"));
                            continue;
                        }

                        // Reasoning content markers
                        if token == REASONING_START {
                            in_reasoning = true;
                            eprint!("{}", ansi!("90"));
                            continue;
                        }
                        if token == REASONING_END {
                            in_reasoning = false;
                            eprint!("{}", ansi!("0"));
                            eprintln!();
                            continue;
                        }

                        if in_reasoning {
                            eprint!("{}", token);
                        } else {
                            // Feed to streaming renderer for ANSI formatting first,
                            // then display only the formatted output — avoids the raw-text
                            // flash followed by ANSI-replace flicker that occurs when
                            // printing raw first then cursor-up+erase.
                            renderer.feed(&token);
                            let (formatted, _) = renderer.flush();
                            if !formatted.is_empty() {
                                eprint!("{}", formatted);
                            } else {
                                // No complete lines yet — buffer via renderer, do NOT
                                // print raw to avoid flicker.
                            }
                        }
                        std::io::Write::flush(&mut std::io::stdout()).ok();
                    }
                    None => break,
                }
            }
            _ = signal::ctrl_c() => {
                eprintln!(
                    "\n{}Interrupted agent response. Use /clear to reset.{}",
                    ansi!("33"), ansi!("0")
                );
                // Abort the agent task to prevent zombie background tasks
                chat_task.abort();
                break;
            }
        }
    }

    // Await the task — if it was aborted, JoinError is expected
    match chat_task.await {
        Ok(Ok(())) => {} // completed normally
        Ok(Err(e)) => warn!("Agent chat failed: {e}"),
        Err(e) => {
            if e.is_cancelled() {
                debug!("Agent chat cancelled by user");
            } else {
                warn!("Agent chat task panicked: {e}");
            }
        }
    }

    // ── Flush remaining renderer output ──
    let mut response: String = {
        let (remaining, _) = renderer.flush();
        if !remaining.is_empty() {
            let n = remaining.lines().count();
            for _ in 0..n {
                eprint!("\x1B[F\x1B[K");
            }
            eprintln!("{}", remaining);
            remaining
        } else {
            renderer.take_raw_response()
        }
    };

    // ── Phase 2: Execute tools with progressive streaming ──
    let mut followup_round_executed = false;
    if !tool_calls.is_empty() {
        eprintln!("{}── Tool execution ──{}", ansi!("33"), ansi!("0"));

        // Use FuturesUnordered for progressive tool result display — results
        // appear as each tool completes rather than waiting for ALL tools to
        // finish. This matches ZED chat's progressive result streaming pattern
        // and provides immediate feedback for fast tools.
        //
        // A Semaphore limits concurrent tool executions to prevent resource
        // exhaustion when the agent emits many parallel tool calls (e.g., 50+).
        let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_TOOLS));
        let mut tool_futures = FuturesUnordered::new();
        for (tool_name, tool_args_str) in &tool_calls {
            let tool_name = tool_name.clone();
            let tool_args_str = tool_args_str.clone();
            let sem_clone = Arc::clone(&semaphore);
            tool_futures.push(async move {
                // Acquire a permit — this limits concurrent exec to MAX_CONCURRENT_TOOLS.
                // I/O-bound tools (read_file, search_files, http_request) benefit from
                // parallelism while CPU-bound or resource-heavy tools are throttled.
                let _permit = sem_clone.acquire().await.ok();
                let parsed_args: Value = serde_json::from_str(&tool_args_str).unwrap_or(json!({}));
                let start = std::time::Instant::now();
                eprintln!("  {}⚡ {}{}...", ansi!("36"), tool_name, ansi!("0"));
                let result = execute_simple_tool(&tool_name, &parsed_args).await;
                let elapsed = start.elapsed();
                (tool_name, tool_args_str, result, elapsed)
            });
        }

        let mut tool_results: Vec<String> = Vec::new();
        let mut has_failure = false;

        // Process results as they arrive (progressive streaming)
        while let Some((tool_name, _, result, elapsed)) = tool_futures.next().await {
            match result {
                Ok(result_text) => {
                    let display = if result_text.len() > 500 {
                        let end = result_text
                            .char_indices()
                            .nth(500)
                            .map(|(i, _)| i)
                            .unwrap_or(result_text.len());
                        format!(
                            "{}...\n[{} chars truncated]  ({:.1}s)",
                            &result_text[..end],
                            result_text.len(),
                            elapsed.as_secs_f32()
                        )
                    } else {
                        format!("{}  ({:.1}s)", result_text, elapsed.as_secs_f32())
                    };
                    eprintln!("    {}✓{} {}", ansi!("32"), ansi!("0"), display);
                    let result_for_llm = if result_text.len() > MAX_TOOL_RESULT_CHARS {
                        format!(
                            "{}...\n[truncated: {} total chars, showing first {}]",
                            &result_text[..MAX_TOOL_RESULT_CHARS],
                            result_text.len(),
                            MAX_TOOL_RESULT_CHARS
                        )
                    } else {
                        result_text.clone()
                    };
                    tool_results.push(build_tool_result_block(&tool_name, &result_for_llm, false));
                }
                Err(e) => {
                    has_failure = true;
                    eprintln!(
                        "    {}✗ Error: {}{}  ({:.1}s)",
                        ansi!("31"),
                        e,
                        ansi!("0"),
                        elapsed.as_secs_f32()
                    );
                    tool_results.push(build_tool_result_block(&tool_name, &e.to_string(), true));
                }
            }
        }

        // ── Phase 3: Send tool results back to agent for follow-up ──
        if !tool_results.is_empty() {
            followup_round_executed = true;
            messages.push(Message {
                role: "assistant".to_string(),
                content: response.clone(),
            });
            messages.push(Message {
                role: "user".to_string(),
                content: build_tool_execution_followup_message(&tool_results, has_failure),
            });

            eprint!("{}── Agent follow-up ──{}\n🤖 ", ansi!("33"), ansi!("0"));
            std::io::Write::flush(&mut std::io::stdout()).ok();

            // Streaming follow-up
            let (tx2, mut rx2) = mpsc::channel::<String>(2048);
            let sender2 = StreamingSender::from(tx2);
            let msgs2 = messages.clone();

            let agent_ref2 = Arc::clone(agent);
            let followup_principles = if principles.is_empty() {
                None
            } else {
                Some(principles.clone())
            };
            let followup_task = tokio::spawn(async move {
                agent_ref2
                    .chat(msgs2, followup_principles, None, sender2)
                    .await
            });

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
                                if in_reasoning2 {
                                    eprint!("{}", token);
                                } else {
                                    // Feed to renderer first, display only formatted output
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

            // ── Flush remaining follow-up renderer output ──
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

            if !rendered_final.trim().is_empty() {
                crate::acp::helpers::autonomy_metrics::record_tool_followup_success();
                response = rendered_final;
            } else {
                crate::acp::helpers::autonomy_metrics::record_tool_followup_fallback();
            }
        }
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
    let canonical_name = match name {
        "read" => "read_file",
        "write" | "create" => "write_file",
        "search" | "grep" => "search_files",
        "ls" => "list_directory",
        "bash" | "execute_command" | "run" => "shell_exec",
        other => other,
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
        "run_tests" => {
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

        // ── Git diff: diff content ──
        "inspect_git_diff" => {
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

        // ── Cargo check: structured errors/warnings ──
        "cargo_check" => {
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

        // ── Generic fallback: pretty-printed JSON ──
        _ => Ok(serde_json::to_string_pretty(r).unwrap_or_default()),
    }
}

/// Cached build of CLI principles — rebuilt only when skills change.
/// Uses a generation counter so that principles are re-built only when
/// the skill registry content changes (detected via total skill count).
fn build_cli_principles() -> Option<Vec<String>> {
    static CACHED: std::sync::OnceLock<std::sync::Mutex<(Vec<String>, usize)>> =
        std::sync::OnceLock::new();
    let cache = CACHED.get_or_init(|| std::sync::Mutex::new((Vec::new(), usize::MAX)));

    // Detect skill registry change by current skill count
    let current_skill_count = crate::orchestration::tool::skill_registry()
        .and_then(|r| r.read().ok())
        .map(|g| g.list().len())
        .unwrap_or(0);

    if let Ok(mut guard) = cache.lock() {
        if guard.1 == current_skill_count && !guard.0.is_empty() {
            return Some(guard.0.clone());
        }

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
                let skill_list = guard2.list();
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

        guard.0 = principles.clone();
        guard.1 = current_skill_count;
        Some(principles)
    } else {
        // Fallback: rebuild uncached
        Some(vec![
            "You are a helpful AI coding assistant with access to tools.".to_string(),
            "You can use __tool_call__:tool_name:json_args to invoke tools.".to_string(),
        ])
    }
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
            err.contains("missing") || err.contains("required") || err.contains("missing_path"),
            "error should mention missing/required field, got: {err}"
        );

        let err = execute_simple_tool("write_file", &json!({"path": "test.txt"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("missing") || err.contains("required") || err.contains("missing_content"),
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
