//! Git tool

use crate::governance::pua::tool_execution_report;
use crate::i18n::runtime::{t, tf};
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
use anyhow::Result;
use std::process::Command;
use tracing::debug;

const ALLOWED_GIT_SUBCOMMANDS: &[&str] = &["status", "log", "diff", "show", "stash"];

pub struct GitTool;

impl Tool for GitTool {
    fn name(&self) -> &'static str {
        "git"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let subcommand = input.payload["subcommand"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_subcommand")))?;

        if !ALLOWED_GIT_SUBCOMMANDS.contains(&subcommand) {
            anyhow::bail!(
                "{}",
                tf("error.command_not_allowed", &[("command", subcommand)])
            );
        }

        let directory = input.payload["directory"].as_str().unwrap_or(".");
        let args = input.payload["args"]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(|text| text.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        // Validate arguments to prevent injection
        for arg in &args {
            if !arg
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '/')
            {
                anyhow::bail!("{}", tf("error.invalid_git_argument", &[("arg", arg)]));
            }
        }

        let current_dir = sanitize_path(input, directory)?;

        let mut command = Command::new("git");
        command.arg(subcommand).current_dir(&current_dir);

        // Add --no-pager for read-only commands to prevent hanging
        match subcommand {
            "log" | "diff" | "show" => {
                command.arg("--no-pager");
            }
            _ => {}
        }

        if !args.is_empty() {
            command.args(&args);
        }

        debug!(subcommand = %subcommand, args = ?args, directory = %directory, "tool: running git command");

        let output = command.output()?;
        let success = output.status.success();
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        Ok(ToolOutput {
            success,
            result: Some(serde_json::json!({
                "stdout": stdout,
                "stderr": stderr,
                "exit_code": output.status.code(),
                "subcommand": subcommand,
            })),
            error: (!success).then(|| stderr.trim().to_string()),
            verification: Some("git_command_executed".to_string()),
            audit_log: Some(format!("git {} executed in '{}'", subcommand, directory)),
            pua_report: Some(tool_execution_report("git", Some("git_command_executed"))),
        })
    }
}
