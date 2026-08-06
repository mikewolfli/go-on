//! Shared execution infrastructure for tool implementations.
//!
//! Centralizes timeout handling, output truncation, result building, and
//! blocked-command filtering so individual tools (shell_exec, build,
//! git, docker, etc.) don't duplicate these patterns.
//!
//! ## Rationale
//!
//! Previously each tool had its own copy of `truncate_output`, `MAX_OUTPUT_BYTES`,
//! blocked-pattern lists, and `ToolOutput` construction boilerplate. This module
//! eliminates that duplication and provides a single place to tune safety limits.

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::ToolOutput;
use tracing::warn;

// ---------------------------------------------------------------------------
// Output size limits
// ---------------------------------------------------------------------------

/// Maximum bytes of stdout/stderr retained per execution (10 MB).
/// Output beyond this limit is silently truncated to prevent OOM conditions
/// in the LLM context window.
pub const MAX_OUTPUT_BYTES: usize = 10 * 1024 * 1024;

/// Truncate a string to `MAX_OUTPUT_BYTES` if it exceeds that limit.
///
/// Uses `String::truncate()` which is O(1) for ASCII content. For multi-byte
/// UTF-8 boundaries, truncation may split a character — the partial char will
/// appear as the Unicode replacement character, which is acceptable for a
/// safety boundary.
pub fn truncate_output(s: &mut String) {
    if s.len() > MAX_OUTPUT_BYTES {
        warn!(
            "exec_common TRUNCATED: {} bytes > {} max",
            s.len(),
            MAX_OUTPUT_BYTES
        );
        s.truncate(MAX_OUTPUT_BYTES);
    }
}

// ---------------------------------------------------------------------------
// Shell command result builder
// ---------------------------------------------------------------------------

/// Build a standard `ToolOutput` for shell command execution results.
///
/// Used by `shell_exec` and any tool that runs an
/// external command and wants consistent output formatting.
pub fn build_shell_tool_output(
    success: bool,
    stdout: String,
    stderr: String,
    exit_code: Option<i32>,
    command: &str,
    directory: &str,
    tool_name: &str,
) -> ToolOutput {
    let mut audit_stdout = stdout.clone();
    let mut audit_stderr = stderr.clone();
    truncate_output(&mut audit_stdout);
    truncate_output(&mut audit_stderr);

    ToolOutput {
        success,
        result: Some(serde_json::json!({
            "stdout": stdout,
            "stderr": stderr,
            "exit_code": exit_code,
            "command": command,
            "directory": directory,
        })),
        error: (!success).then(|| stderr.trim().to_string()),
        verification: Some("shell_command_executed".to_string()),
        audit_log: Some(format!(
            "Shell exec '{}' in '{}' (exit: {:?})",
            command, directory, exit_code
        )),
        pua_report: Some(tool_execution_report(
            tool_name,
            Some("shell_command_executed"),
        )),
    }
}

/// Build a timeout ToolOutput for a shell command that exceeded its time limit.
pub fn build_timeout_tool_output(
    stdout: String,
    stderr: String,
    command: &str,
    directory: &str,
    timeout_ms: u64,
    tool_name: &str,
) -> ToolOutput {
    ToolOutput {
        success: false,
        result: Some(serde_json::json!({
            "stdout": stdout,
            "stderr": stderr,
            "exit_code": null,
            "command": command,
            "directory": directory,
            "timeout": true,
        })),
        error: Some(format!("Command timed out after {}ms", timeout_ms)),
        verification: Some("shell_command_executed".to_string()),
        audit_log: Some(format!(
            "{} exec '{}' in '{}' timed out after {}ms",
            tool_name, command, directory, timeout_ms
        )),
        pua_report: Some(tool_execution_report(
            tool_name,
            Some("shell_command_executed"),
        )),
    }
}

/// Build a blocked-by-sandbox ToolOutput for a command that was rejected.
pub fn build_blocked_tool_output(pattern: &str, command: &str, tool_name: &str) -> ToolOutput {
    ToolOutput {
        success: false,
        result: None,
        error: Some(format!(
            "Command blocked by security policy: contains '{}'",
            pattern
        )),
        verification: Some("shell_sandbox_blocked".to_string()),
        audit_log: Some(format!(
            "BLOCKED {} (pattern '{}'): {}",
            tool_name, pattern, command
        )),
        pua_report: Some(tool_execution_report(
            tool_name,
            Some("shell_sandbox_blocked"),
        )),
    }
}

// ---------------------------------------------------------------------------
// Blocked command patterns
// ---------------------------------------------------------------------------

/// Patterns that are blocked in shell execution tools.
///
/// These prevent dangerous operations (rm -rf /, fork bombs, format, etc.)
/// regardless of the tool that invokes them. This is the single canonical
/// block-list: the governance terminal-chat gate (`governance::status`)
/// delegates here so both entry points agree on what is blocked.
pub fn is_blocked_command(command: &str) -> Option<&'static str> {
    let command_lower = command.to_lowercase();
    let blocked_patterns: &[&str] = &[
        "rm -rf /",
        "rm -rf /*",
        "rm -rf --no-preserve-root",
        "sudo rm -rf",
        "mkfs.",
        "sudo mkfs",
        "dd if=",
        "sudo dd",
        "format ",
        ":(){ :|:& };:", // fork bomb (full form)
        ":(){ ",         /* fork bomb (abbreviated) */
        "fork bomb",
        "chmod -R 000",
        "chmod 777 /",
        "chown -R",
        "> /dev/sda",
        "> /dev/hda",
        "> /dev/sd",
        "> /dev/disk",
        "| shutdown",
        "| reboot",
        "shutdown",
        "reboot",
        "halt",
        "poweroff",
        "sudo shutdown",
        "sudo reboot",
        "wget http://",
        "wget -O - |",
        "curl http://",
        "curl | sh",
        "curl | bash",
        "nmap ",
        "hydra ",
        "eval ",
    ];
    if let Some(pattern) = blocked_patterns
        .iter()
        .find(|pattern| command_lower.contains(**pattern))
    {
        return Some(*pattern);
    }
    // Also block commands that pipe into a shell (blind execution of remote content).
    if command_lower.contains("| sh")
        || command_lower.contains("| bash")
        || command_lower.contains("| zsh")
    {
        return Some("pipe-to-shell");
    }
    // Block destructive redirects to block devices (allow /dev/null).
    if command_lower.contains("> /dev/") && !command_lower.contains("/dev/null") {
        return Some("redirect to block device");
    }
    None
}

// ---------------------------------------------------------------------------
// Timeout utilities
// ---------------------------------------------------------------------------

/// Maximum allowed timeout (5 minutes). Any requested timeout above this is
/// silently capped.
pub const MAX_TIMEOUT_SECS: u64 = 300;

/// Cap a requested timeout to the maximum allowed value.
pub fn cap_timeout_secs(requested_secs: u64) -> u64 {
    std::cmp::min(requested_secs, MAX_TIMEOUT_SECS)
}
