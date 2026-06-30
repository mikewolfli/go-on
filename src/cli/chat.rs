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

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use anyhow::Result;
use serde_json::{json, Value};
use tokio::signal;
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};
use tracing::{debug, warn};

use crate::acp::helpers::autonomy::terminal_chat_contract_snapshot;
use crate::agents::agent::{Agent, AgentRegistry, Message, StreamingSender};
use crate::config::AppConfig;

use crate::governance::status::quick_check_tool as governance_gate;
use crate::intelligence::capability_graph::CapabilityGraph;
use crate::orchestration::autonomy_runtime::{
    build_tool_execution_followup_message, build_tool_result_block, parse_tool_call_token,
};
use crate::orchestration::tool::{ToolInput, ToolRegistry};

/// Maximum number of characters from a tool result sent to the LLM.
const MAX_TOOL_RESULT_CHARS: usize = 100_000;
/// Session file name for conversation persistence.
const SESSION_FILE: &str = ".goon-chat-session.json";

/// Threshold at which we prompt the user to compact the conversation.
const COMPACT_PROMPT_THRESHOLD: usize = 30;

/// Max lines to display for help text.
const HELP_TEXT: &str = "\
Commands:
  /quit        Exit chat
  /clear       Clear conversation history
  /save        Save session to file
  /load        Load session from file
  /help        Show this help
  /agents      List configured agents
  /tools       List available tools
  /skills      List available skills
  /stats       Show conversation stats
  /compact     Summarize & compact conversation history
  /cost        Show token usage and estimated cost
  /diff        Show git diff (optional path filter)
  /commit      Git commit with AI-generated message
  /review      Review current git diff before committing
  /plan        Show structured execution plan

The AI agent has access to tools:
  - Read/write files
  - Search files and directories
  - Execute shell commands
  - Create and invoke skills
  - Git operations (diff, status, log, commit)
  - Multi-turn conversation with context
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
    version: u32,
}

/// Track cumulative token usage and cost across the session.
#[derive(Default, Clone, Serialize, Deserialize)]
struct TokenTracker {
    total_prompt_tokens: u64,
    total_completion_tokens: u64,
    total_cost_usd: f64,
}

impl TokenTracker {
    fn record_usage(&mut self, prompt_tokens: u64, completion_tokens: u64) {
        self.total_prompt_tokens += prompt_tokens;
        self.total_completion_tokens += completion_tokens;
        // Rough estimate: $0.15/M input tokens, $0.60/M output tokens (GPT-4o pricing)
        self.total_cost_usd += (prompt_tokens as f64 * 0.15 / 1_000_000.0)
            + (completion_tokens as f64 * 0.60 / 1_000_000.0);
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

    let agent_names: Vec<String> = config.agents().keys().cloned().collect();
    let primary = agent_names[0].clone();

    let agent = registry
        .get(&primary)
        .ok_or_else(|| anyhow::anyhow!("Agent '{primary}' not found in registry"))?;

    // ── Print banner ──
    let version = env!("CARGO_PKG_VERSION");
    eprintln!("╔════════════════════════════════════════════════════════════════╗");
    eprintln!("║            go-on terminal chat v{:<46} ║", version);
    eprintln!("╠════════════════════════════════════════════════════════════════╣");
    eprintln!("║  Agent: {:<60} ║", primary);
    eprintln!("║  Commands: /help /quit /clear /save /load /cost /compact    ║");
    eprintln!("║            /diff /commit /plan /tools /skills /stats         ║");
    eprintln!("╚════════════════════════════════════════════════════════════════╝");
    eprintln!();

    let mut messages: Vec<Message> = Vec::new();
    let mut input = String::new();
    let mut token_tracker = TokenTracker::default();

    // ── Session persistence in current directory ──
    let session_path = std::path::PathBuf::from(SESSION_FILE);

    // ── Main chat loop with interrupt handling ──
    loop {
        input.clear();
        eprint!("🟢 {} > ", primary);
        std::io::Write::flush(&mut std::io::stdout()).ok();

        // ── Read user input with Ctrl+C handling ──
        let read_result = {
            let stdin = std::io::stdin();
            let mut line = String::new();
            tokio::select! {
                result = async { stdin.read_line(&mut line).map(|_| line.clone()) } => {
                    Some(result)
                }
                _ = signal::ctrl_c() => {
                    eprintln!("\n{}Interrupted. Type /quit to exit, or continue typing.{}", ansi!("33"), ansi!("0"));
                    None
                }
            }
        };

        let line = match read_result {
            Some(Ok(l)) => l.trim().to_string(),
            Some(Err(e)) => {
                eprintln!("Read error: {e}");
                break;
            }
            None => continue,
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
                        agent_name: primary.clone(),
                        version: 1,
                    };
                    let json = serde_json::to_string_pretty(&session)?;
                    std::fs::write(&session_path, &json)?;
                    eprintln!(
                        "{}Session saved to {}{}",
                        ansi!("32"),
                        session_path.display(),
                        ansi!("0")
                    );
                    continue;
                }
                "load" => {
                    match std::fs::read_to_string(&session_path) {
                        Ok(json) => match serde_json::from_str::<ChatSession>(&json) {
                            Ok(session) => {
                                messages = session.messages;
                                eprintln!(
                                    "{}Session loaded: {} messages from '{}'{}",
                                    ansi!("32"),
                                    messages.len(),
                                    session.agent_name,
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
                    continue;
                }
                "tools" => {
                    let registry = ToolRegistry::default();
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
                    let keep_back = 4.min(messages.len().saturating_sub(keep_front));
                    let compact_range = keep_front..(messages.len() - keep_back);
                    let compact_count = compact_range.len();
                    if compact_count == 0 {
                        eprintln!("{}No messages to compact.{}", ansi!("33"), ansi!("0"));
                        continue;
                    }
                    let summary = format!(
                        "[Conversation compacted: {} earlier messages summarized. Continuing conversation.]",
                        compact_count
                    );
                    messages.drain(compact_range);
                    messages.insert(
                        1,
                        Message {
                            role: "system".to_string(),
                            content: summary,
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
                "diff" => {
                    let path_filter = cmd
                        .strip_prefix("diff ")
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty());
                    let mut git_cmd = std::process::Command::new("git");
                    git_cmd.arg("diff");
                    if let Some(filter) = path_filter {
                        git_cmd.arg("--").arg(filter);
                    }
                    match git_cmd.output() {
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
                                // Show diff with color highlighting
                                for line in diff.lines() {
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
                        }
                        Err(e) => {
                            eprintln!("{}Git diff failed: {}{}", ansi!("31"), e, ansi!("0"));
                        }
                    }
                    continue;
                }
                "commit" => {
                    eprintln!("Generating commit message...");
                    // Check git status to determine what to commit
                    match std::process::Command::new("git")
                        .args(["status", "--short"])
                        .output()
                    {
                        Ok(out) => {
                            let s = String::from_utf8_lossy(&out.stdout);
                            if s.trim().is_empty() {
                                eprintln!("{}Nothing to commit.{}", ansi!("33"), ansi!("0"));
                                continue;
                            }
                            eprintln!("Changes to commit:\n{}", s);
                        }
                        Err(e) => {
                            eprintln!(
                                "{}Git status check failed: {}{}",
                                ansi!("31"),
                                e,
                                ansi!("0")
                            );
                            continue;
                        }
                    }
                    // Stage all and commit with a generic message
                    match std::process::Command::new("git")
                        .args(["add", "-A"])
                        .output()
                    {
                        Ok(out) if !out.status.success() => {
                            let stderr = String::from_utf8_lossy(&out.stderr);
                            eprintln!("{}Git add failed: {}{}", ansi!("31"), stderr, ansi!("0"));
                            continue;
                        }
                        Err(e) => {
                            eprintln!("{}Git add failed: {}{}", ansi!("31"), e, ansi!("0"));
                            continue;
                        }
                        _ => {}
                    }
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let msg = format!("feat: go-on chat auto-commit {}", now);
                    match std::process::Command::new("git")
                        .args(["commit", "-m", &msg])
                        .output()
                    {
                        Ok(out) if out.status.success() => {
                            let stdout = String::from_utf8_lossy(&out.stdout);
                            eprintln!("{}✓{}{}", ansi!("32"), ansi!("0"), stdout.trim());
                        }
                        Ok(out) => {
                            let stderr = String::from_utf8_lossy(&out.stderr);
                            eprintln!("{}Commit failed: {}{}", ansi!("31"), stderr, ansi!("0"));
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
                            "{}No conversation to derive plan from.{}",
                            ansi!("33"),
                            ansi!("0")
                        );
                        continue;
                    }
                    // Simple keyword-based step extraction from the last user message
                    let last_user = messages.iter().rev().find(|m| m.role == "user");
                    if let Some(user_msg) = last_user {
                        let lines: Vec<&str> = user_msg
                            .content
                            .lines()
                            .filter(|l| {
                                let t = l.trim();
                                t.starts_with("- [")
                                    || t.starts_with("*")
                                    || t.starts_with("1.")
                                    || t.starts_with("- ")
                            })
                            .collect();
                        if lines.is_empty() {
                            eprintln!(
                                "{}No structured plan found. Ask the agent to create a plan.{}",
                                ansi!("33"),
                                ansi!("0")
                            );
                        } else {
                            eprintln!("{}── Extracted Plan ──{}", ansi!("1"), ansi!("0"));
                            for (i, line) in lines.iter().enumerate() {
                                eprintln!("  {}. {}", i + 1, line.trim());
                            }
                        }
                    }
                    continue;
                }
                "review" => {
                    let diff = match std::process::Command::new("git")
                        .args(["diff", "--stat"])
                        .output()
                    {
                        Ok(out) => String::from_utf8_lossy(&out.stdout).to_string(),
                        Err(_) => String::new(),
                    };
                    if diff.trim().is_empty() {
                        eprintln!("{}No changes to review.{}", ansi!("33"), ansi!("0"));
                        continue;
                    }
                    eprintln!("{}── Changes to review ──{}", ansi!("1"), ansi!("0"));
                    eprintln!("{}", diff);
                    let detailed = match std::process::Command::new("git")
                        .args(["diff"])
                        .output()
                    {
                        Ok(out) => String::from_utf8_lossy(&out.stdout).to_string(),
                        Err(_) => String::new(),
                    };
                    for line in detailed.lines().take(60) {
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
                    if detailed.lines().count() > 60 {
                        eprintln!("{}... ({} more lines){}  Use /diff for full view", ansi!("90"), detailed.lines().count() - 60, ansi!("0"));
                    }
                    eprintln!("{}Use /commit to stage and commit these changes.{}", ansi!("33"), ansi!("0"));
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

        // ── Run agent with tool execution loop ──
        match run_agent_with_tools(&agent, &mut messages).await {
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

        // ── Auto-save session every turn ──
        if !messages.is_empty() {
            let session = ChatSession {
                messages: messages.clone(),
                agent_name: primary.clone(),
                version: 1,
            };
            if let Ok(json) = serde_json::to_string(&session) {
                let _ = std::fs::write(&session_path, &json);
            }
        }

        // ── Prompt to compact if conversation is long ──
        if messages.len() >= COMPACT_PROMPT_THRESHOLD {
            eprintln!(
                "{}💡 Tip: Use /compact to summarize old messages and free context.{}  ({}/{} msgs)",
                ansi!("33"),
                ansi!("0"),
                messages.len(),
                COMPACT_PROMPT_THRESHOLD
            );
        }
    }

    // ── Save session on clean exit ──
    if !messages.is_empty() {
        let session = ChatSession {
            messages,
            agent_name: primary.clone(),
            version: 1,
        };
        if let Ok(json) = serde_json::to_string(&session) {
            let _ = std::fs::write(&session_path, &json);
            eprintln!("Session auto-saved to {}", session_path.display());
        }
    }

    eprintln!("Goodbye!");
    Ok(())
}

/// Run a single agent turn: agent chat → tool execution → followup.
/// Returns the response text and estimated token usage.
async fn run_agent_with_tools(
    agent: &Arc<dyn Agent>,
    messages: &mut Vec<Message>,
) -> Result<(String, u64, u64)> {
    // ── Estimate prompt tokens from existing messages (rough: 4 chars ≈ 1 token) ──
    let prompt_chars: usize = messages.iter().map(|m| m.content.len()).sum();
    let estimated_prompt_tokens = (prompt_chars / 4) as u64;
    let (tx, mut rx) = mpsc::channel::<String>(2048);
    let sender = StreamingSender::from(tx);
    let msgs = messages.clone();
    let options: Option<HashMap<String, Value>> = None;

    let agent_ref = Arc::clone(agent);
    let chat_task = tokio::spawn(async move { agent_ref.chat(msgs, None, options, sender).await });

    let mut response = String::new();
    let mut tool_calls: Vec<(String, String)> = Vec::new();
    let mut in_reasoning = false;

    // ── Streaming output with interrupt support ──
    let mut output_line = String::new();
    loop {
        tokio::select! {
            token = rx.recv() => {
                match token {
                    Some(token) => {
                        // Tool call detection (agents emit __tool_call__:tool_name:args)
                        if let Some((tool_name, tool_args)) = parse_tool_call_token(&token) {
                            tool_calls.push((tool_name.to_string(), tool_args.to_string()));
                            if !output_line.is_empty() {
                                eprintln!("{}", output_line);
                                output_line.clear();
                            }
                            eprintln!("{}🔧 [Tool call: {tool_name}]{}", ansi!("33"), ansi!("0"));
                            continue;
                        }

                        // Reasoning content markers
                        if token == "\u{001E}" {
                            in_reasoning = true;
                            if !output_line.is_empty() {
                                eprintln!("{}", output_line);
                                output_line.clear();
                            }
                            eprint!("{}", ansi!("90"));
                            continue;
                        }
                        if token == "\u{001F}" {
                            in_reasoning = false;
                            eprint!("{}", ansi!("0"));
                            eprintln!();
                            continue;
                        }

                        if in_reasoning {
                            eprint!("{}", token);
                        } else {
                            response.push_str(&token);
                            output_line.push_str(&token);
                            print!("{}", token);
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
                // We can't cancel the chat task from here, but we break out
                // of the streaming loop. The agent will complete in background.
                break;
            }
        }
    }

    if !output_line.is_empty() {
        eprintln!();
    }

    if let Err(e) = chat_task.await {
        warn!("Agent chat task failed: {e}");
    }

    // ── Markdown re-render of full response ──
    if !response.is_empty() {
        let rendered = crate::cli::markdown_renderer::render_markdown(&response);
        if rendered.contains("\u{001B}") {
            eprint!("\r");
            let line_count = response.lines().count().max(1);
            for _ in 0..line_count {
                // ANSI cursor up + clear line
                eprint!("\x1B[F\x1B[K");
            }
            eprintln!("{}", rendered);
            response = rendered;
        }
    }

    // ── Phase 2: Execute any tool calls ──
    let mut followup_round_executed = false;
    if !tool_calls.is_empty() {
        eprintln!("{}── Tool execution ──{}", ansi!("33"), ansi!("0"));
        let mut tool_results: Vec<String> = Vec::new();
        let mut has_failure = false;

        for (tool_name, tool_args_str) in &tool_calls {
            eprintln!("  {}⚡ {}{}...", ansi!("36"), tool_name, ansi!("0"));
            let parsed_args: Value = serde_json::from_str(tool_args_str).unwrap_or(json!({}));

            match execute_simple_tool(tool_name, &parsed_args).await {
                Ok(result_text) => {
                    let display = if result_text.len() > 500 {
                        let end = result_text
                            .char_indices()
                            .nth(500)
                            .map(|(i, _)| i)
                            .unwrap_or(result_text.len());
                        format!(
                            "{}...\n[{} chars truncated]",
                            &result_text[..end],
                            result_text.len()
                        )
                    } else {
                        result_text.clone()
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
                    tool_results.push(build_tool_result_block(tool_name, &result_for_llm, false));
                }
                Err(e) => {
                    has_failure = true;
                    eprintln!("    {}✗ Error: {}{}", ansi!("31"), e, ansi!("0"));
                    // Error recovery: suggest possible fixes
                    let err_msg = e.to_string().to_lowercase();
                    if err_msg.contains("not found") || err_msg.contains("no such") {
                        eprintln!("    {}💡 Tip: Check the file path. Use /tools to see available tools.{}  [P3]", ansi!("33"), ansi!("0"));
                    } else if err_msg.contains("permission") || err_msg.contains("denied") {
                        eprintln!("    {}💡 Tip: Permission issue. Try using sudo or check file ownership.{}  [P3]", ansi!("33"), ansi!("0"));
                    } else if err_msg.contains("timeout") || err_msg.contains("timed out") {
                        eprintln!("    {}💡 Tip: Operation timed out. Try splitting into smaller steps or use --verbose.{}  [P3]", ansi!("33"), ansi!("0"));
                    } else if err_msg.contains("syntax") || err_msg.contains("parse error") {
                        eprintln!("    {}💡 Tip: Syntax error detected. Check the command syntax or file format.{}  [P3]", ansi!("33"), ansi!("0"));
                    } else if err_msg.contains("not a git") || err_msg.contains("git") {
                        eprintln!("    {}💡 Tip: Git operation failed. Check that you're in a git repository.{}  [P3]", ansi!("33"), ansi!("0"));
                    } else if err_msg.contains("network") || err_msg.contains("connection") {
                        eprintln!("    {}💡 Tip: Network issue detected. Check your internet connection.{}  [P3]", ansi!("33"), ansi!("0"));
                    } else {
                        eprintln!("    {}💡 Tip: Try running the command separately or check arguments with /help.{}  [P3]", ansi!("33"), ansi!("0"));
                    }
                    tool_results.push(build_tool_result_block(tool_name, &e.to_string(), true));
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
            let followup_task =
                tokio::spawn(async move { agent_ref2.chat(msgs2, None, None, sender2).await });

            let mut followup_response = String::new();
            let mut in_reasoning2 = false;
            while let Some(token) = rx2.recv().await {
                if token == "\u{001E}" {
                    in_reasoning2 = true;
                    eprint!("{}", ansi!("90"));
                    continue;
                }
                if token == "\u{001F}" {
                    in_reasoning2 = false;
                    eprint!("{}", ansi!("0"));
                    eprintln!();
                    continue;
                }
                if in_reasoning2 {
                    eprint!("{}", token);
                } else {
                    followup_response.push_str(&token);
                    print!("{}", token);
                }
                std::io::Write::flush(&mut std::io::stdout()).ok();
            }
            eprintln!();

            if let Err(e) = followup_task.await {
                warn!("Agent followup task failed: {e}");
            }

            if !followup_response.trim().is_empty() {
                crate::acp::helpers::autonomy_metrics::record_tool_followup_success();
                response = followup_response;
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

    let estimated_completion_tokens = (response.len() / 4) as u64;
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
    // ── Governance gate: validate tool + arguments before execution ──
    if let Err(reason) = governance_gate(name, args) {
        return Err(anyhow::anyhow!("governance denied: {reason}"));
    }

    // ── Map aliases to canonical ToolRegistry names ──
    let canonical_name = match name {
        "read" => "read_file",
        "write" | "create" => "write_file",
        "search" | "grep" => "search_files",
        "ls" => "list_directory",
        "bash" | "execute_command" | "run" => "shell_exec",
        other => other,
    };

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

    // ── Build ToolInput envelope and execute via ToolRegistry ──
    let input = ToolInput {
        task_id: "execute_simple_tool".to_string(),
        phase: "act".to_string(),
        agent_role: "coder".to_string(),
        objective: format!("Execute tool: {canonical_name}"),
        constraints: None,
        evidence: None,
        payload,
        allowed_base_dir: None,
    };

    let canonical_owned = canonical_name.to_string();

    // Use the profile's timeout budget when available, otherwise default to
    // 300 seconds (matching the most generous old bash timeout).
    let registry = ToolRegistry::default();
    let timeout_ms = registry
        .profile(&canonical_owned)
        .map(|p| p.timeout_budget_ms)
        .unwrap_or(300_000);
    let timeout_dur = Duration::from_millis(timeout_ms.max(5_000));

    // Run the blocking tool logic on a dedicated blocking thread so we don't
    // starve the async runtime.  The ToolRegistry is created inside the closure
    // so everything is owned and Send.
    let output = timeout(timeout_dur, async {
        tokio::task::spawn_blocking(move || {
            let registry = ToolRegistry::default();
            registry.run_with_fallback(&canonical_owned, &input)
        })
        .await
        .map_err(|e| anyhow::anyhow!("tool blocking task failed: {e}"))?
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

    // ── Format ToolOutput back into the simple string every caller expects ──
    match canonical_name {
        "read_file" => Ok(output
            .result
            .as_ref()
            .and_then(|r| r["content"].as_str())
            .unwrap_or("")
            .to_string()),
        "write_file" => {
            let path = output
                .result
                .as_ref()
                .and_then(|r| r["path"].as_str())
                .unwrap_or("unknown");
            Ok(format!("wrote file: {path}"))
        }
        "search_files" => {
            let files: Vec<String> = output
                .result
                .as_ref()
                .and_then(|r| r["files"].as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            if files.is_empty() {
                Ok("No files matching pattern".to_string())
            } else {
                Ok(files.join("\n"))
            }
        }
        "list_directory" => {
            let mut entries: Vec<String> = output
                .result
                .as_ref()
                .and_then(|r| r["entries"].as_array())
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
        "shell_exec" => {
            let r = output.result.as_ref();
            let stdout = r.and_then(|r| r["stdout"].as_str()).unwrap_or("");
            let stderr = r.and_then(|r| r["stderr"].as_str()).unwrap_or("");
            let exit_code = r.and_then(|r| r["exit_code"].as_i64());
            let mut buf = String::new();
            if !stdout.is_empty() {
                buf.push_str(stdout);
            }
            if !stderr.is_empty() {
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(stderr);
            }
            if let Some(code) = exit_code {
                buf.push_str(&format!("\nexit code: {code}"));
            }
            Ok(buf)
        }
        // Generic fallback for any other tool registered in the future
        _ => Ok(output
            .result
            .map(|r| serde_json::to_string_pretty(&r).unwrap_or_default())
            .unwrap_or_default()),
    }
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
            "read_file with traversal path should be rejected"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("canonicalization failed"),
            "error should mention canonicalization failure, got: {}",
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
            err.contains("missing_path"),
            "error should mention missing_path, got: {err}"
        );

        let err = execute_simple_tool("write_file", &json!({"path": "test.txt"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("missing_content"),
            "error should mention missing_content, got: {err}"
        );
    }

    /// Verify that unknown tool names produce a descriptive error.
    #[tokio::test]
    async fn test_execute_simple_tool_unknown_tool() {
        let result = execute_simple_tool("nonexistent_tool", &json!({})).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not registered in governance gate"),
            "error should indicate unknown tool was rejected by governance"
        );
    }
}
