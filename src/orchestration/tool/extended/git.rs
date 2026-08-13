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
    fn description(&self) -> &str {
        // Keep in sync with ALLOWED_GIT_SUBCOMMANDS below: the whitelist is
        // deliberately read-only, so the description must not promise
        // operations the implementation rejects.
        "Execute safe, read-only git operations (status, diff, log, show, stash)"
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

        // `stash` is write-capable (pop/apply/drop/clear mutate the stash),
        // so only its read-only subcommands are permitted: `list` and `show`.
        // Other subcommands take no state-mutating arguments after the
        // subcommand itself (status/log/diff/show are read-only).
        if subcommand == "stash" {
            // `stash list`/`stash show` are the only read-only stash ops.
            let read_only = matches!(
                args.first().map(String::as_str),
                None | Some("list") | Some("show")
            );
            if !read_only {
                anyhow::bail!(
                    "{}",
                    tf(
                        "error.command_not_allowed",
                        &[(
                            "command",
                            &format!("stash {}", args.first().map(String::as_str).unwrap_or(""))
                        )]
                    )
                );
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

        // Capped execution: `Command::output()` would buffer the full output
        // of `git diff`/`git log` on a huge repo (OOM) and push it unclipped
        // into the LLM context. Truncation is reported explicitly.
        let capped = crate::orchestration::tool::exec_common::run_command_capped(
            &mut command,
            crate::orchestration::tool::exec_common::MAX_OUTPUT_BYTES,
        )?;
        let success = capped.status == Some(0);
        let stdout = capped.stdout_lossy();
        let stderr = capped.stderr_lossy();
        if capped.stdout_truncated || capped.stderr_truncated {
            tracing::warn!(
                "git {subcommand}: output truncated at {} bytes (stdout={}, stderr={})",
                crate::orchestration::tool::exec_common::MAX_OUTPUT_BYTES,
                capped.stdout_truncated,
                capped.stderr_truncated
            );
        }

        Ok(ToolOutput {
            success,
            result: Some(serde_json::json!({
                "stdout": stdout,
                "stderr": stderr,
                "exit_code": capped.status,
                "subcommand": subcommand,
            })),
            error: (!success).then(|| stderr.trim().to_string()),
            verification: Some("git_command_executed".to_string()),
            audit_log: Some(format!("git {} executed in '{}'", subcommand, directory)),
            pua_report: Some(tool_execution_report("git", Some("git_command_executed"))),
        })
    }
}
