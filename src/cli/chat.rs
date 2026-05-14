//! Terminal chat mode — full-featured interactive AI chat in the terminal
//! (like Claude Code, Codex, OpenClaw).
//!
//! Features:
//! - Streaming agent chat with thinking/reasoning display
//! - Tool execution (file read/write, search, code execution, etc.)
//! - Skill invocation
//! - Multi-turn conversation
//! - Built-in commands
//!
//! Usage: `go-on -a` or `go-on --chat`

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::time::Duration;
use tracing::warn;

use crate::agents::agent::{Agent, AgentRegistry, Message, StreamingSender};
use crate::config::AppConfig;
use crate::flow::FlowManager;
use crate::intelligence::capability_graph::CapabilityGraph;

/// Maximum file size we'll read in a single tool call (10 MB).
const MAX_FILE_READ_BYTES: u64 = 10 * 1024 * 1024;

/// Maximum number of characters from a tool result sent to the LLM.
const MAX_TOOL_RESULT_CHARS: usize = 100_000;

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

/// Resolve a path relative to the current working directory.
/// Rejects paths that escape the workspace via ".." or absolute paths outside cwd.
fn resolve_safe_path(path_str: &str, allow_new_file: bool) -> Result<std::path::PathBuf> {
    let cwd = std::env::current_dir().context("failed to get current directory")?;
    let cwd = cwd.canonicalize().context("failed to canonicalize cwd")?;
    let target = std::path::Path::new(path_str);
    let target = if target.is_relative() {
        cwd.join(target)
    } else {
        target.to_path_buf()
    };
    // For new files, canonicalize the parent to check it's within cwd
    if allow_new_file && !target.exists() {
        if let Some(parent) = target.parent() {
            let canon_parent = parent.canonicalize().with_context(|| {
                format!("parent directory does not exist: {}", parent.display())
            })?;
            if !canon_parent.starts_with(&cwd) {
                anyhow::bail!("path '{}' is outside the workspace directory", path_str);
            }
        }
        return Ok(target);
    }
    // For existing files, canonicalize the full path
    let target = target
        .canonicalize()
        .with_context(|| format!("path does not exist: {path_str}"))?;
    if !target.starts_with(&cwd) {
        anyhow::bail!("path '{}' is outside the workspace directory", path_str);
    }
    Ok(target)
}

/// Run an interactive terminal chat session with full agent capabilities.
pub async fn run_terminal_chat(config: Arc<AppConfig>) -> Result<()> {
    if config.agents.is_empty() {
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

    let _flow = Arc::new(FlowManager::new(Arc::clone(&config), None));

    let agent_names: Vec<String> = config.agents.keys().cloned().collect();
    let primary = agent_names[0].clone();

    let agent = registry
        .get(&primary)
        .ok_or_else(|| anyhow::anyhow!("Agent '{primary}' not found in registry"))?;

    // ── Print banner ──
    eprintln!("╔══════════════════════════════════════════════════════════╗");
    eprintln!("║            go-on terminal chat mode                     ║");
    eprintln!("╠══════════════════════════════════════════════════════════╣");
    eprintln!("║  Agent: {:<46} ║", primary);
    eprintln!("║  Tools: file read/write, search, code execution, skills ║");
    eprintln!("║  Commands: /help  /quit  /clear  /agents               ║");
    eprintln!("╚══════════════════════════════════════════════════════════╝");
    eprintln!();

    let mut messages: Vec<Message> = Vec::new();
    let stdin = std::io::stdin();
    let mut input = String::new();

    loop {
        input.clear();
        eprint!("🟢 {} > ", primary);
        std::io::Write::flush(&mut std::io::stdout()).ok();

        match stdin.read_line(&mut input) {
            Ok(0) => {
                eprintln!();
                break;
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("Read error: {e}");
                break;
            }
        }

        let line = input.trim();
        if line.is_empty() {
            continue;
        }

        // ── Built-in commands ──
        if let Some(cmd) = line.strip_prefix('/') {
            match cmd {
                "quit" | "exit" | "q" => break,
                "help" | "h" => {
                    eprintln!("Commands:");
                    eprintln!("  /quit        Exit chat");
                    eprintln!("  /clear       Clear conversation history");
                    eprintln!("  /help        Show this help");
                    eprintln!("  /agents      List configured agents");
                    eprintln!();
                    eprintln!("The AI agent has access to tools:");
                    eprintln!("  - Read/write files");
                    eprintln!("  - Search files and directories");
                    eprintln!("  - Execute shell commands");
                    eprintln!("  - Create and invoke skills");
                    eprintln!("  - Multi-turn conversation with context");
                    continue;
                }
                "clear" => {
                    messages.clear();
                    eprintln!("Conversation cleared.");
                    continue;
                }
                "agents" => {
                    for name in &agent_names {
                        eprintln!("  {name}");
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
            content: line.to_string(),
        });

        eprint!("🤖 ");
        std::io::Write::flush(&mut std::io::stdout()).ok();

        // ── Run agent with tool execution loop ──
        match run_agent_with_tools(&agent, &mut messages).await {
            Ok(()) => {}
            Err(e) => {
                eprintln!("\n⚠️  Error: {e}");
            }
        }
    }

    eprintln!("Goodbye!");
    Ok(())
}

/// Run a single agent turn with full tool execution support.
/// Streams the response to stdout, executes any tool calls the agent makes,
/// and appends the final assistant message to `messages`.
async fn run_agent_with_tools(agent: &Arc<dyn Agent>, messages: &mut Vec<Message>) -> Result<()> {
    // ── Phase 1: Agent chat with streaming ──
    let (tx, mut rx) = mpsc::channel::<String>(2048);
    let sender = StreamingSender::from(tx);
    let msgs = messages.clone();
    let options: Option<HashMap<String, Value>> = None;

    let agent_ref = Arc::clone(agent);
    let chat_task = tokio::spawn(async move { agent_ref.chat(msgs, None, options, sender).await });

    let mut response = String::new();
    let mut tool_calls: Vec<(String, String)> = Vec::new();
    let mut in_reasoning = false;

    while let Some(token) = rx.recv().await {
        // Tool call detection (agents emit __tool_call__:tool_name:args)
        // Use splitn(3, ':') to handle colons inside JSON tool args correctly.
        if token.starts_with("__tool_call__:") {
            let parts: Vec<&str> = token.splitn(3, ':').collect();
            if parts.len() == 3 {
                let tool_name = parts[1];
                let tool_args = parts[2];
                tool_calls.push((tool_name.to_string(), tool_args.to_string()));
                eprintln!();
                eprintln!("🔧 [Tool call: {tool_name}]");
                continue;
            }
        }

        // Reasoning content markers
        if token == "\u{001E}" {
            in_reasoning = true;
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
            // Do NOT add reasoning tokens to response — they would pollute conversation history
        } else {
            response.push_str(&token);
            print!("{}", token);
        }
        std::io::Write::flush(&mut std::io::stdout()).ok();
    }

    if let Err(e) = chat_task.await {
        warn!("Agent chat task failed: {e}");
    }
    eprintln!();

    // ── Phase 2: Execute any tool calls ──
    if !tool_calls.is_empty() {
        eprintln!("{}── Tool execution ──{}", ansi!("33"), ansi!("0"));
        let mut tool_results: Vec<String> = Vec::new();

        for (tool_name, tool_args_str) in &tool_calls {
            eprintln!("  ⚡ {tool_name}...");
            let parsed_args: Value = serde_json::from_str(tool_args_str).unwrap_or(json!({}));

            // Execute the tool. We use a simple approach: map known tools to actions.
            match execute_simple_tool(tool_name, &parsed_args).await {
                Ok(result_text) => {
                    let display = if result_text.len() > 500 {
                        format!(
                            "{}...\n[{} chars truncated]",
                            &result_text[..500],
                            result_text.len()
                        )
                    } else {
                        result_text.clone()
                    };
                    eprintln!("    {}✓{} {display}", ansi!("32"), ansi!("0"));
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
                    tool_results.push(format!(
                        "[Tool result: {}]\n{}[/Tool result]",
                        tool_name, result_for_llm
                    ));
                }
                Err(e) => {
                    eprintln!("    {}✗ Error: {e}{}", ansi!("31"), ansi!("0"));
                    tool_results.push(format!("[Tool error: {}]\n{}[/Tool error]", tool_name, e));
                }
            }
        }

        // ── Phase 3: Send tool results back to agent for follow-up ──
        if !tool_results.is_empty() {
            let combined = tool_results.join("\n\n");
            messages.push(Message {
                role: "assistant".to_string(),
                content: response.clone(),
            });
            messages.push(Message {
                role: "user".to_string(),
                content: format!(
                    "[Tool execution results]\n{}[/Tool execution results]\n\n\
                     Please continue based on the tool results above. \
                     If the task is complete, provide a summary.",
                    combined
                ),
            });

            eprint!("{}── Agent follow-up ──{}\n🤖 ", ansi!("33"), ansi!("0"));
            std::io::Write::flush(&mut std::io::stdout()).ok();

            let followup_response = agent_followup(agent, messages).await?;
            response = followup_response;
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

    Ok(())
}

/// Send tool results back to the agent and collect the follow-up response.
async fn agent_followup(agent: &Arc<dyn Agent>, messages: &[Message]) -> Result<String> {
    let (tx, mut rx) = mpsc::channel::<String>(2048);
    let sender = StreamingSender::from(tx);
    let msgs = messages.to_vec();

    let agent_ref = Arc::clone(agent);
    let task = tokio::spawn(async move { agent_ref.chat(msgs, None, None, sender).await });

    let mut response = String::new();
    while let Some(token) = rx.recv().await {
        if token.starts_with("__tool_call__:") {
            continue; // No nested tool execution (safety limit)
        }
        response.push_str(&token);
        print!("{}", token);
        std::io::Write::flush(&mut std::io::stdout()).ok();
    }

    task.await.ok();
    eprintln!();
    Ok(response)
}

/// Execute a simple tool call by name with given arguments.
/// Handles common file operations and shell commands.
async fn execute_simple_tool(name: &str, args: &Value) -> Result<String> {
    match name {
        "read_file" | "read" => {
            let path = args["path"]
                .as_str()
                .or_else(|| args["file_path"].as_str())
                .ok_or_else(|| anyhow::anyhow!("missing path argument"))?;
            let resolved = resolve_safe_path(path, false)?;
            let metadata = tokio::fs::metadata(&resolved)
                .await
                .with_context(|| format!("failed to read metadata for {path}"))?;
            if metadata.len() > MAX_FILE_READ_BYTES {
                anyhow::bail!(
                    "file too large: {} (max {} bytes)",
                    metadata.len(),
                    MAX_FILE_READ_BYTES
                );
            }
            let content = tokio::fs::read_to_string(&resolved)
                .await
                .with_context(|| format!("failed to read {path}"))?;
            Ok(content)
        }
        "write_file" | "write" | "create" => {
            let path = args["path"]
                .as_str()
                .or_else(|| args["file_path"].as_str())
                .ok_or_else(|| anyhow::anyhow!("missing path argument"))?;
            let content = args["content"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("missing content argument"))?;
            let resolved = resolve_safe_path(path, true)?;
            if let Some(parent) = resolved.parent() {
                tokio::fs::create_dir_all(parent).await.with_context(|| {
                    format!("failed to create directory for {}", parent.display())
                })?;
            }
            tokio::fs::write(&resolved, content)
                .await
                .with_context(|| format!("failed to write {}", resolved.display()))?;
            Ok(format!(
                "wrote {} bytes to {}",
                content.len(),
                resolved.display()
            ))
        }
        "search_files" | "grep" | "search" => {
            let pattern = args["pattern"]
                .as_str()
                .or_else(|| args["query"].as_str())
                .ok_or_else(|| anyhow::anyhow!("missing pattern argument"))?;
            let path = args["path"]
                .as_str()
                .or_else(|| args["directory"].as_str())
                .unwrap_or(".");
            let max_results = args["max_results"].as_u64().unwrap_or(20) as usize;

            let mut results = Vec::new();
            let mut dir = tokio::fs::read_dir(path)
                .await
                .with_context(|| format!("failed to read directory {path}"))?;
            while let Some(entry) = dir.next_entry().await? {
                if results.len() >= max_results {
                    break;
                }
                let fname = entry.file_name().to_string_lossy().to_string();
                if fname.contains(pattern) {
                    results.push(entry.path().display().to_string());
                }
            }
            Ok(if results.is_empty() {
                format!("No files matching '{pattern}' in {path}")
            } else {
                results.join("\n")
            })
        }
        "list_files" | "ls" => {
            let path = args["path"]
                .as_str()
                .or_else(|| args["directory"].as_str())
                .unwrap_or(".");
            let mut entries = Vec::new();
            let mut dir = tokio::fs::read_dir(path)
                .await
                .with_context(|| format!("failed to read directory {path}"))?;
            while let Some(entry) = dir.next_entry().await? {
                let fname = entry.file_name().to_string_lossy().to_string();
                let ftype = if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                    "dir"
                } else {
                    "file"
                };
                entries.push(format!(" [{ftype}] {fname}"));
            }
            entries.sort();
            Ok(entries.join("\n"))
        }
        "bash" | "execute_command" | "run" => {
            let command = args["command"]
                .as_str()
                .or_else(|| args["cmd"].as_str())
                .ok_or_else(|| anyhow::anyhow!("missing command argument"))?;
            let output = if cfg!(target_os = "windows") {
                tokio::process::Command::new("cmd")
                    .args(["/c", command])
                    .output()
                    .await
            } else {
                tokio::process::Command::new("sh")
                    .args(["-c", command])
                    .output()
                    .await
            }
            .with_context(|| format!("failed to execute: {command}"))?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let mut result = String::new();
            if !stdout.is_empty() {
                result.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str(&stderr);
            }
            if !output.status.success() {
                result.push_str(&format!("\nexit code: {:?}", output.status.code()));
            }
            Ok(result)
        }
        _ => Err(anyhow::anyhow!("Unknown tool: {name}")),
    }
}
