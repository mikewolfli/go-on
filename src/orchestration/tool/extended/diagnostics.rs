//! Diagnostics tool
//!
//! Provides project diagnostics information by running `cargo check`
//! and reporting warnings/errors in a structured format.

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
use anyhow::{Context, Result};
use tracing::debug;

/// Tool that runs `cargo check` and returns structured diagnostics.
///
/// Accepts an optional `directory` parameter to run in a specific subdirectory.
/// Returns the raw compiler output, warning/error counts, and a summary.
pub struct DiagnosticsTool;

impl Tool for DiagnosticsTool {
    fn name(&self) -> &'static str {
        "diagnostics"
    }
    fn description(&self) -> &str {
        "Check project diagnostics (errors, warnings) for a directory"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let directory = input
            .payload
            .get("directory")
            .and_then(|v| v.as_str())
            .unwrap_or(".");

        let current_dir = sanitize_path(input, directory)?;

        debug!(directory = %directory, "tool: diagnostics");

        // Run `cargo check` (a command executor, so it goes through the OS
        // sandbox) and capture stderr (where diagnostics appear).
        let (output, _sandbox_applied) =
            crate::orchestration::tool::exec_common::run_sandboxed_output(
                &current_dir,
                "cargo",
                &["check".to_string(), "--message-format=short".to_string()],
                |_| {},
            )
            .context("failed to execute `cargo check` — is cargo installed?")?;

        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();

        // Count diagnostics by parsing lines with typical patterns.
        let error_count = stderr.lines().filter(|l| l.contains("error")).count();
        let warning_count = stderr.lines().filter(|l| l.contains("warning")).count();

        let success = output.status.success();

        // Collect relevant diagnostic lines (first 100 to avoid huge payloads).
        let diagnostic_lines: Vec<String> =
            stderr.lines().take(100).map(|l| l.to_string()).collect();

        let result = serde_json::json!({
            "success": success,
            "exit_code": output.status.code().unwrap_or(-1),
            "error_count": error_count,
            "warning_count": warning_count,
            "diagnostics": diagnostic_lines,
            "stdout_truncated": stdout.chars().count() > 2000,
            "stderr_truncated": stderr.lines().count() > 100,
        });

        debug!(
            success = %success,
            errors = %error_count,
            warnings = %warning_count,
            "tool: diagnostics complete"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(result),
            error: None,
            verification: Some("diagnostics_checked".to_string()),
            audit_log: Some(format!(
                "diagnostics: {} errors, {} warnings, exit={}",
                error_count,
                warning_count,
                output.status.code().unwrap_or(-1)
            )),
            pua_report: Some(tool_execution_report(
                "diagnostics",
                Some("diagnostics_checked"),
            )),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::tool::ToolInput;
    use std::path::PathBuf;

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-diag".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload,
            allowed_base_dir: None,
        }
    }

    #[test]
    fn diagnostics_runs_in_cargo_manifest_dir() {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let input = tool_input(serde_json::json!({
            "directory": workspace.to_string_lossy(),
        }));
        let tool = DiagnosticsTool;
        let output = tool.run(&input).expect("diagnostics should run");
        assert!(output.success);
        let result = output.result.unwrap();
        // Should always have some output from cargo check
        assert!(result["exit_code"].as_i64().is_some());
    }

    #[test]
    fn diagnostics_rejects_nonexistent_directory() {
        let input = tool_input(serde_json::json!({
            "directory": "/nonexistent-path-12345",
        }));
        let tool = DiagnosticsTool;
        let result = tool.run(&input);
        assert!(
            result.is_err(),
            "diagnostics should fail for nonexistent directory"
        );
    }
}
