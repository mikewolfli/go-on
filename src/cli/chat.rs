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

use anyhow::Result;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};
use tracing::{debug, warn};

use crate::acp::helpers::autonomy::run_followup_after_tool_observation;
use crate::acp::helpers::autonomy::terminal_chat_contract_snapshot;
use crate::agents::agent::{Agent, AgentRegistry, Message, StreamingSender};
use crate::config::AppConfig;
use crate::flow::FlowManager;
use crate::governance::status::quick_check_tool as governance_gate;
use crate::intelligence::capability_graph::CapabilityGraph;
use crate::orchestration::autonomy_runtime::{
    build_tool_execution_followup_message, build_tool_result_block, parse_tool_call_token,
};
use crate::orchestration::tool::{ToolInput, ToolRegistry};

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
    // The old code accepted multiple field-name variants (path/file_path,
    // pattern/query, command/cmd).  We normalise them here before routing
    // through the registry so callers continue to work unchanged.
    let mut payload = args.clone();
    if let Some(v) = payload.get("file_path").and_then(|v| v.as_str()) {
        payload["path"] = json!(v);
    }
    if let Some(v) = payload.get("query").and_then(|v| v.as_str()) {
        payload["pattern"] = json!(v);
    }
    if let Some(v) = payload.get("directory").and_then(|v| v.as_str()) {
        // list_directory uses "path" rather than "directory"
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
        // Attempt to read a file outside workspace via path traversal
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
