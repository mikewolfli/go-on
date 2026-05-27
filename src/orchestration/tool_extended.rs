//! Extended built-in tools for go-on
//!
//! Additional tool implementations beyond the original 6, providing
//! shell execution, HTTP requests, file operations, and cargo integration.

use crate::governance::pua::tool_execution_report;
use crate::i18n::runtime::{t, tf};
use crate::orchestration::tool::{
    sanitize_path, sanitize_path_for_write, Tool, ToolInput, ToolOutput,
};
use anyhow::{Context, Result};
use glob::Pattern;
use regex::Regex;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use tracing::{debug, info, warn};

// ── ShellExecTool ──────────────────────────────────────────────────────────

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

        let timeout_available = Command::new("timeout").arg("--version").output().is_ok();

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

// ── HttpRequestTool ────────────────────────────────────────────────────────

pub struct HttpRequestTool;

impl Tool for HttpRequestTool {
    fn name(&self) -> &'static str {
        "http_request"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let url = input.payload["url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_url")))?;
        let method = input.payload["method"].as_str().unwrap_or("GET");
        let body = input.payload["body"].as_str();
        let timeout_ms = input.payload["timeout_ms"].as_u64().unwrap_or(15_000);

        debug!(method = %method, url = %url, "tool: making HTTP request");

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .context("failed to build HTTP client")?;

        let request_builder = match method.to_uppercase().as_str() {
            "GET" => client.get(url),
            "POST" => {
                let mut builder = client.post(url);
                if let Some(body_text) = body {
                    builder = builder.body(body_text.to_string());
                }
                builder
            }
            other => {
                anyhow::bail!(
                    "{}",
                    tf("error.unsupported_http_method", &[("method", other)])
                );
            }
        };

        let response = request_builder.send().context("HTTP request failed")?;
        let status = response.status().as_u16();
        let response_body = response
            .text()
            .unwrap_or_else(|_| "(body read failed)".to_string());
        let success = (200..400).contains(&status);

        Ok(ToolOutput {
            success,
            result: Some(serde_json::json!({
                "status": status,
                "body": response_body,
                "url": url,
                "method": method,
            })),
            error: (!success).then(|| format!("HTTP status {}", status)),
            verification: Some("http_request_completed".to_string()),
            audit_log: Some(format!("HTTP {} {} -> {}", method, url, status)),
            pua_report: Some(tool_execution_report(
                "http_request",
                Some("http_request_completed"),
            )),
        })
    }
}

// ── GrepTool ───────────────────────────────────────────────────────────────

pub struct GrepTool;

struct GrepCollectState<'a> {
    matches: &'a mut Vec<serde_json::Value>,
    files_scanned: &'a mut u64,
    total_matches: &'a mut u64,
    max_matches: u64,
}

impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "grep"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let pattern = input.payload["pattern"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_pattern")))?;
        let directory = input.payload["directory"].as_str().unwrap_or(".");
        let include_pattern = input.payload["include"].as_str();
        let case_sensitive = input.payload["case_sensitive"].as_bool().unwrap_or(false);

        let regex = if case_sensitive {
            Regex::new(pattern).context("invalid regex pattern")?
        } else {
            Regex::new(&format!("(?i){}", pattern)).context("invalid regex pattern")?
        };

        let root = sanitize_path(input, directory)?;
        let glob_matcher = include_pattern.and_then(|p| Pattern::new(p).ok());

        let mut matches: Vec<serde_json::Value> = Vec::new();
        let mut files_scanned = 0u64;
        let mut total_matches = 0u64;
        let max_matches = 1000u64;

        let mut state = GrepCollectState {
            matches: &mut matches,
            files_scanned: &mut files_scanned,
            total_matches: &mut total_matches,
            max_matches,
        };

        collect_grep_matches(&root, &root, &regex, &glob_matcher, &mut state)?;

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "matches": matches,
                "files_scanned": files_scanned,
                "total_matches": total_matches,
                "truncated": total_matches >= max_matches,
            })),
            error: None,
            verification: Some("grep_completed".to_string()),
            audit_log: Some(format!(
                "Grep '{}' in '{}': {} matches in {} files",
                pattern, directory, total_matches, files_scanned
            )),
            pua_report: Some(tool_execution_report("grep", Some("grep_completed"))),
        })
    }
}

fn collect_grep_matches(
    root: &Path,
    current: &Path,
    regex: &Regex,
    glob_matcher: &Option<Pattern>,
    state: &mut GrepCollectState<'_>,
) -> Result<()> {
    if *state.total_matches >= state.max_matches {
        return Ok(());
    }

    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            // Skip common non-source directories
            let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if dir_name == ".git" || dir_name == "target" || dir_name == "node_modules" {
                continue;
            }
            collect_grep_matches(root, &path, regex, glob_matcher, state)?;
            continue;
        }

        // Apply glob filter if provided
        if let Some(ref matcher) = glob_matcher {
            let relative = path.strip_prefix(root).unwrap_or(&path);
            if !matcher.matches_path(relative) {
                continue;
            }
        }

        *state.files_scanned += 1;

        // Try to read file as UTF-8 text
        if let Ok(content) = fs::read_to_string(&path) {
            for (line_num, line) in content.lines().enumerate() {
                if *state.total_matches >= state.max_matches {
                    break;
                }
                if regex.is_match(line) {
                    *state.total_matches += 1;
                    let relative = path.strip_prefix(root).unwrap_or(&path);
                    state.matches.push(serde_json::json!({
                        "file": relative.to_string_lossy(),
                        "line": line_num + 1,
                        "content": line,
                    }));
                }
            }
        }
    }
    Ok(())
}

// ── FindFilesTool ──────────────────────────────────────────────────────────

pub struct FindFilesTool;

impl Tool for FindFilesTool {
    fn name(&self) -> &'static str {
        "find_files"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let pattern = input.payload["pattern"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_pattern")))?;
        let directory = input.payload["directory"].as_str().unwrap_or(".");
        let max_results = input.payload["max_results"].as_u64().unwrap_or(500);

        let root = sanitize_path(input, directory)?;
        let matcher = Pattern::new(pattern).context("invalid glob pattern")?;

        let mut files: Vec<String> = Vec::new();
        collect_matching_files_bounded(&root, &root, &matcher, &mut files, max_results as usize)?;

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "files": files,
                "count": files.len(),
                "truncated": files.len() as u64 >= max_results,
            })),
            error: None,
            verification: Some("find_files_completed".to_string()),
            audit_log: Some(format!(
                "Find files '{}' in '{}': {} results",
                pattern,
                directory,
                files.len()
            )),
            pua_report: Some(tool_execution_report(
                "find_files",
                Some("find_files_completed"),
            )),
        })
    }
}

fn collect_matching_files_bounded(
    root: &Path,
    current: &Path,
    matcher: &Pattern,
    files: &mut Vec<String>,
    max_results: usize,
) -> Result<()> {
    if files.len() >= max_results {
        return Ok(());
    }

    for entry in fs::read_dir(current)? {
        if files.len() >= max_results {
            break;
        }
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if dir_name == ".git" || dir_name == "target" || dir_name == "node_modules" {
                continue;
            }
            collect_matching_files_bounded(root, &path, matcher, files, max_results)?;
            continue;
        }

        let relative = path.strip_prefix(root).unwrap_or(&path);
        let candidate = relative.to_string_lossy().replace('\\', "/");
        if matcher.matches(&candidate) || matcher.matches_path(relative) {
            files.push(path.to_string_lossy().to_string());
        }
    }
    Ok(())
}

// ── GitTool ────────────────────────────────────────────────────────────────

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

// ── ListDirectoryTool ──────────────────────────────────────────────────────

pub struct ListDirectoryTool;

impl Tool for ListDirectoryTool {
    fn name(&self) -> &'static str {
        "list_directory"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;

        let validated = sanitize_path(input, path)?;
        let mut entries: Vec<serde_json::Value> = Vec::new();

        for entry in fs::read_dir(&validated).context("failed to read directory")? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type().ok();
            let metadata = entry.metadata().ok();

            let entry_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();

            let is_dir = file_type.as_ref().map(|ft| ft.is_dir()).unwrap_or(false);

            let size = metadata
                .as_ref()
                .and_then(|m| if m.is_file() { Some(m.len()) } else { None });

            entries.push(serde_json::json!({
                "name": entry_name,
                "is_directory": is_dir,
                "size_bytes": size,
            }));
        }

        // Sort: directories first, then alphabetical
        entries.sort_by(|a, b| {
            let a_dir = a["is_directory"].as_bool().unwrap_or(false);
            let b_dir = b["is_directory"].as_bool().unwrap_or(false);
            b_dir
                .cmp(&a_dir)
                .then_with(|| a["name"].as_str().cmp(&b["name"].as_str()))
        });

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "entries": entries,
                "count": entries.len(),
                "path": validated.to_string_lossy(),
            })),
            error: None,
            verification: Some("directory_listed".to_string()),
            audit_log: Some(format!(
                "Listed directory: {} ({} entries)",
                validated.display(),
                entries.len()
            )),
            pua_report: Some(tool_execution_report(
                "list_directory",
                Some("directory_listed"),
            )),
        })
    }
}

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

// ── CargoTestTool ──────────────────────────────────────────────────────────

pub struct CargoTestTool;

impl Tool for CargoTestTool {
    fn name(&self) -> &'static str {
        "cargo_test"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let directory = input.payload["directory"].as_str().unwrap_or(".");
        let filter = input.payload["filter"].as_str();
        let current_dir = sanitize_path(input, directory)?;

        let mut command = Command::new("cargo");
        command.arg("test").current_dir(&current_dir);

        // Add test name filter if provided
        if let Some(test_filter) = filter {
            if !test_filter
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == ':' || c == '-')
            {
                anyhow::bail!(
                    "{}",
                    tf("error.invalid_test_filter", &[("filter", test_filter)])
                );
            }
            command.arg(test_filter);
        }

        debug!(filter = ?filter, directory = %directory, "tool: running cargo test");

        let output = command.output().context("failed to run cargo test")?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let success = output.status.success();

        Ok(ToolOutput {
            success,
            result: Some(serde_json::json!({
                "stdout": stdout,
                "stderr": stderr,
                "exit_code": output.status.code(),
                "filter": filter,
            })),
            error: (!success).then(|| {
                let summary = stderr.lines().last().unwrap_or("unknown error");
                summary.to_string()
            }),
            verification: Some("cargo_test_completed".to_string()),
            audit_log: Some(format!(
                "cargo test executed in '{}' (success: {})",
                directory, success
            )),
            pua_report: Some(tool_execution_report(
                "cargo_test",
                Some("cargo_test_completed"),
            )),
        })
    }
}

// ── FileMoveTool ───────────────────────────────────────────────────────────

pub struct FileMoveTool;

impl Tool for FileMoveTool {
    fn name(&self) -> &'static str {
        "file_move"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let source = input.payload["source"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_source_path")))?;
        let destination = input.payload["destination"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_destination_path")))?;

        let source_path = sanitize_path(input, source)?;
        let dest_path = sanitize_path_for_write(input, destination)?;

        if !source_path.exists() {
            anyhow::bail!("{}", tf("error.source_not_found", &[("source", source)]));
        }

        // Create parent directories if they don't exist
        if let Some(parent) = dest_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .context("failed to create destination parent directories")?;
            }
        }

        fs::rename(&source_path, &dest_path).context("failed to move file")?;

        info!(
            source = %source_path.display(),
            dest = %dest_path.display(),
            "tool: file moved successfully"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "source": source_path.to_string_lossy(),
                "destination": dest_path.to_string_lossy(),
            })),
            error: None,
            verification: Some("file_moved".to_string()),
            audit_log: Some(format!(
                "Moved '{}' -> '{}'",
                source_path.display(),
                dest_path.display()
            )),
            pua_report: Some(tool_execution_report("file_move", Some("file_moved"))),
        })
    }
}

// ── FileDeleteTool ─────────────────────────────────────────────────────────

pub struct FileDeleteTool;

impl Tool for FileDeleteTool {
    fn name(&self) -> &'static str {
        "file_delete"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;
        let confirm = input.payload["confirm"].as_bool().unwrap_or(false);

        if !confirm {
            anyhow::bail!("{}", t("error.delete_not_confirmed"));
        }

        let validated = sanitize_path(input, path)?;

        if !validated.exists() {
            anyhow::bail!("{}", tf("error.path_not_found", &[("path", path)]));
        }

        let is_dir = validated.is_dir();

        if is_dir {
            fs::remove_dir_all(&validated).context("failed to delete directory")?;
        } else {
            fs::remove_file(&validated).context("failed to delete file")?;
        }

        warn!(
            path = %validated.display(),
            is_dir = %is_dir,
            "tool: file/directory deleted"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "deleted_path": validated.to_string_lossy(),
                "is_directory": is_dir,
            })),
            error: None,
            verification: Some("file_deleted".to_string()),
            audit_log: Some(format!(
                "Deleted {} '{}'",
                if is_dir { "directory" } else { "file" },
                validated.display()
            )),
            pua_report: Some(tool_execution_report("file_delete", Some("file_deleted"))),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::tool::ToolInput;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-1".to_string(),
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
    fn shell_exec_runs_echo() {
        let input = tool_input(serde_json::json!({
            "command": "echo hello",
            "timeout_ms": 5000,
        }));
        let tool = ShellExecTool;
        let output = tool.run(&input).expect("shell_exec should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        assert!(result["stdout"].as_str().unwrap().contains("hello"));
    }

    #[test]
    fn find_files_finds_rs_files() {
        let tmp = TempDir::new().expect("temp dir");
        std::fs::write(tmp.path().join("a.rs"), "fn main() {}").unwrap();
        std::fs::write(tmp.path().join("b.txt"), "text").unwrap();

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "find".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "pattern": "*.rs",
                "directory": tmp.path().to_string_lossy(),
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = FindFilesTool;
        let output = tool.run(&input).expect("find_files should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        let files = result["files"].as_array().unwrap();
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn list_directory_lists_entries() {
        let tmp = TempDir::new().expect("temp dir");
        std::fs::write(tmp.path().join("hello.txt"), "world").unwrap();
        std::fs::create_dir(tmp.path().join("subdir")).unwrap();

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "list".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "path": tmp.path().to_string_lossy(),
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = ListDirectoryTool;
        let output = tool.run(&input).expect("list_directory should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        let entries = result["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn file_move_renames_file() {
        let tmp = TempDir::new().expect("temp dir");
        let src = tmp.path().join("old.txt");
        let dst = tmp.path().join("new.txt");
        std::fs::write(&src, "content").unwrap();

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "move".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "source": src.to_string_lossy(),
                "destination": dst.to_string_lossy(),
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = FileMoveTool;
        let output = tool.run(&input).expect("file_move should succeed");
        assert!(output.success);
        assert!(!src.exists());
        assert!(dst.exists());
    }

    #[test]
    fn file_delete_requires_confirmation() {
        let tmp = TempDir::new().expect("temp dir");
        let f = tmp.path().join("delete_me.txt");
        std::fs::write(&f, "bye").unwrap();

        let input_no_confirm = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "delete".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "path": f.to_string_lossy(),
                "confirm": false,
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = FileDeleteTool;
        let output = tool.run(&input_no_confirm);
        assert!(output.is_err(), "file_delete without confirm should error");
        assert!(f.exists());

        let input_confirm = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "delete".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "path": f.to_string_lossy(),
                "confirm": true,
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let output2 = tool
            .run(&input_confirm)
            .expect("file_delete with confirm should succeed");
        assert!(output2.success);
        assert!(!f.exists());
    }

    #[test]
    fn cargo_check_in_workspace() {
        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "check".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "directory": ".",
            }),
            allowed_base_dir: Some(PathBuf::from(".")),
        };

        let tool = CargoCheckTool;
        let output = tool.run(&input).expect("cargo_check should run");
        // Should succeed in this workspace
        assert!(output.success);
    }

    #[test]
    fn git_status_runs() {
        let input = tool_input(serde_json::json!({
            "subcommand": "status",
            "directory": ".",
        }));
        let tool = GitTool;
        let output = tool.run(&input).expect("git status should run");
        assert!(output.success);
    }

    #[test]
    fn cargo_test_runs() {
        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "tester".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "directory": ".",
                "filter": "shell_exec_runs",
            }),
            allowed_base_dir: Some(PathBuf::from(".")),
        };

        let tool = CargoTestTool;
        let output = tool.run(&input).expect("cargo_test should run");
        // May or may not find the test
        assert!(output.result.is_some());
    }
}
