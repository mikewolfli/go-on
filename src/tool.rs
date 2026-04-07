//! Tool trait and tool runtime for go-on
//!
//! These structures are intentional framework definitions for Phase 0-9 architecture.
//! Tool trait, registry, and implementations will be connected to the execution flow
//! once orchestration logic integrates them.

#![allow(dead_code)]

use anyhow::Result;
use glob::Pattern;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::pua::{tool_execution_report, PuaExecutionReport};

/// Tool input envelope
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInput {
    pub task_id: String,
    pub phase: String,
    pub agent_role: String,
    pub objective: String,
    pub constraints: Option<String>,
    pub evidence: Option<String>,
    pub payload: serde_json::Value,
}

/// Tool output envelope
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub success: bool,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
    pub verification: Option<String>,
    pub audit_log: Option<String>,
    pub pua_report: Option<PuaExecutionReport>,
}

/// Tool trait
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn run(&self, input: &ToolInput) -> Result<ToolOutput>;
}

/// Tool registry
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        let mut registry = Self { tools: Vec::new() };
        registry.register(ReadFileTool);
        registry.register(WriteFileTool);
        registry.register(SearchFilesTool);
        registry.register(ApplyPatchTool);
        registry.register(RunTestsTool);
        registry.register(InspectGitDiffTool);
        registry
    }
    pub fn register<T: Tool + 'static>(&mut self, tool: T) {
        self.tools.push(Box::new(tool));
    }
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|t| t.name() == name)
            .map(|b| b.as_ref())
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.tools.iter().map(|tool| tool.name()).collect()
    }
}

pub struct ReadFileTool;
impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing path"))?;
        let content = std::fs::read_to_string(path)?;
        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({"content": content})),
            error: None,
            verification: Some("file_read".to_string()),
            audit_log: Some(format!("Read file: {}", path)),
            pua_report: Some(tool_execution_report("read_file", Some("file_read"))),
        })
    }
}

pub struct WriteFileTool;
impl Tool for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing path"))?;
        let content = input.payload["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing content"))?;
        let mode = input.payload["mode"].as_str().unwrap_or("overwrite");
        let path_buf = PathBuf::from(path);
        if let Some(parent) = path_buf.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        match mode {
            "append" => {
                let mut file = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path_buf)?;
                file.write_all(content.as_bytes())?;
            }
            "overwrite" => {
                fs::write(&path_buf, content)?;
            }
            other => {
                anyhow::bail!("unsupported write mode '{}'", other);
            }
        }

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({"path": path, "mode": mode})),
            error: None,
            verification: Some("file_written".to_string()),
            audit_log: Some(format!("Wrote file: {} ({})", path, mode)),
            pua_report: Some(tool_execution_report("write_file", Some("file_written"))),
        })
    }
}

pub struct SearchFilesTool;
impl Tool for SearchFilesTool {
    fn name(&self) -> &'static str {
        "search_files"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let pattern = input.payload["pattern"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing pattern"))?;
        let directory = input.payload["directory"].as_str().unwrap_or(".");
        let root = PathBuf::from(directory);
        let matcher = Pattern::new(pattern)?;
        let mut files = Vec::new();
        collect_matching_files(&root, &root, &matcher, &mut files)?;

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({"files": files})),
            error: None,
            verification: Some("search_done".to_string()),
            audit_log: Some(format!(
                "Search files completed for pattern '{}' in '{}'",
                pattern, directory
            )),
            pua_report: Some(tool_execution_report("search_files", Some("search_done"))),
        })
    }
}

pub struct ApplyPatchTool;
impl Tool for ApplyPatchTool {
    fn name(&self) -> &'static str {
        "apply_patch"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let patch = input.payload["patch"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing patch"))?;
        let check_only = input.payload["check"].as_bool().unwrap_or(false);
        let current_dir = input.payload["directory"].as_str().unwrap_or(".");
        let patch_file = env::temp_dir().join(format!(
            "go_on_patch_{}.diff",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));

        fs::write(&patch_file, patch)?;
        let mut command = Command::new("git");
        command.arg("apply");
        if check_only {
            command.arg("--check");
        }
        let output = command.arg(&patch_file).current_dir(current_dir).output()?;
        let _ = fs::remove_file(&patch_file);
        let success = output.status.success();

        Ok(ToolOutput {
            success,
            result: Some(serde_json::json!({
                "applied": success && !check_only,
                "checked": check_only,
                "stdout": String::from_utf8_lossy(&output.stdout).to_string(),
                "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
                "exit_code": output.status.code(),
            })),
            error: (!success).then(|| String::from_utf8_lossy(&output.stderr).trim().to_string()),
            verification: Some(
                if check_only {
                    "patch_checked"
                } else {
                    "patch_applied"
                }
                .to_string(),
            ),
            audit_log: Some(format!("git apply executed in '{}'", current_dir)),
            pua_report: Some(tool_execution_report(
                "apply_patch",
                Some(if check_only {
                    "patch_checked"
                } else {
                    "patch_applied"
                }),
            )),
        })
    }
}

pub struct RunTestsTool;
impl Tool for RunTestsTool {
    fn name(&self) -> &'static str {
        "run_tests"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let command_name = input.payload["command"].as_str().unwrap_or("cargo");
        let args = input.payload["args"]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(|text| text.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec!["test".to_string()]);
        let current_dir = input.payload["directory"].as_str().unwrap_or(".");
        let output = Command::new(command_name)
            .args(&args)
            .current_dir(current_dir)
            .output()?;
        let success = output.status.success();

        Ok(ToolOutput {
            success,
            result: Some(serde_json::json!({
                "command": command_name,
                "args": args,
                "stdout": String::from_utf8_lossy(&output.stdout).to_string(),
                "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
                "exit_code": output.status.code(),
            })),
            error: (!success).then(|| String::from_utf8_lossy(&output.stderr).trim().to_string()),
            verification: Some("tests_passed".to_string()),
            audit_log: Some(format!("Executed '{}' in '{}'", command_name, current_dir)),
            pua_report: Some(tool_execution_report("run_tests", Some("tests_passed"))),
        })
    }
}

pub struct InspectGitDiffTool;
impl Tool for InspectGitDiffTool {
    fn name(&self) -> &'static str {
        "inspect_git_diff"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let current_dir = input.payload["directory"].as_str().unwrap_or(".");
        let staged = input.payload["staged"].as_bool().unwrap_or(false);
        let files = input.payload["files"]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(|text| text.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let mut command = Command::new("git");
        command.arg("diff").current_dir(current_dir);
        if staged {
            command.arg("--cached");
        }
        if !files.is_empty() {
            command.arg("--").args(&files);
        }
        let output = command.output()?;
        let success = output.status.success();

        Ok(ToolOutput {
            success,
            result: Some(serde_json::json!({
                "diff": String::from_utf8_lossy(&output.stdout).to_string(),
                "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
                "exit_code": output.status.code(),
                "staged": staged,
                "files": files,
            })),
            error: (!success).then(|| String::from_utf8_lossy(&output.stderr).trim().to_string()),
            verification: Some("diff_inspected".to_string()),
            audit_log: Some(format!("git diff inspected in '{}'", current_dir)),
            pua_report: Some(tool_execution_report(
                "inspect_git_diff",
                Some("diff_inspected"),
            )),
        })
    }
}

fn collect_matching_files(
    root: &Path,
    current: &Path,
    matcher: &Pattern,
    files: &mut Vec<String>,
) -> Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_matching_files(root, &path, matcher, files)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn init_git_repo(dir: &Path) {
        run_git(dir, &["init"]);
        run_git(dir, &["config", "user.email", "copilot@example.com"]);
        run_git(dir, &["config", "user.name", "Copilot Test"]);
    }

    fn run_git(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git command should spawn");
        assert!(
            output.status.success(),
            "git command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-task".to_string(),
            phase: "test".to_string(),
            agent_role: "tool".to_string(),
            objective: "tool test".to_string(),
            constraints: None,
            evidence: None,
            payload,
        }
    }

    #[test]
    fn apply_patch_tool_checks_and_applies_patch() {
        let temp = tempdir().expect("tempdir should be created");
        init_git_repo(temp.path());

        let file_path = temp.path().join("sample.txt");
        fs::write(&file_path, "hello\n").expect("initial file should be written");
        run_git(temp.path(), &["add", "sample.txt"]);
        run_git(temp.path(), &["commit", "-m", "init"]);

        fs::write(&file_path, "hello world\n").expect("updated file should be written");
        let patch = run_git(temp.path(), &["diff", "--", "sample.txt"]);
        run_git(temp.path(), &["checkout", "--", "sample.txt"]);

        let tool = ApplyPatchTool;
        let checked = tool
            .run(&tool_input(serde_json::json!({
                "patch": patch,
                "check": true,
                "directory": temp.path().to_string_lossy().to_string(),
            })))
            .expect("patch check should succeed");
        assert!(checked.success);

        let applied = tool
            .run(&tool_input(serde_json::json!({
                "patch": patch,
                "directory": temp.path().to_string_lossy().to_string(),
            })))
            .expect("patch apply should succeed");
        assert!(applied.success);
        let normalized = fs::read_to_string(&file_path)
            .expect("patched file should be readable")
            .replace("\r\n", "\n");
        assert_eq!(normalized, "hello world\n");
    }

    #[test]
    fn run_tests_tool_executes_configured_command() {
        let tool = RunTestsTool;
        let result = tool
            .run(&tool_input(serde_json::json!({
                "command": "git",
                "args": ["--version"],
                "directory": ".",
            })))
            .expect("command should execute");
        assert!(result.success);
        let stdout = result.result.expect("result should exist")["stdout"]
            .as_str()
            .expect("stdout should be string")
            .to_string();
        assert!(stdout.contains("git version"));
    }

    #[test]
    fn inspect_git_diff_tool_returns_actual_diff() {
        let temp = tempdir().expect("tempdir should be created");
        init_git_repo(temp.path());

        let file_path = temp.path().join("sample.txt");
        fs::write(&file_path, "hello\n").expect("initial file should be written");
        run_git(temp.path(), &["add", "sample.txt"]);
        run_git(temp.path(), &["commit", "-m", "init"]);
        fs::write(&file_path, "hello world\n").expect("updated file should be written");

        let tool = InspectGitDiffTool;
        let result = tool
            .run(&tool_input(serde_json::json!({
                "directory": temp.path().to_string_lossy().to_string(),
                "files": ["sample.txt"],
            })))
            .expect("git diff should execute");
        assert!(result.success);
        let diff = result.result.expect("result should exist")["diff"]
            .as_str()
            .expect("diff should be string")
            .to_string();
        assert!(diff.contains("hello world"));
    }
}
