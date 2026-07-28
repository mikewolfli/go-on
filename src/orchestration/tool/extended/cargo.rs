//! Cargo integration tools (cargo_check, cargo_test)

use crate::governance::pua::tool_execution_report;

use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
use anyhow::{Context, Result};
use std::process::Command;
use tracing::debug;

// ── CargoCheckTool ─────────────────────────────────────────────────────────

pub struct CargoCheckTool;

impl Tool for CargoCheckTool {
    fn name(&self) -> &'static str {
        "cargo_check"
    }
    fn description(&self) -> &str {
        "Run cargo check on a Rust project directory"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let directory = input.payload["directory"].as_str().unwrap_or(".");
        let current_dir = sanitize_path(input, directory)?;

        debug!(directory = %directory, "tool: running cargo check");

        let output = Command::new("cargo")
            .arg("check")
            .arg("--message-format=json")
            .current_dir(&current_dir)
            .output()
            .context("failed to run cargo check")?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let success = output.status.success();

        // Parse JSON diagnostic messages from cargo output
        let mut errors: Vec<serde_json::Value> = Vec::new();
        let mut warnings: Vec<serde_json::Value> = Vec::new();

        for line in stdout.lines() {
            if let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) {
                let reason = msg["reason"].as_str().unwrap_or("");
                match reason {
                    "compiler-message" => {
                        let message = &msg["message"];
                        let level = message["level"].as_str().unwrap_or("");
                        let rendered = message["rendered"].as_str().unwrap_or("");
                        let spans = &message["spans"];
                        let entry = serde_json::json!({
                            "level": level,
                            "message": message["message"],
                            "rendered": rendered,
                            "spans": spans,
                        });
                        if level == "error" {
                            errors.push(entry);
                        } else if level == "warning" {
                            warnings.push(entry);
                        }
                    }
                    "compiler-artifact" => {
                        // Skip artifact messages
                    }
                    _ => {}
                }
            }
        }

        Ok(ToolOutput {
            success,
            result: Some(serde_json::json!({
                "errors": errors,
                "error_count": errors.len(),
                "warnings": warnings,
                "warning_count": warnings.len(),
                "raw_stderr": stderr,
                "exit_code": output.status.code(),
            })),
            error: (!success).then(|| {
                format!(
                    "cargo check failed with {} errors, {} warnings",
                    errors.len(),
                    warnings.len()
                )
            }),
            verification: Some("cargo_check_completed".to_string()),
            audit_log: Some(format!(
                "cargo check executed in '{}': {} errors, {} warnings",
                directory,
                errors.len(),
                warnings.len()
            )),
            pua_report: Some(tool_execution_report(
                "cargo_check",
                Some("cargo_check_completed"),
            )),
        })
    }
}
