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
use anyhow::Context;
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

// ---------------------------------------------------------------------------
// Blocking tokio runtime
// ---------------------------------------------------------------------------

/// Shared dedicated blocking tokio runtime for synchronous tool `run()`
/// paths. Tools must never call `block_on` on an async worker; this runtime
/// is created once and reused so each tool does not build its own runtime.
pub fn blocking_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build shared blocking tool runtime")
    })
}

/// Run `f` with exclusive access to the shared blocking runtime.
///
/// A current-thread runtime must not be driven concurrently from multiple OS
/// threads: parallel tool calls (e.g. two LSP queries in one tool batch) run
/// on separate blocking-pool threads, and tokio treats concurrent `block_on`
/// on the same runtime as UB/deadlock. The mutex is held only for the
/// duration of `f`. All sync `run()` paths must use this instead of calling
/// `blocking_runtime().block_on(...)` directly.
pub fn with_blocking_runtime<T>(f: impl FnOnce(&tokio::runtime::Runtime) -> T) -> T {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    let _guard = LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    f(blocking_runtime())
}

// ---------------------------------------------------------------------------
// Capped file reads
// ---------------------------------------------------------------------------

/// Cap for tools that buffer whole files in memory (read_file, compress,
/// decompress, gzip extraction): a model-picked 10GB file must not OOM the
/// process. 1 GiB comfortably covers legitimate use (logs, bundles, dumps).
pub const MAX_TOOL_FILE_READ_BYTES: usize = 1024 * 1024 * 1024;

/// Read a file with a byte cap (input-side OOM guard). Uses the metadata
/// length for a cheap pre-check, then enforces the cap during the read.
pub fn read_file_capped(path: &std::path::Path, cap: usize) -> anyhow::Result<Vec<u8>> {
    let file =
        std::fs::File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    if let Some(len) = file.metadata().ok().map(|m| m.len()) {
        if len > cap as u64 {
            anyhow::bail!(
                "file '{}' exceeds the {} byte input limit",
                path.display(),
                cap
            );
        }
    }
    use std::io::Read;
    let mut data = Vec::new();
    let read = file
        .take(cap as u64 + 1)
        .read_to_end(&mut data)
        .with_context(|| format!("failed to read {}", path.display()))?;
    if read as u64 > cap as u64 {
        anyhow::bail!(
            "file '{}' exceeds the {} byte input limit",
            path.display(),
            cap
        );
    }
    Ok(data)
}

/// Async variant of [`read_file_capped`] for tokio contexts.
pub async fn read_file_capped_async(path: &std::path::Path, cap: usize) -> anyhow::Result<Vec<u8>> {
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("failed to read {}", path.display()))?;
    if let Ok(meta) = file.metadata().await {
        if meta.len() > cap as u64 {
            anyhow::bail!(
                "file '{}' exceeds the {} byte input limit",
                path.display(),
                cap
            );
        }
    }
    use tokio::io::AsyncReadExt;
    let mut data = Vec::new();
    let read = (&mut file)
        .take(cap as u64 + 1)
        .read_to_end(&mut data)
        .await
        .with_context(|| format!("failed to read {}", path.display()))?;
    if read as u64 > cap as u64 {
        anyhow::bail!(
            "file '{}' exceeds the {} byte input limit",
            path.display(),
            cap
        );
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn read_file_capped_rejects_oversized_files() {
        let tmp = TempDir::new().unwrap();
        let small = tmp.path().join("small.txt");
        std::fs::write(&small, b"hello").unwrap();
        let big = tmp.path().join("big.txt");
        std::fs::write(&big, b"x".repeat(2048)).unwrap();

        // Under the cap: content returned as-is.
        let data = read_file_capped(&small, 1024).unwrap();
        assert_eq!(data, b"hello");
        // Over the cap (metadata pre-check): rejected.
        let err = read_file_capped(&big, 1024).unwrap_err();
        assert!(err.to_string().contains("limit"), "got: {err}");
        // Cap enforced during the read too (metadata check bypassed).
        let err2 = read_file_capped(&big, 10).unwrap_err();
        assert!(err2.to_string().contains("limit"), "got: {err2}");
    }
}
