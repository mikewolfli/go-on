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
///
/// # Parameters
/// Eight parameters is one above the clippy default; each maps 1:1 to a
/// `ToolOutput` field and merging them would obscure the audit trail.
#[allow(clippy::too_many_arguments)]
pub fn build_shell_tool_output(
    success: bool,
    mut stdout: String,
    mut stderr: String,
    exit_code: Option<i32>,
    command: &str,
    directory: &str,
    tool_name: &str,
    sandbox: &str,
) -> ToolOutput {
    // Enforce the output cap at this single choke point: previously the
    // truncated clones (`audit_stdout`/`audit_stderr`) were never referenced
    // while the returned payload embedded the *untruncated* buffers, so the
    // 10 MiB OOM guard only worked for callers that happened to truncate
    // before calling this builder (shell.rs success path) and never for the
    // timeout path.
    truncate_output(&mut stdout);
    truncate_output(&mut stderr);

    ToolOutput {
        success,
        result: Some(serde_json::json!({
            "stdout": stdout,
            "stderr": stderr,
            "exit_code": exit_code,
            "command": command,
            "directory": directory,
            "sandbox": sandbox,
        })),
        error: (!success).then(|| stderr.trim().to_string()),
        verification: Some("shell_command_executed".to_string()),
        audit_log: Some(format!(
            "Shell exec '{}' in '{}' (exit: {:?}, sandbox: {})",
            command, directory, exit_code, sandbox
        )),
        pua_report: Some(tool_execution_report(
            tool_name,
            Some("shell_command_executed"),
        )),
    }
}

/// Build a timeout ToolOutput for a shell command that exceeded its time limit.
#[allow(clippy::too_many_arguments)]
pub fn build_timeout_tool_output(
    mut stdout: String,
    mut stderr: String,
    command: &str,
    directory: &str,
    timeout_ms: u64,
    tool_name: &str,
    sandbox: &str,
) -> ToolOutput {
    // Apply the same output cap as the success path: a chatty command that
    // times out must not leak unbounded output into the result payload.
    truncate_output(&mut stdout);
    truncate_output(&mut stderr);
    ToolOutput {
        success: false,
        result: Some(serde_json::json!({
            "stdout": stdout,
            "stderr": stderr,
            "exit_code": null,
            "command": command,
            "directory": directory,
            "timeout": true,
            "sandbox": sandbox,
        })),
        error: Some(format!("Command timed out after {}ms", timeout_ms)),
        verification: Some("shell_command_executed".to_string()),
        audit_log: Some(format!(
            "{} exec '{}' in '{}' timed out after {}ms (sandbox: {})",
            tool_name, command, directory, timeout_ms, sandbox
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

/// Text variant of [`read_file_capped`]: bytes are decoded lossily so text-
/// oriented tools (TOML/YAML/XML/plain parsers) get the same input-side OOM
/// guard without duplicating the cap logic per tool.
pub fn read_text_capped(path: &std::path::Path, cap: usize) -> anyhow::Result<String> {
    Ok(String::from_utf8_lossy(&read_file_capped(path, cap)?).into_owned())
}

/// Output of a command run under a byte cap.
pub struct CappedCommandOutput {
    /// Process exit code (None when the process was signaled).
    pub status: Option<i32>,
    /// stdout bytes (truncated at `cap`).
    pub stdout: Vec<u8>,
    /// stderr bytes (truncated at `cap`).
    pub stderr: Vec<u8>,
    /// True when stdout exceeded `cap` (explicit, never silent).
    pub stdout_truncated: bool,
    /// True when stderr exceeded `cap` (explicit, never silent).
    pub stderr_truncated: bool,
}

impl CappedCommandOutput {
    /// Decode stdout lossily.
    pub fn stdout_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
    /// Decode stderr lossily.
    pub fn stderr_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

/// Run a command with BOTH stdout and stderr capped at `cap` bytes each.
///
/// `Command::output()`/`wait_with_output()` buffer the full output — a
/// `git diff` over a huge repo or a verbose `cargo test` would OOM the
/// process before the tool ever formatted a response. This spawns and reads
/// the two pipes concurrently (one thread per pipe, so a full stdout pipe
/// cannot deadlock the child's stderr writes), KEEPING only the first `cap`
/// bytes of each stream while draining the rest — the child runs to natural
/// completion (a `Read::take`-style stop would SIGPIPE-kill it on an
/// oversized write and lose the true exit status). Output beyond the cap is
/// dropped and the corresponding `*_truncated` flag is set — callers must
/// surface it (warn/log), never silently truncate.
pub fn run_command_capped(
    cmd: &mut std::process::Command,
    cap: usize,
) -> anyhow::Result<CappedCommandOutput> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn command")?;
    let stdout = child
        .stdout
        .take()
        .map(|r| -> Box<dyn Read + Send> { Box::new(r) });
    let stderr = child
        .stderr
        .take()
        .map(|r| -> Box<dyn Read + Send> { Box::new(r) });
    let read_capped = move |mut reader: Box<dyn Read + Send>| -> (Vec<u8>, bool) {
        let mut kept = Vec::with_capacity(cap.min(64 * 1024));
        let mut truncated = false;
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if kept.len() < cap {
                        let take = (cap - kept.len()).min(n);
                        kept.extend_from_slice(&chunk[..take]);
                        if take < n {
                            truncated = true;
                        }
                    } else {
                        truncated = true;
                    }
                }
                Err(_) => break,
            }
        }
        (kept, truncated)
    };
    let stdout_thread = stdout.map(|r| std::thread::spawn(move || read_capped(r)));
    let stderr_thread = stderr.map(|r| std::thread::spawn(move || read_capped(r)));
    let status = child.wait().ok().and_then(|s| s.code());
    let stdout_res = stdout_thread.and_then(|h| h.join().ok());
    let stderr_res = stderr_thread.and_then(|h| h.join().ok());
    Ok(CappedCommandOutput {
        status,
        stdout: stdout_res
            .as_ref()
            .map(|(b, _)| b.clone())
            .unwrap_or_default(),
        stderr: stderr_res
            .as_ref()
            .map(|(b, _)| b.clone())
            .unwrap_or_default(),
        stdout_truncated: stdout_res.map(|(_, t)| t).unwrap_or(false),
        stderr_truncated: stderr_res.map(|(_, t)| t).unwrap_or(false),
    })
}

/// Tokio variant of [`run_command_capped`]: the two pipe reads run as
/// concurrent futures (no threads) — the async equivalent of the thread-per-
/// pipe design above, with the same keep-first-drain-rest semantics.
pub async fn run_command_capped_async(
    cmd: &mut tokio::process::Command,
    cap: usize,
) -> anyhow::Result<CappedCommandOutput> {
    use tokio::io::AsyncReadExt;
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to spawn command")?;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let stdout_fut = async {
        let mut kept = Vec::with_capacity(cap.min(64 * 1024));
        let mut truncated = false;
        let mut chunk = [0u8; 4096];
        if let Some(mut reader) = stdout.take() {
            loop {
                match reader.read(&mut chunk).await {
                    Ok(0) => break,
                    Ok(n) => {
                        if kept.len() < cap {
                            let take = (cap - kept.len()).min(n);
                            kept.extend_from_slice(&chunk[..take]);
                            if take < n {
                                truncated = true;
                            }
                        } else {
                            truncated = true;
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        (kept, truncated)
    };
    let stderr_fut = async {
        let mut kept = Vec::with_capacity(cap.min(64 * 1024));
        let mut truncated = false;
        let mut chunk = [0u8; 4096];
        if let Some(mut reader) = stderr.take() {
            loop {
                match reader.read(&mut chunk).await {
                    Ok(0) => break,
                    Ok(n) => {
                        if kept.len() < cap {
                            let take = (cap - kept.len()).min(n);
                            kept.extend_from_slice(&chunk[..take]);
                            if take < n {
                                truncated = true;
                            }
                        } else {
                            truncated = true;
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        (kept, truncated)
    };
    let (stdout_res, stderr_res) = tokio::join!(stdout_fut, stderr_fut);
    let status = child.wait().await.ok().and_then(|s| s.code());
    Ok(CappedCommandOutput {
        status,
        stdout: stdout_res.0,
        stderr: stderr_res.0,
        stdout_truncated: stdout_res.1,
        stderr_truncated: stderr_res.1,
    })
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

// ---------------------------------------------------------------------------
// Sandbox-aware command runners (LAYER 3: bubblewrap isolation)
// ---------------------------------------------------------------------------
//
// The command-executing tools (git, cargo, run_tests, build, diagnostics,
// diff, ping, clippy --fix, ...) all funnel through these runners so the OS
// sandbox covers every model-issued command, not just shell_exec. Each runner:
// 1. wraps `program args` in bwrap when the effective sandbox mode requires it
//    and the probe passed (see security::sandbox),
// 2. scrubs credential env vars whenever a sandbox mode is requested
//    (independent of whether bwrap is available),
// 3. degrades to direct execution on a spawn-level failure OR a bwrap setup
//    failure (spawned fine but exited 1 with `bwrap:` stderr — e.g.
//    unprivileged user namespaces denied). A plain non-zero exit is never
//    degraded — that is the sandbox doing its job.
//
// `configure` reconfigures the base command (env vars, stdio, stdin) and is
// re-invoked when degrading, so it must be side-effect-free beyond `Command`
// and capture by reference (Fn, not FnOnce).

/// Build the base command for `program args`, applying env scrubbing when
/// `scrub` is set. The wrapper is applied by passing the wrapped program/args.
fn build_sandboxed_base_cmd(
    workspace: &std::path::Path,
    program: &str,
    args: &[String],
    scrub: bool,
    configure: &impl Fn(&mut std::process::Command),
) -> std::process::Command {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    cmd.current_dir(workspace);
    if scrub {
        cmd.env_clear();
        let passthrough = crate::security::sandbox::sandbox_config()
            .map(|c| c.passthrough_env)
            .unwrap_or_default();
        for (key, val) in crate::security::sandbox::sanitized_env(&passthrough) {
            cmd.env(key, val);
        }
    }
    configure(&mut cmd);
    cmd
}

/// Sandbox-aware [`run_command_capped`] for sync tool code.
pub fn run_sandboxed_capped(
    workspace: &std::path::Path,
    program: &str,
    args: &[String],
    cap: usize,
    configure: impl Fn(&mut std::process::Command),
) -> anyhow::Result<CappedCommandOutput> {
    let prep = crate::security::sandbox::prepare_command(
        crate::security::sandbox::effective_mode(),
        workspace,
        program,
        args,
    );
    let attempt =
        |program: &str, args: &[String], scrub: bool| -> anyhow::Result<CappedCommandOutput> {
            let mut cmd = build_sandboxed_base_cmd(workspace, program, args, scrub, &configure);
            run_command_capped(&mut cmd, cap)
        };
    match attempt(&prep.program, &prep.args, prep.scrub_env) {
        Ok(capped)
            if prep.applied
                && crate::security::sandbox::is_bwrap_setup_failure(
                    capped.status,
                    &capped.stderr_lossy(),
                ) =>
        {
            crate::security::sandbox::record_sandbox_degraded();
            tracing::warn!(
                mode = prep.mode.as_str(),
                "sandboxed command failed at setup (bwrap error) — retrying without sandbox"
            );
            attempt(program, args, prep.scrub_env)
        }
        Err(_) if prep.applied => {
            crate::security::sandbox::record_sandbox_degraded();
            tracing::warn!(
                mode = prep.mode.as_str(),
                "sandboxed command spawn failed — falling back to direct execution"
            );
            attempt(program, args, prep.scrub_env)
        }
        other => other,
    }
}

/// Sandbox-aware [`run_command_capped_async`] for tokio tool code.
pub async fn run_sandboxed_capped_async(
    workspace: &std::path::Path,
    program: &str,
    args: &[String],
    cap: usize,
    configure: impl Fn(&mut tokio::process::Command),
) -> anyhow::Result<CappedCommandOutput> {
    let prep = crate::security::sandbox::prepare_command(
        crate::security::sandbox::effective_mode(),
        workspace,
        program,
        args,
    );
    let attempt = |program: &str, args: &[String], scrub: bool| {
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args);
        cmd.current_dir(workspace);
        if scrub {
            cmd.env_clear();
            let passthrough = crate::security::sandbox::sandbox_config()
                .map(|c| c.passthrough_env)
                .unwrap_or_default();
            for (key, val) in crate::security::sandbox::sanitized_env(&passthrough) {
                cmd.env(key, val);
            }
        }
        configure(&mut cmd);
        async move { run_command_capped_async(&mut cmd, cap).await }
    };
    match attempt(&prep.program, &prep.args, prep.scrub_env).await {
        Ok(capped)
            if prep.applied
                && crate::security::sandbox::is_bwrap_setup_failure(
                    capped.status,
                    &capped.stderr_lossy(),
                ) =>
        {
            crate::security::sandbox::record_sandbox_degraded();
            tracing::warn!(
                mode = prep.mode.as_str(),
                "sandboxed command failed at setup (bwrap error) — retrying without sandbox"
            );
            attempt(program, args, prep.scrub_env).await
        }
        Err(_) if prep.applied => {
            crate::security::sandbox::record_sandbox_degraded();
            tracing::warn!(
                mode = prep.mode.as_str(),
                "sandboxed command spawn failed — falling back to direct execution"
            );
            attempt(program, args, prep.scrub_env).await
        }
        other => other,
    }
}

/// Sandbox-aware single-shot `.output()` run for tools that need the full
/// [`std::process::Output`] (git, ping, diff, diagnostics, clippy --fix).
/// Returns the output plus whether the sandbox was applied.
pub fn run_sandboxed_output(
    workspace: &std::path::Path,
    program: &str,
    args: &[String],
    configure: impl Fn(&mut std::process::Command),
) -> anyhow::Result<(std::process::Output, bool)> {
    let prep = crate::security::sandbox::prepare_command(
        crate::security::sandbox::effective_mode(),
        workspace,
        program,
        args,
    );
    let attempt =
        |program: &str, args: &[String], scrub: bool| -> anyhow::Result<std::process::Output> {
            let mut cmd = build_sandboxed_base_cmd(workspace, program, args, scrub, &configure);
            cmd.output().context("failed to spawn command")
        };
    let first = match attempt(&prep.program, &prep.args, prep.scrub_env) {
        Ok(out) => out,
        Err(_) if prep.applied => {
            crate::security::sandbox::record_sandbox_degraded();
            tracing::warn!(
                mode = prep.mode.as_str(),
                "sandboxed command spawn failed — falling back to direct execution"
            );
            return Ok((attempt(program, args, prep.scrub_env)?, false));
        }
        Err(e) => return Err(e),
    };
    if prep.applied
        && crate::security::sandbox::is_bwrap_setup_failure(
            first.status.code(),
            &String::from_utf8_lossy(&first.stderr),
        )
    {
        crate::security::sandbox::record_sandbox_degraded();
        tracing::warn!(
            mode = prep.mode.as_str(),
            "sandboxed command failed at setup (bwrap error) — retrying without sandbox"
        );
        return Ok((attempt(program, args, prep.scrub_env)?, false));
    }
    Ok((first, prep.applied))
}

/// Sandbox-aware spawn-and-feed-stdin run (apply_patch pipes the patch to
/// `git apply` via stdin). Same degrade semantics as the other runners.
pub fn run_sandboxed_stdin_output(
    workspace: &std::path::Path,
    program: &str,
    args: &[String],
    stdin_input: &str,
) -> anyhow::Result<(std::process::Output, bool)> {
    use std::io::Write;
    let prep = crate::security::sandbox::prepare_command(
        crate::security::sandbox::effective_mode(),
        workspace,
        program,
        args,
    );
    let attempt =
        |program: &str, args: &[String], scrub: bool| -> anyhow::Result<std::process::Output> {
            let configure = |cmd: &mut std::process::Command| {
                cmd.stdin(std::process::Stdio::piped());
                cmd.stdout(std::process::Stdio::piped());
                cmd.stderr(std::process::Stdio::piped());
            };
            let mut cmd = build_sandboxed_base_cmd(workspace, program, args, scrub, &configure);
            let mut child = cmd.spawn()?;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(stdin_input.as_bytes())?;
            }
            child.wait_with_output().map_err(Into::into)
        };
    let first = match attempt(&prep.program, &prep.args, prep.scrub_env) {
        Ok(out) => out,
        Err(_) if prep.applied => {
            crate::security::sandbox::record_sandbox_degraded();
            tracing::warn!(
                mode = prep.mode.as_str(),
                "sandboxed command spawn failed — falling back to direct execution"
            );
            return Ok((attempt(program, args, prep.scrub_env)?, false));
        }
        Err(e) => return Err(e),
    };
    if prep.applied
        && crate::security::sandbox::is_bwrap_setup_failure(
            first.status.code(),
            &String::from_utf8_lossy(&first.stderr),
        )
    {
        crate::security::sandbox::record_sandbox_degraded();
        tracing::warn!(
            mode = prep.mode.as_str(),
            "sandboxed command failed at setup (bwrap error) — retrying without sandbox"
        );
        return Ok((attempt(program, args, prep.scrub_env)?, false));
    }
    Ok((first, prep.applied))
}

/// Tokio variant of [`run_sandboxed_stdin_output`].
pub async fn run_sandboxed_stdin_output_async(
    workspace: &std::path::Path,
    program: &str,
    args: &[String],
    stdin_input: &str,
) -> anyhow::Result<(std::process::Output, bool)> {
    use tokio::io::AsyncWriteExt;
    let prep = crate::security::sandbox::prepare_command(
        crate::security::sandbox::effective_mode(),
        workspace,
        program,
        args,
    );
    let attempt = |program: &str, args: &[String], scrub: bool| {
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args);
        cmd.current_dir(workspace);
        if scrub {
            cmd.env_clear();
            let passthrough = crate::security::sandbox::sandbox_config()
                .map(|c| c.passthrough_env)
                .unwrap_or_default();
            for (key, val) in crate::security::sandbox::sanitized_env(&passthrough) {
                cmd.env(key, val);
            }
        }
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        async move {
            let mut child = cmd.spawn()?;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(stdin_input.as_bytes()).await?;
            }
            child.wait_with_output().await
        }
    };
    let first = match attempt(&prep.program, &prep.args, prep.scrub_env).await {
        Ok(out) => out,
        Err(_) if prep.applied => {
            crate::security::sandbox::record_sandbox_degraded();
            tracing::warn!(
                mode = prep.mode.as_str(),
                "sandboxed command spawn failed — falling back to direct execution"
            );
            return Ok((attempt(program, args, prep.scrub_env).await?, false));
        }
        Err(e) => return Err(e.into()),
    };
    if prep.applied
        && crate::security::sandbox::is_bwrap_setup_failure(
            first.status.code(),
            &String::from_utf8_lossy(&first.stderr),
        )
    {
        crate::security::sandbox::record_sandbox_degraded();
        tracing::warn!(
            mode = prep.mode.as_str(),
            "sandboxed command failed at setup (bwrap error) — retrying without sandbox"
        );
        return Ok((attempt(program, args, prep.scrub_env).await?, false));
    }
    Ok((first, prep.applied))
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

    #[test]
    fn read_text_capped_decodes_lossily() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("mixed.txt");
        std::fs::write(&path, b"hello \xff world").unwrap();
        let text = read_text_capped(&path, 1024).unwrap();
        assert!(text.starts_with("hello "), "lossy decode, got: {text}");
    }

    #[test]
    fn run_command_capped_keeps_small_output() {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg("printf 'hello-world'");
        let out = run_command_capped(&mut cmd, 1024).expect("run");
        assert_eq!(out.status, Some(0));
        assert!(!out.stdout_truncated);
        assert_eq!(out.stdout_lossy(), "hello-world");
        assert!(!out.stderr_truncated);
    }

    #[test]
    fn run_command_capped_truncates_oversized_stdout() {
        // 100 KiB of output under a 1 KiB cap: the cap must hold and the
        // truncation must be reported explicitly (never silent).
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg("printf 'x%.0s' $(seq 1 100000)");
        let out = run_command_capped(&mut cmd, 1024).expect("run");
        assert_eq!(out.status, Some(0));
        assert!(out.stdout_truncated, "oversized stdout must be flagged");
        assert_eq!(out.stdout.len(), 1024, "output must be capped");
    }

    #[test]
    fn run_command_capped_reports_stdout_and_stderr_independently() {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c")
            .arg("printf 'err%.0s' $(seq 1 100000) >&2; echo ok");
        let out = run_command_capped(&mut cmd, 1024).expect("run");
        assert_eq!(out.stdout_lossy(), "ok\n");
        assert!(!out.stdout_truncated);
        assert!(out.stderr_truncated, "oversized stderr must be flagged");
        assert_eq!(out.stderr.len(), 1024);
    }
}
