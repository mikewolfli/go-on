//! Tool pipeline for composing and executing sequential tool workflows.
//!
//! Builds on the tool registry to execute steps sequentially.

use serde_json::Value;
use std::time::Instant;
use tracing;

use crate::governance::hardening::SandboxLevel;
use crate::orchestration::tool::{ToolInput, ToolRegistry};

// ---------------------------------------------------------------------------
// PipelineStep
// ---------------------------------------------------------------------------

/// A single step in a tool execution pipeline.
pub struct PipelineStep {
    /// Name of the tool to execute.
    pub tool_name: String,
    /// Input payload for the tool.
    pub input: Value,
}

// ---------------------------------------------------------------------------
// PipelineErrorStrategy
// ---------------------------------------------------------------------------

/// Determines behaviour when a pipeline step fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineErrorStrategy {
    /// Continue executing remaining steps despite an error.
    Continue,
}

// ---------------------------------------------------------------------------
// ToolPipeline
// ---------------------------------------------------------------------------

/// A named, executable pipeline of tool steps with an error strategy.
pub struct ToolPipeline {
    /// Human-readable name for observability.
    pub name: String,
    /// Steps that make up this pipeline.
    pub steps: Vec<PipelineStep>,
    /// Error handling strategy applied across all steps.
    pub on_error: PipelineErrorStrategy,
    /// Sandbox enforcement level (None = no governance checks).
    pub sandbox_level: Option<SandboxLevel>,
}

// ---------------------------------------------------------------------------
// PipelineResult / PipelineStepResult
// ---------------------------------------------------------------------------

/// Outcome of executing an entire pipeline.
#[derive(Debug, Clone)]
pub struct PipelineResult {
    /// Per-step results in execution order.
    pub step_results: Vec<PipelineStepResult>,
    /// Total wall-clock duration in milliseconds.
    pub total_duration_ms: u64,
    /// Whether all steps completed without error.
    pub success: bool,
}

/// Outcome of a single pipeline step.
#[derive(Debug, Clone)]
pub struct PipelineStepResult {
    /// Name of the tool executed.
    pub tool_name: String,
    /// Output value produced by the tool (None if the step failed).
    pub output: Option<Value>,
    /// Error message if the step failed.
    pub error: Option<String>,
    /// Wall-clock duration of this step in milliseconds.
    pub duration_ms: u64,
}

// ---------------------------------------------------------------------------
// Pipeline execution
// ---------------------------------------------------------------------------

impl ToolPipeline {
    /// Execute all pipeline steps sequentially against the given tool registry.
    pub async fn execute(&self, registry: &ToolRegistry, context: &Value) -> PipelineResult {
        let total_start = Instant::now();
        let mut step_results: Vec<PipelineStepResult> = Vec::new();
        let mut all_success = true;
        let mut tool_calls_used: u32 = 0;

        for step in &self.steps {
            let (results, should_continue) = execute_step(
                registry,
                step,
                context,
                self.on_error,
                self.sandbox_level,
                &mut tool_calls_used,
            )
            .await;

            let step_success = results.iter().all(|r| r.error.is_none());
            if !step_success {
                all_success = false;
            }

            step_results.extend(results);

            if !should_continue {
                break;
            }
        }

        PipelineResult {
            step_results,
            total_duration_ms: total_start.elapsed().as_millis() as u64,
            success: all_success,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal step executor
// ---------------------------------------------------------------------------

/// Map a tool name to a governance action for pipeline sandbox checks.
/// This mirrors the evaluator's tool-to-action mapping in a simplified form.
///
/// # Security audit
/// All tools registered in `ToolRegistry::new()` must be mapped here.
/// Unknown tools default to "read" (lowest risk) but log a warning.
fn pipeline_tool_to_action(tool_name: &str) -> &'static str {
    match tool_name {
        // ── Read operations (read-only file/content access) ──
        "read_file" | "search_files" | "inspect_git_diff" | "list_directory" | "date_time"
        | "skill_list" | "archive_inspect" | "jsonl_read" | "diagnostics" | "environment_info"
        | "echo_skill" | "builtin.echo" | "goon_skill_version_list"
        | "skill-finder" | "chat.execute"
        | "acp_trace_get" | "acp_debug_panel_get"
        | "goon_workflow_run_list" | "goon_workflow_run_get"
        | "goon_metrics_window_query" | "goon_metrics_errors_summary"
        | "goon_provider_capabilities" | "prompts_list" | "prompts_get"
        | "workflow_execute" | "workflow_ask" | "workflow_generate"
        | "import_skill" | "skill_reload"
        | "semantic_search"
        // ── CAD read tools (read-only 3d/2d format parsing) ──
        | "dxf_read" | "stl_read" | "obj_read" | "step_read" | "ply_read" | "iges_read"
        | "gltf_read" | "svg_read" | "obj_model_read" | "gcode_read" | "gpx_read" | "geo_util"
        // ── Image read/analyze tools ──
        | "image_analyze"
        // ── Document read tools ──
        | "read_docx" | "read_excel" | "read_pdf" | "read_ppt"
        | "email_parse" | "csv_read" | "csv_analyze" | "toml_read" | "yaml_read"
        | "web_scrape" | "invoice_parse" | "rss_read" | "sqlite_query" => "read",

        // ── Search operations ──
        "grep" | "find_path" | "find_files" | "code_index_search" => "search",

        // ── Write operations (file creation/modification) ──
        "write_file"
        | "apply_patch"
        | "create_directory"
        | "delete_path"
        | "move_path"
        | "copy_path"
        | "file_move"
        | "file_delete"
        | "compress"
        | "decompress"
        | "archive_extract"
        | "jsonl_write"
        | "csv_write"
        | "csv_transform"
        | "toml_write"
        | "yaml_write"
        | "game_mod_install"
        | "game_replay_recorder"
        | "game_save_manager"
        | "game_screen_capture"
        | "goon_skill_update"
        | "goon_skill_version_rollback"
        | "goon_workflow_run_cancel"
        | "goon_workflow_run_pause"
        | "goon_workflow_run_resume"
        | "image_generate"
        | "image_resize"
        | "image_convert"
        | "skill-creator"
        | "stl_generate"
        | "svg_export"
        | "svg_generate"
        | "qrcode_generate"
        | "write_docx"
        | "write_excel"
        | "write_ppt"
        | "pdf_merge" | "pdf_split"
        | "cad_convert"
        | "game_auto_grind"
        | "game_keyboard_input"
        | "game_mouse_input"
        | "game_state_modify" => "write",

        // ── Shell operations (command/code execution) ──
        "run_tests"
        | "execute_command"
        | "terminal"
        | "bash"
        | "cargo_test"
        | "shell_exec"
        | "cargo_check"
        | "game_launch"
        | "skill_execute" => "shell",

        // ── Network operations (outbound) ──
        "http_request"
        | "dns_lookup"
        | "ping"
        | "port_scan"
        | "git"
        | "github_search_skills"
        | "game_monitor"
        | "game_online_status"
        | "goon_provider_test_completion"
        | "goon_provider_test_connection" => "network",

        // Unknown — default to read (lowest risk), log warning for security audit
        _ => {
            tracing::warn!(
                target: "tool_pipeline",
                tool = %tool_name,
                "pipeline_tool_to_action: unknown tool '{}', defaulting to 'read' action — audit needed",
                tool_name,
            );
            "read"
        }
    }
}

/// Check if a tool is allowed at the given sandbox level.
fn check_tool_in_pipeline(
    tool_name: &str,
    sandbox_level: Option<SandboxLevel>,
) -> Result<(), String> {
    let Some(level) = sandbox_level else {
        return Ok(()); // No sandbox enforcement
    };
    let action = pipeline_tool_to_action(tool_name);
    let result = crate::governance::hardening::SandboxPolicy::check_with_feedback(level, action);
    if result.allowed {
        Ok(())
    } else {
        let hint = result
            .hint
            .unwrap_or("Try a different tool or adjust sandbox level in config.");
        Err(format!(
            "tool '{}' denied by sandbox policy at level '{}' (action: '{}'). {}. Hint: {}",
            tool_name, level, action, result.reason, hint
        ))
    }
}

/// Execute a single [`PipelineStep`] and return its results plus
/// a flag indicating whether execution should continue.
///
/// Governance checks (sandbox + budget) are applied before executing the tool.
async fn execute_step(
    registry: &ToolRegistry,
    step: &PipelineStep,
    _context: &Value,
    strategy: PipelineErrorStrategy,
    sandbox_level: Option<SandboxLevel>,
    tool_calls_used: &mut u32,
) -> (Vec<PipelineStepResult>, bool) {
    let PipelineStep { tool_name, input } = step;

    // ── Sandbox governance check ──────────────────────────────────────────
    if let Err(e) = check_tool_in_pipeline(tool_name, sandbox_level) {
        tracing::warn!(
            target: "tool_pipeline",
            tool = %tool_name,
            error = %e,
            "pipeline step blocked by sandbox policy"
        );
        let result = PipelineStepResult {
            tool_name: tool_name.to_string(),
            output: None,
            error: Some(e),
            duration_ms: 0,
        };
        let should_continue = strategy == PipelineErrorStrategy::Continue;
        return (vec![result], should_continue);
    }

    // ── Budget governance: max 256 tool calls per pipeline ────────────────
    *tool_calls_used += 1;
    if *tool_calls_used > 256 {
        let result = PipelineStepResult {
            tool_name: tool_name.to_string(),
            output: None,
            error: Some("pipeline budget exceeded: max 256 tool calls per pipeline".to_string()),
            duration_ms: 0,
        };
        return (vec![result], false);
    }

    tracing::info!(
        target: "tool_pipeline",
        tool = %tool_name,
        sandbox = ?sandbox_level,
        tool_calls_used = *tool_calls_used,
        "pipeline step — governance check passed"
    );

    let result = run_single_tool(registry, tool_name, input).await;
    let should_continue = result.error.is_none() || strategy == PipelineErrorStrategy::Continue;
    (vec![result], should_continue)
}

/// Execute a single tool and record the result.
async fn run_single_tool(
    registry: &ToolRegistry,
    tool_name: &str,
    input: &Value,
) -> PipelineStepResult {
    let span = tracing::info_span!(
        "tool.run_single",
        tool = %tool_name,
        input_size = input.to_string().len() as u64,
        latency_ms = 0u64,
        success = false,
    );
    let _guard = span.enter();
    let start = Instant::now();

    let tool_input = ToolInput {
        task_id: "pipeline".to_string(),
        phase: "execute".to_string(),
        agent_role: "pipeline".to_string(),
        objective: format!("execute {}", tool_name),
        constraints: None,
        evidence: None,
        payload: input.clone(),
        allowed_base_dir: None,
    };

    let output = match registry
        .run_with_fallback_async(tool_name, &tool_input)
        .await
    {
        Ok(out) => out,
        Err(e) => {
            let duration_ms = start.elapsed().as_millis() as u64;
            span.record("latency_ms", duration_ms);
            span.record("success", false);
            tracing::warn!(
                target: "tool_execution",
                tool = %tool_name,
                duration_ms = duration_ms,
                error = %e,
                "tool execution failed"
            );
            return PipelineStepResult {
                tool_name: tool_name.to_string(),
                output: None,
                error: Some(format!("{}", e)),
                duration_ms,
            };
        }
    };

    let duration_ms = start.elapsed().as_millis() as u64;
    let logical_error = if output.success {
        span.record("latency_ms", duration_ms);
        span.record("success", true);
        None
    } else {
        span.record("latency_ms", duration_ms);
        span.record("success", false);
        Some(
            output
                .error
                .clone()
                .unwrap_or_else(|| format!("tool '{}' reported unsuccessful result", tool_name)),
        )
    };

    PipelineStepResult {
        tool_name: tool_name.to_string(),
        output: Some(output.result.unwrap_or(Value::Null)),
        error: logical_error,
        duration_ms,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// -----------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::tool::{Tool, ToolInput, ToolOutput, ToolRegistry};
    use serde_json::json;

    struct EchoTool;

    impl Tool for EchoTool {
        fn name(&self) -> &'static str {
            "echo"
        }
        fn run(&self, _input: &ToolInput) -> anyhow::Result<ToolOutput> {
            Ok(ToolOutput {
                success: true,
                result: Some(json!({"echoed": true})),
                error: None,
                verification: None,
                audit_log: None,
                pua_report: None,
            })
        }
    }

    #[tokio::test]
    async fn single_step_pipeline_succeeds() {
        let mut registry = ToolRegistry::new_empty();
        registry.register(EchoTool);

        let pipeline = ToolPipeline {
            name: "test-single".to_string(),
            steps: vec![PipelineStep {
                tool_name: "echo".to_string(),
                input: json!({}),
            }],
            on_error: PipelineErrorStrategy::Continue,
            sandbox_level: None,
        };

        let result = pipeline.execute(&registry, &json!({})).await;
        assert!(result.success);
        assert_eq!(result.step_results.len(), 1);
        assert!(result.step_results[0].error.is_none());
    }
}
