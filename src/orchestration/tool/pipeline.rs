//! Tool pipeline for composing and executing multi-step tool workflows.
//!
//! Builds on the tool registry to support sequential, parallel, and
//! conditional tool execution with configurable error handling strategies.

use serde_json::Value;
use std::time::Instant;
use tracing;

use crate::orchestration::tool::{ToolInput, ToolRegistry};

// ---------------------------------------------------------------------------
// PipelineStep
// ---------------------------------------------------------------------------

/// A single step in a tool execution pipeline.
pub enum PipelineStep {
    /// Single tool execution.
    Single { tool_name: String, input: Value },
}

// ---------------------------------------------------------------------------
// PipelineErrorStrategy
// ---------------------------------------------------------------------------

/// Determines behaviour when a pipeline step fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineErrorStrategy {
    /// Continue executing remaining steps despite the error.
    Continue,
}

// ---------------------------------------------------------------------------
// ToolPipeline
// ---------------------------------------------------------------------------

/// A named, executable pipeline of tool steps with an error strategy.
pub struct ToolPipeline {
    /// Human-readable name for observability.
    pub _name: String,
    /// Steps that make up this pipeline.
    pub steps: Vec<PipelineStep>,
    /// Error handling strategy applied across all steps.
    pub on_error: PipelineErrorStrategy,
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
    /// Execute all pipeline steps against the given tool registry.
    ///
    /// Walks the pipeline DAG respecting sequence, parallel, and conditional
    /// semantics.  The `context` value is available for conditional evaluations.
    pub async fn execute(&self, registry: &ToolRegistry, context: &Value) -> PipelineResult {
        let total_start = Instant::now();
        let mut step_results: Vec<PipelineStepResult> = Vec::new();
        let mut all_success = true;

        for step in &self.steps {
            let (results, should_continue) =
                execute_step(registry, step, context, self.on_error).await;

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

/// Recursively execute a single [`PipelineStep`] and return its results plus
/// a flag indicating whether execution should continue.
async fn execute_step(
    registry: &ToolRegistry,
    step: &PipelineStep,
    _context: &Value,
    strategy: PipelineErrorStrategy,
) -> (Vec<PipelineStepResult>, bool) {
    match step {
        PipelineStep::Single { tool_name, input } => {
            let result = run_single_tool(registry, tool_name, input).await;
            let should_continue =
                result.error.is_none() || strategy == PipelineErrorStrategy::Continue;
            (vec![result], should_continue)
        }
    }
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

    let output = match registry.run_with_fallback(tool_name, &tool_input) {
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::tool::{Tool, ToolInput, ToolOutput, ToolRegistry};
    use serde_json::json;

    /// A tool that succeeds and echoes its input as output.
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

    /// A tool that always fails.
    #[allow(dead_code)]
    struct FailTool;

    impl Tool for FailTool {
        fn name(&self) -> &'static str {
            "fail"
        }
        fn run(&self, _input: &ToolInput) -> anyhow::Result<ToolOutput> {
            Ok(ToolOutput {
                success: false,
                result: None,
                error: Some("intentional failure".to_string()),
                verification: None,
                audit_log: None,
                pua_report: None,
            })
        }
    }

    #[allow(dead_code)] // F-GAP-49 — reserved for tool subsystem
                        // F-GAP-49 — reserved for future use
    fn dummy_input() -> ToolInput {
        ToolInput {
            task_id: "pipeline-test".to_string(),
            phase: "test".to_string(),
            agent_role: "tester".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: json!({}),
            allowed_base_dir: None,
        }
    }

    // -----------------------------------------------------------------------
    // Pipeline execute helper
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn single_step_pipeline_succeeds() {
        let mut registry = ToolRegistry::new_empty();
        registry.register(EchoTool);

        let pipeline = ToolPipeline {
            _name: "test-single".to_string(),
            steps: vec![PipelineStep::Single {
                tool_name: "echo".to_string(),
                input: json!({}),
            }],
            on_error: PipelineErrorStrategy::Continue,
        };

        let result = pipeline.execute(&registry, &json!({})).await;
        assert!(result.success);
        assert_eq!(result.step_results.len(), 1);
        assert!(result.step_results[0].error.is_none());
    }
}
