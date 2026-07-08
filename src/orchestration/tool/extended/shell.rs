//! Shell execution tool

use crate::governance::pua::tool_execution_report;
use crate::i18n::runtime::t;
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
use anyhow::Result;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;
use tracing::{debug, info, warn};

pub struct ShellExecTool;

impl Tool for ShellExecTool {
    fn name(&self) -> &'static str {
        "shell_exec"
    }
    fn description(&self) -> &str {
        "Execute a shell command with timeout and capture output"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let command = input.payload["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_command")))?;
        let timeout_ms = input.payload["timeout_ms"].as_u64().unwrap_or(30_000);
        let directory = input.payload["directory"].as_str().unwrap_or(".");

        // ── LAYER 2: Runtime sandbox ────────────────────────────────────
        // Block dangerous commands that could harm the system.
        let command_lower = command.to_lowercase();
        let blocked_patterns = [
            "rm -rf /",
            "rm -rf /*",
            "mkfs.",
            "dd if=",
            "format ",
            ":(){",
            "fork bomb",
            "chmod -R 000",
            "> /dev/sda",
            "> /dev/hda",
            "| shutdown",
            "| reboot",
            "wget http://",
            "curl http://",
            "nmap ",
            "hydra ",
        ];
        for pattern in &blocked_patterns {
            if command_lower.contains(pattern) {
                warn!(
                    "shell_exec BLOCKED: command matches blocked pattern '{}' — cmd={}",
                    pattern, command
                );
                return Ok(ToolOutput {
                    success: false,
                    result: None,
                    error: Some(format!(
                        "Command blocked by security policy: contains '{}'",
                        pattern
                    )),
                    verification: Some("shell_sandbox_blocked".to_string()),
                    audit_log: Some(format!(
                        "BLOCKED shell exec (pattern '{}'): {}",
                        pattern, command
                    )),
                    pua_report: Some(tool_execution_report(
                        "shell_exec",
                        Some("shell_sandbox_blocked"),
                    )),
                });
            }
        }

        // Limit output size to prevent memory exhaustion (default 10MB)
        const MAX_OUTPUT_BYTES: usize = 10 * 1024 * 1024;

        debug!(command = %command, timeout_ms = %timeout_ms, directory = %directory, "tool: executing shell command");

        let current_dir = sanitize_path(input, directory)?;

        // Environment variables from payload["env"] as a JSON object
        let env_vars: Vec<(String, String)> = input.payload["env"]
            .as_object()
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|val_str| (k.clone(), val_str.to_string())))
                    .collect()
            })
            .unwrap_or_default();

        // stdin input from payload["stdin"] as a string
        let stdin_input = input.payload["stdin"].as_str().map(|s| s.to_string());

        // Determine the shell to use: cmd.exe on Windows, sh on Unix.
        let (shell, shell_arg) = if cfg!(target_os = "windows") {
            ("cmd.exe", "/C")
        } else {
            // Prefer GNU `timeout` when available, but keep a portable fallback for
            // environments like macOS where `timeout` is not installed by default.
            // On Windows, timeout.exe is a different tool, so we use the Rust-level fallback.
            ("sh", "-c")
        };

        let timeout_secs = (timeout_ms as f64 / 1000.0).ceil() as u64;
        let max_timeout = std::cmp::min(timeout_secs, 300); // Cap at 5 minutes

        // Only check for GNU timeout on non-Windows. On Windows, always use
        // thread-based kill approach.
        let use_gnu_timeout = if cfg!(target_os = "windows") {
            false
        } else {
            Command::new("timeout")
                .arg("1")
                .arg("sh")
                .arg("-c")
                .arg("true")
                .output()
                .map(|out| out.status.success())
                .unwrap_or(false)
        };

        let output = if use_gnu_timeout {
            let mut cmd = Command::new("timeout");
            cmd.arg(format!("{}", max_timeout))
                .arg(shell)
                .arg(shell_arg)
                .arg(command)
                .current_dir(&current_dir)
                .stdin(if stdin_input.is_some() {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            // Apply environment variables
            for (key, val) in &env_vars {
                cmd.env(key, val);
            }

            let mut child = cmd.spawn()?;

            // Write stdin if provided
            if let Some(stdin_text) = &stdin_input {
                if let Some(mut stdin_writer) = child.stdin.take() {
                    let _ = stdin_writer.write_all(stdin_text.as_bytes());
                }
            }

            child.wait_with_output()
        } else {
            // Rust-level timeout fallback: spawn the child process, then use a
            // separate thread to enforce the timeout by killing the process.
            let mut cmd = Command::new(shell);
            cmd.arg(shell_arg)
                .arg(command)
                .current_dir(&current_dir)
                .stdin(if stdin_input.is_some() {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            // Apply environment variables before spawning
            for (key, val) in &env_vars {
                cmd.env(key, val);
            }

            let mut child = cmd.spawn()?;

            // Write stdin if provided
            if let Some(stdin_text) = &stdin_input {
                if let Some(mut stdin_writer) = child.stdin.take() {
                    let _ = stdin_writer.write_all(stdin_text.as_bytes());
                }
            }

            let kill_after = Duration::from_millis(timeout_ms);
            let pid = child.id();
            let killed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let killed_clone = killed.clone();

            let handle = std::thread::spawn(move || {
                std::thread::sleep(kill_after);
                killed_clone.store(true, std::sync::atomic::Ordering::SeqCst);
                if cfg!(target_os = "windows") {
                    // Windows: use taskkill to terminate the process tree
                    let _ = Command::new("taskkill")
                        .arg("/F")
                        .arg("/T")
                        .arg("/PID")
                        .arg(pid.to_string())
                        .output();
                } else {
                    // Unix: send SIGTERM then SIGKILL
                    let _ = Command::new("kill").arg("--").arg(pid.to_string()).output();
                    let _ = Command::new("kill")
                        .arg("-9")
                        .arg("--")
                        .arg(pid.to_string())
                        .output();
                }
            });

            let result = child.wait_with_output();

            // Ensure the kill thread has finished
            let _ = handle.join();

            if killed.load(std::sync::atomic::Ordering::SeqCst) {
                // Timeout was triggered
                let (timeout_stdout, timeout_stderr) = match result {
                    Ok(out) => (out.stdout, out.stderr),
                    Err(_) => (Vec::new(), Vec::new()),
                };
                let stdout = String::from_utf8_lossy(&timeout_stdout).to_string();
                let stderr = String::from_utf8_lossy(&timeout_stderr).to_string();
                warn!(
                    command = %command,
                    timeout_ms = %timeout_ms,
                    "tool: shell command timed out"
                );
                return Ok(ToolOutput {
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
                        "Shell exec '{}' in '{}' timed out after {}ms",
                        command, directory, timeout_ms
                    )),
                    pua_report: Some(tool_execution_report(
                        "shell_exec",
                        Some("shell_command_executed"),
                    )),
                });
            }

            result
        };

        match output {
            Ok(output) => {
                let success = output.status.success();
                let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let exit_code = output.status.code();

                // ── LAYER 2: Output size limit ──────────────────────────────────
                if stdout.len() > MAX_OUTPUT_BYTES {
                    warn!(
                        "shell_exec TRUNCATED: stdout {} bytes > {} max",
                        stdout.len(),
                        MAX_OUTPUT_BYTES
                    );
                    // Truncate rather than fail - partial output is better than none
                    let mut truncated = String::with_capacity(MAX_OUTPUT_BYTES);
                    for ch in stdout.chars().take(MAX_OUTPUT_BYTES) {
                        truncated.push(ch);
                    }
                    stdout = truncated;
                }
                if stderr.len() > MAX_OUTPUT_BYTES {
                    let mut truncated = String::with_capacity(MAX_OUTPUT_BYTES);
                    for ch in stderr.chars().take(MAX_OUTPUT_BYTES) {
                        truncated.push(ch);
                    }
                    stderr = truncated;
                }

                if !success {
                    warn!(
                        command = %command,
                        exit_code = ?exit_code,
                        stderr = %stderr.trim(),
                        "tool: shell command failed"
                    );
                } else {
                    info!(command = %command, exit_code = ?exit_code, "tool: shell command succeeded");
                }

                Ok(ToolOutput {
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
                        "shell_exec",
                        Some("shell_command_executed"),
                    )),
                })
            }
            Err(e) => {
                warn!(command = %command, error = %e, "tool: shell command spawn failed");
                Ok(ToolOutput {
                    success: false,
                    result: None,
                    error: Some(format!("{}", e)),
                    verification: None,
                    audit_log: Some(format!("Shell exec failed: {}", e)),
                    pua_report: Some(tool_execution_report("shell_exec", None)),
                })
            }
        }
    }
}
