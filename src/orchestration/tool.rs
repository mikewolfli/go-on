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
use tracing::{debug, info, warn};

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
    pub allowed_base_dir: Option<PathBuf>,
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

/// Sanitize and validate a file path against the allowed base directory.
///
/// 1. Resolves the path relative to the current working directory.
/// 2. Canonicalizes (or normalizes) the resolved path.
/// 3. If `allowed_base_dir` is set, verifies the resolved path starts with it.
fn sanitize_path(input: &ToolInput, path: &str) -> Result<PathBuf> {
    let resolved = PathBuf::from(path);
    let canonical = if resolved.is_absolute() {
        std::fs::canonicalize(&resolved).unwrap_or(resolved)
    } else {
        let cwd = std::env::current_dir().unwrap_or_default();
        let joined = cwd.join(&resolved);
        std::fs::canonicalize(&joined).unwrap_or(joined)
    };

    if let Some(ref base_dir) = input.allowed_base_dir {
        let base_canonical = std::fs::canonicalize(base_dir).unwrap_or_else(|_| base_dir.clone());
        if !canonical.starts_with(&base_canonical) {
            anyhow::bail!(
                "path traversal denied: '{}' is outside the allowed base directory '{}'",
                path,
                base_dir.display()
            );
        }
    }

    Ok(canonical)
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
        let validated_path = sanitize_path(input, path)?;
        let content = std::fs::read_to_string(&validated_path)?;
        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({"content": content})),
            error: None,
            verification: Some("file_read".to_string()),
            audit_log: Some(format!("Read file: {}", validated_path.display())),
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
        let path_buf = sanitize_path(input, path)?;
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
            audit_log: Some(format!("Wrote file: {} ({})", path_buf.display(), mode)),
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
        let root = sanitize_path(input, directory)?;
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
                pattern,
                root.display()
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

const ALLOWED_TEST_COMMANDS: &[&str] = &[
    "cargo", "npm", "yarn", "pnpm", "make", "go", "python", "pytest", "mvn", "gradle", "git",
];

pub struct RunTestsTool;
impl Tool for RunTestsTool {
    fn name(&self) -> &'static str {
        "run_tests"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let command_name = input.payload["command"].as_str().unwrap_or("cargo");
        if !ALLOWED_TEST_COMMANDS.contains(&command_name) {
            anyhow::bail!(
                "command '{}' is not in the allowed test commands whitelist",
                command_name
            );
        }
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

// ---------------------------------------------------------------------------
// Think-Act-Observe tool execution loop (F-GAP-01)
// ---------------------------------------------------------------------------
//
// Full Think → Act → Observe orchestration loop:
//
// 1. Think:   Analyze task context, select the best tool candidate
// 2. Act:     Execute tool call with fallback-chain support
// 3. Observe: Validate output, decide next action (continue / retry /
//             switch tool / complete / escalate)
//
// Loop termination:
// - Tool succeeds and output verification passes
// - All tool candidates exhausted (retry + fallback limits reached)
// - Maximum iteration count reached

/// Stage label for a single Think-Act-Observe iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopStage {
    Think,
    Act,
    Observe,
}

/// Outcome of a single Observe phase.
#[derive(Debug, Clone)]
pub enum LoopDecision {
    /// Continue to the next Think-Act-Observe cycle.
    Continue,
    /// Retry the same tool.
    Retry { tool: String, reason: String },
    /// Switch to a different tool candidate.
    SwitchTool {
        from: String,
        to: String,
        reason: String,
    },
    /// Loop completed successfully.
    Complete(ToolOutput),
    /// All candidates exhausted – final failure.
    Failed {
        reason: String,
        last_output: Option<ToolOutput>,
    },
    /// Escalate to human review.
    Escalate { reason: String, output: ToolOutput },
}

/// Configuration for the Think-Act-Observe loop.
#[derive(Debug, Clone)]
pub struct LoopConfig {
    /// Maximum number of iterations (Think→Act→Observe cycles).
    pub max_iterations: u32,
    /// Maximum retries per tool before switching.
    pub max_retries_per_tool: u32,
    /// Whether to enable fallback-chain execution.
    pub enable_fallback: bool,
    /// Optional output-verification function.
    pub verify_output: Option<fn(&ToolOutput) -> bool>,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 5,
            max_retries_per_tool: 2,
            enable_fallback: true,
            verify_output: None,
        }
    }
}

/// A single trace entry for one loop iteration.
#[derive(Debug, Clone, Serialize)]
pub struct LoopIteration {
    pub stage: String,
    pub tool: String,
    pub success: bool,
    pub duration_ms: u64,
    pub detail: String,
}

/// Full execution trace of a Think-Act-Observe loop.
#[derive(Debug, Clone, Serialize)]
pub struct LoopTrace {
    pub iterations: Vec<LoopIteration>,
    pub final_decision: String,
    pub total_duration_ms: u64,
}

/// Think phase result: which tool to run and why.
#[derive(Debug, Clone)]
struct ThinkResult {
    tool: String,
    confidence: f64,
    rationale: String,
}

/// Run the Think-Act-Observe loop for a given task.
///
/// # Arguments
///
/// * `task` - Human-readable task description (used for logging / tracing).
/// * `registry` - Tool registry holding all available tools.
/// * `input` - Input envelope passed to each tool.
/// * `preferred_tools` - Ordered list of tool names to try first.
/// * `config` - Loop configuration (iterations, retries, verification).
///
/// # Returns
///
/// A tuple of `(LoopDecision, LoopTrace)` where the decision conveys the
/// final outcome and the trace records every iteration for observability.
pub fn execute_loop(
    task: &str,
    registry: &ToolRegistry,
    input: &ToolInput,
    preferred_tools: &[String],
    config: &LoopConfig,
) -> (LoopDecision, LoopTrace) {
    let start = std::time::Instant::now();
    let mut trace = LoopTrace {
        iterations: Vec::new(),
        final_decision: String::new(),
        total_duration_ms: 0,
    };

    // Build the candidate list with retry bookkeeping.
    let tool_candidates: Vec<String> = if preferred_tools.is_empty() {
        registry.names().iter().map(|&n| n.to_string()).collect()
    } else {
        preferred_tools.to_vec()
    };
    let mut retry_counts: HashMap<String, u32> = HashMap::new();

    for iteration in 0..config.max_iterations {
        // ── Think ────────────────────────────────────────────────
        // Select the best tool candidate based on retry history.
        let think_result = think(task, &tool_candidates, &retry_counts, config);

        let Some(tr) = think_result else {
            let decision = LoopDecision::Failed {
                reason: "no available tool candidates after think phase".to_string(),
                last_output: None,
            };
            trace.final_decision = "failed_no_candidates".to_string();
            trace.total_duration_ms = start.elapsed().as_millis() as u64;
            warn!(task, iteration, "TAO: no candidates – failed");
            return (decision, trace);
        };

        trace.iterations.push(LoopIteration {
            stage: "think".to_string(),
            tool: tr.tool.clone(),
            success: true,
            duration_ms: 0,
            detail: format!(
                "confidence={:.2}, rationale={}",
                tr.confidence, tr.rationale
            ),
        });

        // ── Act ──────────────────────────────────────────────────
        // Execute the selected tool (with fallback if enabled).
        let act_start = std::time::Instant::now();
        let output = if config.enable_fallback {
            registry
                .run_with_fallback(&tr.tool, input)
                .unwrap_or_else(|e| ToolOutput {
                    success: false,
                    result: None,
                    error: Some(format!("tool '{}' error: {}", tr.tool, e)),
                    verification: None,
                    audit_log: None,
                    pua_report: None,
                })
        } else {
            registry.get(&tr.tool).map_or_else(
                || ToolOutput {
                    success: false,
                    result: None,
                    error: Some(format!("tool '{}' not found", tr.tool)),
                    verification: None,
                    audit_log: None,
                    pua_report: None,
                },
                |tool| {
                    tool.run(input).unwrap_or_else(|e| ToolOutput {
                        success: false,
                        result: None,
                        error: Some(format!("{}", e)),
                        verification: None,
                        audit_log: None,
                        pua_report: None,
                    })
                },
            )
        };
        let act_duration_ms = act_start.elapsed().as_millis() as u64;

        trace.iterations.push(LoopIteration {
            stage: "act".to_string(),
            tool: tr.tool.clone(),
            success: output.success,
            duration_ms: act_duration_ms,
            detail: if output.success {
                "execution ok".to_string()
            } else {
                output
                    .error
                    .clone()
                    .unwrap_or_else(|| "unknown error".to_string())
            },
        });

        // ── Observe ──────────────────────────────────────────────
        let observe_decision = observe(
            &output,
            &tr.tool,
            &mut retry_counts,
            config,
            |tool, reason| {
                trace.iterations.push(LoopIteration {
                    stage: "observe".to_string(),
                    tool,
                    success: false,
                    duration_ms: 0,
                    detail: reason,
                });
            },
        );

        match observe_decision {
            LoopDecision::Continue => {
                // Move to next iteration
                trace.iterations.push(LoopIteration {
                    stage: "think".to_string(),
                    tool: tr.tool.clone(),
                    success: true,
                    duration_ms: 0,
                    detail: "output ok, continuing".to_string(),
                });
                continue;
            }
            LoopDecision::Retry { tool, reason } => {
                trace.iterations.push(LoopIteration {
                    stage: "think".to_string(),
                    tool: tool.clone(),
                    success: false,
                    duration_ms: 0,
                    detail: format!("retry: {}", reason),
                });
                continue;
            }
            LoopDecision::SwitchTool { from, to, reason } => {
                debug!(from, to, reason, "TAO: switching tool");
                trace.iterations.push(LoopIteration {
                    stage: "think".to_string(),
                    tool: from,
                    success: false,
                    duration_ms: 0,
                    detail: format!("switch to '{}': {}", to, reason),
                });
                continue;
            }
            LoopDecision::Complete(output) => {
                trace.final_decision = "success".to_string();
                trace.total_duration_ms = start.elapsed().as_millis() as u64;
                info!(
                    task,
                    tool = tr.tool,
                    iterations = iteration + 1,
                    "TAO: completed"
                );
                return (LoopDecision::Complete(output), trace);
            }
            LoopDecision::Failed {
                reason,
                last_output,
            } => {
                trace.final_decision = "failed".to_string();
                trace.total_duration_ms = start.elapsed().as_millis() as u64;
                warn!(task, reason, "TAO: failed");
                return (
                    LoopDecision::Failed {
                        reason,
                        last_output,
                    },
                    trace,
                );
            }
            LoopDecision::Escalate { reason, output } => {
                trace.final_decision = "escalated".to_string();
                trace.total_duration_ms = start.elapsed().as_millis() as u64;
                warn!(task, reason, "TAO: escalated");
                return (LoopDecision::Escalate { reason, output }, trace);
            }
        }
    }

    // Exhausted maximum iterations.
    let decision = LoopDecision::Failed {
        reason: format!("max iterations ({}) reached", config.max_iterations),
        last_output: None,
    };
    trace.final_decision = "failed_max_iterations".to_string();
    trace.total_duration_ms = start.elapsed().as_millis() as u64;
    warn!(
        task,
        max_iterations = config.max_iterations,
        "TAO: max iterations reached"
    );
    (decision, trace)
}

/// Think phase: select the best tool candidate.
///
/// Picks the tool with the fewest retries; if all have been retried to the
/// limit, returns `None` to signal exhaustion.
fn think(
    _task: &str,
    candidates: &[String],
    retry_counts: &HashMap<String, u32>,
    config: &LoopConfig,
) -> Option<ThinkResult> {
    let best = candidates
        .iter()
        .filter(|t| retry_counts.get(*t).copied().unwrap_or(0) < config.max_retries_per_tool)
        .min_by_key(|t| retry_counts.get(*t).copied().unwrap_or(0))?;

    let retries = retry_counts.get(best).copied().unwrap_or(0);
    let confidence = 1.0 - (retries as f64 / config.max_retries_per_tool as f64).min(1.0);

    Some(ThinkResult {
        tool: best.clone(),
        confidence,
        rationale: format!(
                "retries={}/{} candidates_remaining={}",
                retries,
                config.max_retries_per_tool,
                candidates
                    .iter()
                    .filter(|t| retry_counts.get(*t).copied().unwrap_or(0)
                        < config.max_retries_per_tool)
                    .count(),
            ),
    })
}

/// Observe phase: evaluate the output and decide the next action.
fn observe(
    output: &ToolOutput,
    tool: &str,
    retry_counts: &mut HashMap<String, u32>,
    config: &LoopConfig,
    mut on_fail: impl FnMut(String, String),
) -> LoopDecision {
    if output.success {
        // Optional verification check.
        if let Some(verify) = config.verify_output {
            if !verify(output) {
                let rc = retry_counts.entry(tool.to_string()).or_insert(0);
                *rc += 1;
                on_fail(tool.to_string(), "output verification failed".to_string());
                if *rc < config.max_retries_per_tool {
                    return LoopDecision::Retry {
                        tool: tool.to_string(),
                        reason: "verification failed".to_string(),
                    };
                }
                return LoopDecision::SwitchTool {
                    from: tool.to_string(),
                    to: "next_candidate".to_string(),
                    reason: "verification failed, retries exhausted".to_string(),
                };
            }
        }
        return LoopDecision::Complete(output.clone());
    }

    // Execution failed — increment retry count.
    let rc = retry_counts.entry(tool.to_string()).or_insert(0);
    *rc += 1;

    let error_msg = output
        .error
        .clone()
        .unwrap_or_else(|| "no error detail".to_string());
    on_fail(tool.to_string(), format!("execution failed: {}", error_msg));

    if *rc < config.max_retries_per_tool {
        return LoopDecision::Retry {
            tool: tool.to_string(),
            reason: format!(
                "attempt {}/{} failed: {}",
                rc, config.max_retries_per_tool, error_msg
            ),
        };
    }

    // Retries exhausted for this tool — try another candidate.
    LoopDecision::SwitchTool {
        from: tool.to_string(),
        to: "next_candidate".to_string(),
        reason: format!("retries exhausted for '{}': {}", tool, error_msg),
    }
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
            allowed_base_dir: None,
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

    // ── Think-Act-Observe loop tests ─────────────────────────────

    #[test]
    fn tao_loop_completes_on_first_tool_success() {
        let mut registry = ToolRegistry::new();
        registry.register(AlwaysPassTool);

        let input = tool_input(serde_json::json!({"test": true}));
        let config = LoopConfig::default();

        let (decision, trace) = execute_loop(
            "test success",
            &registry,
            &input,
            &["always_pass".to_string()],
            &config,
        );

        match decision {
            LoopDecision::Complete(output) => {
                assert!(output.success);
                assert_eq!(
                    trace.final_decision, "success",
                    "trace should record success"
                );
                assert!(!trace.iterations.is_empty(), "trace must have entries");
            }
            other => panic!("expected Complete, got {:?}", other),
        }
    }

    #[test]
    fn tao_loop_retries_on_failure_then_switches_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(AlwaysFailTool);
        registry.register(AlwaysPassTool);

        let input = tool_input(serde_json::json!({"test": true}));
        let config = LoopConfig {
            max_iterations: 10,
            max_retries_per_tool: 1,
            enable_fallback: true,
            verify_output: None,
        };

        let (decision, trace) = execute_loop(
            "test fail then pass",
            &registry,
            &input,
            &["always_fail".to_string(), "always_pass".to_string()],
            &config,
        );

        match decision {
            LoopDecision::Complete(output) => {
                assert!(output.success);
                assert_eq!(trace.final_decision, "success");
                // Should have attempted always_fail at least once
                let fail_attempts: Vec<_> = trace
                    .iterations
                    .iter()
                    .filter(|i| i.tool == "always_fail")
                    .collect();
                assert!(!fail_attempts.is_empty(), "must have attempted always_fail");
            }
            other => panic!("expected Complete, got {:?}", other),
        }
    }

    #[test]
    fn tao_loop_exhausts_all_candidates_and_fails() {
        let mut registry = ToolRegistry::new();
        registry.register(AlwaysFailTool);

        let input = tool_input(serde_json::json!({"test": true}));
        let config = LoopConfig {
            max_iterations: 5,
            max_retries_per_tool: 1,
            enable_fallback: false,
            verify_output: None,
        };

        let (decision, _trace) = execute_loop(
            "test all fail",
            &registry,
            &input,
            &["always_fail".to_string()],
            &config,
        );

        match decision {
            LoopDecision::Failed { reason, .. } => {
                assert!(!reason.is_empty(), "failure reason must be non-empty");
            }
            other => panic!("expected Failed, got {:?}", other),
        }
    }

    #[test]
    fn tao_loop_respects_custom_verify_function() {
        let mut registry = ToolRegistry::new();
        registry.register(AlwaysPassTool);

        // A verify function that always rejects the output.
        fn reject_always(_: &ToolOutput) -> bool {
            false
        }

        let input = tool_input(serde_json::json!({"test": true}));
        let config = LoopConfig {
            max_iterations: 3,
            max_retries_per_tool: 1,
            enable_fallback: false,
            verify_output: Some(reject_always),
        };

        let (decision, _trace) = execute_loop(
            "test verify reject",
            &registry,
            &input,
            &["always_pass".to_string()],
            &config,
        );

        // Tool succeeds but verification fails → should switch or fail.
        match decision {
            LoopDecision::SwitchTool { .. } | LoopDecision::Failed { .. } => {}
            other => panic!("expected SwitchTool or Failed, got {:?}", other),
        }
    }

    #[test]
    fn tao_loop_with_empty_preferred_tools_falls_back_to_registry_and_completes() {
        // When preferred_tools is empty, execute_loop falls back to registry.names().
        // The default ToolRegistry has built-in tools — one of them (e.g. inspect_git_diff)
        // will succeed on the test input, so the loop completes.
        let registry = ToolRegistry::new();
        let input = tool_input(serde_json::json!({"directory": ".", "test": true}));
        let config = LoopConfig::default();

        let (decision, trace) = execute_loop(
            "test fallback to registry",
            &registry,
            &input,
            &[], // no preferred tools — falls back to registry.names()
            &config,
        );

        match decision {
            LoopDecision::Complete(output) => {
                assert!(output.success);
                assert_eq!(trace.final_decision, "success");
            }
            other => panic!(
                "expected Complete (fallback to registry tools), got {:?}",
                other
            ),
        }
    }
}
