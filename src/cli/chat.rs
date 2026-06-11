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
use tokio::time::{timeout, Duration};
use tracing::{debug, warn};

use crate::acp::helpers::autonomy::run_followup_after_tool_observation;
use crate::acp::helpers::autonomy::terminal_chat_contract_snapshot;
use crate::agents::agent::{Agent, AgentRegistry, Message, StreamingSender};
use crate::config::AppConfig;
use crate::flow::FlowManager;
use crate::intelligence::capability_graph::CapabilityGraph;
use crate::orchestration::autonomy_runtime::{
    build_tool_execution_followup_message, build_tool_result_block, parse_tool_call_token,
};

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
///
/// TOCTOU-safe: returns the canonicalized path that should be used for all subsequent
/// file operations to prevent symlink race conditions.
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
            // Return the canonicalized parent + filename to avoid TOCTOU
            return Ok(canon_parent.join(target.file_name().unwrap_or_default()));
        }
        return Ok(target);
    }
    // For existing files, canonicalize the full path and return it
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

    let _flow = Arc::new(FlowManager::new(Arc::clone(&config), None));

    let agent_names: Vec<String> = config.agents().keys().cloned().collect();
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
        if let Some((tool_name, tool_args)) = parse_tool_call_token(&token) {
            tool_calls.push((tool_name.to_string(), tool_args.to_string()));
            eprintln!();
            eprintln!("🔧 [Tool call: {tool_name}]");
            continue;
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
    let mut followup_round_executed = false;
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
                    tool_results.push(build_tool_result_block(tool_name, &result_for_llm, false));
                }
                Err(e) => {
                    eprintln!("    {}✗ Error: {e}{}", ansi!("31"), ansi!("0"));
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
                content: build_tool_execution_followup_message(&tool_results, false),
            });

            eprint!("{}── Agent follow-up ──{}\n🤖 ", ansi!("33"), ansi!("0"));
            std::io::Write::flush(&mut std::io::stdout()).ok();

            let (followup_response, _, _) = run_followup_after_tool_observation(
                Arc::clone(agent),
                messages.clone(),
                None,
                None,
                None,
            )
            .await?;
            crate::acp::helpers::autonomy_metrics::record_tool_followup_attempt();
            if followup_response.trim().is_empty() {
                crate::acp::helpers::autonomy_metrics::record_tool_followup_fallback();
            } else {
                crate::acp::helpers::autonomy_metrics::record_tool_followup_success();
            }
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

    let autonomy_contract =
        terminal_chat_contract_snapshot(tool_calls.len(), followup_round_executed, &response);
    debug!(
        target: "go_on::cli::chat",
        autonomy_contract = %autonomy_contract,
        "terminal chat turn completed"
    );

    Ok(())
}

/// Execute a tool by name and arguments, returning the result as a string.
///
/// # Governance bypass
///
/// This function intentionally bypasses the governance layer for simple
/// read/write/file operations in the terminal chat context.  In a more
/// restrictive deployment (e.g. a managed service) these operations should
/// be routed through `enforce_action` to apply sandbox and policy checks.
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
                timeout(
                    Duration::from_secs(300),
                    tokio::process::Command::new("cmd")
                        .args(["/c", command])
                        .output(),
                )
                .await
                .map_err(|_| anyhow::anyhow!("command timed out after 300s: {command}"))?
            } else {
                timeout(
                    Duration::from_secs(300),
                    tokio::process::Command::new("sh")
                        .args(["-c", command])
                        .output(),
                )
                .await
                .map_err(|_| anyhow::anyhow!("command timed out after 300s: {command}"))?
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── resolve_safe_path tests ──────────────────────────────────────

    /// Verify that `resolve_safe_path` rejects paths with ".." that escape
    /// the current working directory.
    #[test]
    fn test_resolve_safe_path_rejects_traversal() {
        // Path traversal that escapes cwd
        let result = resolve_safe_path("../../../etc/passwd", false);
        assert!(
            result.is_err(),
            "path traversal should be rejected, got: {:?}",
            result
        );
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("outside the workspace") || msg.contains("does not exist"),
            "error should mention outside workspace or does not exist, got: {}",
            msg
        );

        // Absolute path outside cwd
        let result = resolve_safe_path("/etc/passwd", false);
        assert!(
            result.is_err(),
            "absolute path outside cwd should be rejected"
        );
    }

    /// Verify that `resolve_safe_path` allows paths within the workspace.
    #[test]
    fn test_resolve_safe_path_allows_relative_paths() {
        // A relative path to a file that exists (Cargo.toml should be in cwd)
        let result = resolve_safe_path("Cargo.toml", false);
        assert!(
            result.is_ok(),
            "Cargo.toml in cwd should be resolvable, got: {:?}",
            result
        );

        // A relative path to a new file (with allow_new_file = true)
        let result = resolve_safe_path("test_temp_new_file.txt", true);
        assert!(
            result.is_ok(),
            "new file in cwd should be resolvable with allow_new_file=true, got: {:?}",
            result
        );
    }

    /// Verify that allowing a new file checks the parent directory exists.
    #[test]
    fn test_resolve_safe_path_new_file_in_nonexistent_dir() {
        let result = resolve_safe_path("nonexistent_dir/some_file.txt", true);
        assert!(
            result.is_err(),
            "new file in nonexistent parent dir should fail"
        );
    }

    // ── execute_simple_tool security ──────────────────────────────────

    /// Verify that `execute_simple_tool` rejects path traversal via the
    /// `name` field (read_file with traversal path).
    #[tokio::test]
    async fn test_execute_simple_tool_rejects_traversal() {
        // Attempt to read a file outside workspace via path traversal
        let result =
            execute_simple_tool("read_file", &json!({"path": "../../../etc/passwd"})).await;
        assert!(
            result.is_err(),
            "read_file with traversal path should be rejected"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("outside the workspace") || err.contains("does not exist"),
            "error should mention security, got: {}",
            err
        );
    }

    /// Verify that executing a simple tool with missing arguments returns
    /// a descriptive error.
    #[tokio::test]
    async fn test_execute_simple_tool_missing_arguments() {
        let result = execute_simple_tool("read_file", &json!({})).await;
        assert!(result.is_err(), "read_file without path should fail");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("missing path argument"),
            "error should mention missing path"
        );

        let result = execute_simple_tool("write_file", &json!({"path": "test.txt"})).await;
        assert!(result.is_err(), "write_file without content should fail");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("missing content argument"),
            "error should mention missing content"
        );
    }

    /// Verify that unknown tool names produce a descriptive error.
    #[tokio::test]
    async fn test_execute_simple_tool_unknown_tool() {
        let result = execute_simple_tool("nonexistent_tool", &json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown tool"));
    }

    // ── Safety edge cases ────────────────────────────────────────────

    /// Verify that resolve_safe_path with relative path that stays within
    /// workspace works even when file doesn't exist (allow_new_file=true).
    #[test]
    fn test_resolve_safe_path_new_file_allowed() {
        // This should work: path is relative and parent (cwd) exists
        let result = resolve_safe_path("_test_write_cleanup.txt", true);
        assert!(result.is_ok(), "new file in cwd should be OK");
    }

    /// Verify that resolve_safe_path with file that exists works.
    #[test]
    fn test_resolve_safe_path_existing_file() {
        let cwd = std::env::current_dir().expect("current directory should be available");
        // Try to resolve the src dir which definitely exists
        let result = resolve_safe_path("src", false);
        assert!(result.is_ok(), "src dir should be resolvable");
        let path = result.expect("resolve_safe_path should succeed for src dir");
        assert!(
            path.starts_with(&cwd),
            "resolved path should start with cwd"
        );
    }
}
