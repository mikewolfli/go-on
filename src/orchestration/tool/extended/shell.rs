//! Shell execution tool

use crate::governance::pua::tool_execution_report;
use crate::i18n::runtime::t;
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
use anyhow::Result;
use std::process::Command;
use tracing::{debug, info, warn};

pub struct ShellExecTool;

impl Tool for ShellExecTool {
    fn name(&self) -> &'static str {
        "shell_exec"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let command = input.payload["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_command")))?;
        let timeout_ms = input.payload["timeout_ms"].as_u64().unwrap_or(30_000);
        let directory = input.payload["directory"].as_str().unwrap_or(".");

        debug!(command = %command, timeout_ms = %timeout_ms, directory = %directory, "tool: executing shell command");

        let current_dir = sanitize_path(input, directory)?;

        // Prefer GNU `timeout` when available, but keep a portable fallback for
        // environments like macOS where `timeout` is not installed by default.
        let timeout_secs = (timeout_ms as f64 / 1000.0).ceil() as u64;
        let max_timeout = std::cmp::min(timeout_secs, 300); // Cap at 5 minutes

        let timeout_available = Command::new("timeout")
            .arg("1")
            .arg("sh")
            .arg("-c")
            .arg("true")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false);

        let output = if timeout_available {
            Command::new("timeout")
                .arg(format!("{}", max_timeout))
                .arg("sh")
                .arg("-c")
                .arg(command)
                .current_dir(&current_dir)
                .output()
        } else {
            Command::new("sh")
                .arg("-c")
                .arg(command)
                .current_dir(&current_dir)
                .output()
        };

        match output {
            Ok(output) => {
                let success = output.status.success();
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let exit_code = output.status.code();

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
