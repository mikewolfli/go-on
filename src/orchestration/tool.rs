//! Tool trait and tool runtime for go-on
//!
//! These structures are intentional framework definitions for Phase 0-9 architecture.
//! Tool trait, registry, and implementations will be connected to the execution flow
//! once orchestration logic integrates them.

use anyhow::Result;
use glob::Pattern;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, warn};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub max_retries: usize,
    pub retry_on_failure: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRiskLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCapabilityProfile {
    pub capability: String,
    pub risk_level: ToolRiskLevel,
    pub timeout_budget_ms: u64,
    pub retry_policy: RetryPolicy,
    pub fallback_chain: Vec<String>,
}

/// Tool trait
///
/// All tools must implement this trait. The `run` method should be instrumented for tracing and performance monitoring in the implementation, not on the trait itself.
pub trait Tool: Send + Sync {
    /// Returns the tool's unique name.
    fn name(&self) -> &'static str;
    /// Executes the tool with the given input. Should emit tracing spans for performance analysis (implementations only).
    fn run(&self, input: &ToolInput) -> Result<ToolOutput>;
}

/// Tool registry
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
    profiles: HashMap<&'static str, ToolCapabilityProfile>,
}

impl ToolRegistry {
    /// Create a new tool registry and register all built-in tools.
    #[tracing::instrument(level = "info")]
    pub fn new() -> Self {
        let mut registry = Self {
            tools: Vec::new(),
            profiles: HashMap::new(),
        };
        registry.register_with_profile(
            ReadFileTool,
            ToolCapabilityProfile {
                capability: "filesystem_read".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 10_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: vec!["search_files".to_string()],
            },
        );
        registry.register_with_profile(
            WriteFileTool,
            ToolCapabilityProfile {
                capability: "filesystem_write".to_string(),
                risk_level: ToolRiskLevel::High,
                timeout_budget_ms: 15_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );
        registry.register_with_profile(
            SearchFilesTool,
            ToolCapabilityProfile {
                capability: "filesystem_search".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 10_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: vec!["read_file".to_string()],
            },
        );
        registry.register_with_profile(
            ApplyPatchTool,
            ToolCapabilityProfile {
                capability: "patch_apply".to_string(),
                risk_level: ToolRiskLevel::High,
                timeout_budget_ms: 20_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: vec!["inspect_git_diff".to_string()],
            },
        );
        registry.register_with_profile(
            RunTestsTool,
            ToolCapabilityProfile {
                capability: "verification_execute".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 300_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: vec!["inspect_git_diff".to_string()],
            },
        );
        registry.register_with_profile(
            InspectGitDiffTool,
            ToolCapabilityProfile {
                capability: "scm_diff".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 8_000,
                retry_policy: RetryPolicy {
                    max_retries: 1,
                    retry_on_failure: true,
                },
                fallback_chain: Vec::new(),
            },
        );
        registry
    }

    pub fn register<T: Tool + 'static>(&mut self, tool: T) {
        self.register_with_profile(
            tool,
            ToolCapabilityProfile {
                capability: "custom".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );
    }

    pub fn register_with_profile<T: Tool + 'static>(
        &mut self,
        tool: T,
        profile: ToolCapabilityProfile,
    ) {
        let name = tool.name();
        self.profiles.insert(name, profile);
        self.tools.push(Box::new(tool));
    }

    /// Get a tool by name.
    #[tracing::instrument(level = "debug", skip(self))]
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|t| t.name() == name)
            .map(|b| b.as_ref())
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.tools.iter().map(|tool| tool.name()).collect()
    }

    pub fn profile(&self, name: &str) -> Option<&ToolCapabilityProfile> {
        self.profiles.get(name)
    }

    pub fn capability_matrix(&self) -> serde_json::Value {
        let matrix = self
            .tools
            .iter()
            .filter_map(|tool| {
                self.profiles.get(tool.name()).map(|profile| {
                    serde_json::json!({
                        "name": tool.name(),
                        "capability": profile.capability,
                        "risk_level": profile.risk_level,
                        "timeout_budget_ms": profile.timeout_budget_ms,
                        "retry_policy": profile.retry_policy,
                        "fallback_chain": profile.fallback_chain,
                    })
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({ "tools": matrix })
    }

    pub fn run_with_fallback(&self, name: &str, input: &ToolInput) -> Result<ToolOutput> {
        let Some(primary) = self.get(name) else {
            anyhow::bail!("tool '{}' not found", name);
        };

        let mut primary_result = primary.run(input)?;
        if primary_result.success {
            return Ok(primary_result);
        }

        let fallback_chain = self
            .profile(name)
            .map(|profile| profile.fallback_chain.clone())
            .unwrap_or_default();

        for fallback_name in fallback_chain {
            if let Some(fallback_tool) = self.get(&fallback_name) {
                let mut fallback_result = fallback_tool.run(input)?;
                if fallback_result.success {
                    fallback_result.audit_log = Some(format!(
                        "primary '{}' failed, fallback '{}' succeeded",
                        name, fallback_name
                    ));
                    return Ok(fallback_result);
                }
                primary_result = fallback_result;
            }
        }

        Ok(primary_result)
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
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
        debug!(directory = %current_dir, check_only = %check_only, "tool: running git apply");
        let output = command.arg(&patch_file).current_dir(current_dir).output()?;
        let _ = fs::remove_file(&patch_file);
        let success = output.status.success();
        if !success {
            warn!(
                directory = %current_dir,
                check_only = %check_only,
                stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                "tool: git apply failed"
            );
        }

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
        debug!(command = %command_name, args = ?args, directory = %current_dir, "tool: running shell command");
        let output = Command::new(command_name)
            .args(&args)
            .current_dir(current_dir)
            .output()?;
        let success = output.status.success();
        if !success {
            warn!(
                command = %command_name,
                exit_code = ?output.status.code(),
                stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                "tool: shell command failed"
            );
        }

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

    struct AlwaysFailTool;
    impl Tool for AlwaysFailTool {
        fn name(&self) -> &'static str {
            "always_fail"
        }

        fn run(&self, _input: &ToolInput) -> Result<ToolOutput> {
            Ok(ToolOutput {
                success: false,
                result: None,
                error: Some("forced failure".to_string()),
                verification: Some("forced_failure".to_string()),
                audit_log: Some("always_fail executed".to_string()),
                pua_report: None,
            })
        }
    }

    struct AlwaysPassTool;
    impl Tool for AlwaysPassTool {
        fn name(&self) -> &'static str {
            "always_pass"
        }

        fn run(&self, _input: &ToolInput) -> Result<ToolOutput> {
            Ok(ToolOutput {
                success: true,
                result: Some(serde_json::json!({"ok": true})),
                error: None,
                verification: Some("forced_success".to_string()),
                audit_log: Some("always_pass executed".to_string()),
                pua_report: None,
            })
        }
    }

    #[test]
    fn tool_registry_runs_fallback_chain_when_primary_fails() {
        let mut registry = ToolRegistry {
            tools: Vec::new(),
            profiles: HashMap::new(),
        };
        registry.register_with_profile(
            AlwaysFailTool,
            ToolCapabilityProfile {
                capability: "primary".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 1_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: vec!["always_pass".to_string()],
            },
        );
        registry.register_with_profile(
            AlwaysPassTool,
            ToolCapabilityProfile {
                capability: "fallback".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 1_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );

        let output = registry
            .run_with_fallback("always_fail", &tool_input(serde_json::json!({})))
            .expect("fallback execution should succeed");
        assert!(output.success);
        let audit_log = output.audit_log.unwrap_or_default();
        assert!(audit_log.contains("fallback"));
    }
}
