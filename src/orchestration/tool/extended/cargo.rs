//! Cargo integration tools (cargo_check, cargo_test)

use crate::governance::pua::tool_execution_report;

use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
use anyhow::{Context, Result};
use tracing::debug;

// ── CargoCheckTool ─────────────────────────────────────────────────────────

pub struct CargoCheckTool;

impl Tool for CargoCheckTool {
    fn name(&self) -> &'static str {
        "cargo_check"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let directory = input.payload["directory"].as_str().unwrap_or(".");
        let current_dir = sanitize_path(input, directory)?;

        debug!(directory = %directory, "tool: running cargo check");

        // Capped execution: `cargo check` diagnostics on a big workspace can
        // be hundreds of MB — `Command::output()` would buffer it all (OOM)
        // and push it unclipped into the LLM context. The command also runs
        // inside the OS sandbox (cargo executes arbitrary build scripts).
        let args = vec!["check".to_string(), "--message-format=json".to_string()];
        let capped = crate::orchestration::tool::exec_common::run_sandboxed_capped(
            &current_dir,
            "cargo",
            &args,
            crate::orchestration::tool::exec_common::MAX_OUTPUT_BYTES,
            |_| {},
        )
        .context("failed to run cargo check")?;

        let stdout = capped.stdout_lossy();
        let stderr = capped.stderr_lossy();
        let success = capped.status == Some(0);
        if capped.stdout_truncated || capped.stderr_truncated {
            tracing::warn!(
                "cargo check: output truncated at {} bytes (stdout={}, stderr={})",
                crate::orchestration::tool::exec_common::MAX_OUTPUT_BYTES,
                capped.stdout_truncated,
                capped.stderr_truncated
            );
        }

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
                "exit_code": capped.status,
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
