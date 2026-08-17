//! Simple single-tool execution and plain chat helper for the terminal chat
//! loop, plus the `execute_simple_tool` security tests.

use std::sync::Arc;

use anyhow::Result;
use serde_json::{json, Value};
use tokio::time::{timeout, Duration};

use crate::agents::agent::{Agent, Message};
use crate::governance::status::quick_check_tool as governance_gate;
use crate::orchestration::tool::{ToolInput, ToolOutput};

use super::ansi;
use super::commands::spawn_agent_chat;
use super::display::{
    append_cmd_result, format_cargo_check_output, format_cmd_output,
    format_inspect_git_diff_output, format_run_tests_output,
};
use super::tokens::{classify_token, TokenKind};
use super::tool_registry;

/// Execute a tool by name and arguments, returning the result as a string.
///
/// # Governance
///
/// Before executing, a lightweight governance gate (`governance::status::quick_check_tool`)
/// validates the tool name and arguments against minimal safety policies.
/// In a stricter deployment (e.g. a managed service), operations should additionally
/// be routed through the full `SecurityGovernor` / `HarnessBus` pipeline.
pub(super) async fn execute_simple_tool(name: &str, args: &Value) -> Result<String> {
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

/// Cached build of CLI principles — rebuilt only when skills change.
/// Uses a content fingerprint (name + description of every visible skill)
/// so both additions/removals and content edits invalidate the cache,
/// unlike a bare count which misses description changes.
pub(super) fn build_cli_principles() -> Vec<String> {
    static CACHED: std::sync::OnceLock<std::sync::RwLock<(Vec<String>, u64)>> =
        std::sync::OnceLock::new();
    let cache = CACHED.get_or_init(|| std::sync::RwLock::new((Vec::new(), u64::MAX)));

    // Fingerprint the skill content (sorted by name so runtime score changes
    // don't reorder entries and trigger spurious rebuilds).
    let current_fingerprint = crate::orchestration::tool::skill_registry()
        .and_then(|r| r.read().ok())
        .map(|g| {
            use std::hash::Hasher;
            let mut entries: Vec<(String, String)> = g
                .list(false)
                .into_iter()
                .map(|d| (d.name, d.description))
                .collect();
            entries.sort();
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            for (name, desc) in &entries {
                hasher.write(name.as_bytes());
                hasher.write(desc.as_bytes());
            }
            hasher.finish()
        })
        .unwrap_or(0);

    if let Ok(guard) = cache.read() {
        if guard.1 == current_fingerprint && !guard.0.is_empty() {
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
        guard.1 = current_fingerprint;
    }
    principles
}

/// Send a simple prompt to the agent and collect the full response as a string.
///
/// Unlike `run_agent_with_tools`, this returns only the text response without
/// tool execution. Ideal for AI-powered commands like `/commit`, `/plan`, `/review`.
pub(super) async fn chat_simple(
    agent: &Arc<dyn Agent>,
    prompt: Vec<Message>,
    principles: Vec<String>,
) -> Result<String> {
    let principles_opt = if principles.is_empty() {
        None
    } else {
        Some(principles)
    };
    let (_chat_task, mut rx) = spawn_agent_chat(Arc::clone(agent), prompt, principles_opt);

    let mut response = String::new();
    let mut chunks = 0usize;
    let mut total_chars = 0usize;
    while let Some(token) = rx.recv().await {
        let next_chars = token.chars().count();
        if crate::acp::helpers::conversation::stream_would_exceed_limits(
            chunks,
            total_chars,
            next_chars,
        ) {
            eprintln!(
                "{}[truncated at {} chars]{}",
                ansi!("33"),
                total_chars,
                ansi!("0")
            );
            break;
        }
        chunks += 1;
        total_chars += next_chars;
        match classify_token(&token) {
            // Skip reasoning markers and tool calls for simple chat
            TokenKind::ReasoningStart | TokenKind::ReasoningEnd => continue,
            TokenKind::ToolCall(..) => continue,
            // Strip __thinking__ prefix from reasoning tokens
            TokenKind::Thinking(think) => {
                eprintln!("{}💭 {}{}", ansi!("90"), think, ansi!("0"));
                continue;
            }
            // Skip finish_reason and usage telemetry tokens
            TokenKind::Telemetry => continue,
            TokenKind::Content => {}
        }
        response.push_str(&token);
    }
    Ok(response.trim().to_string())
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
