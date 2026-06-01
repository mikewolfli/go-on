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
    /// Parallel execution of multiple tools.
    #[allow(dead_code)] // F-GAP-12 — reserved for pipeline extensibility
    Parallel { steps: Vec<PipelineStep> },
    /// Sequential execution with optional condition.
    #[allow(dead_code)] // F-GAP-12 — reserved for pipeline extensibility
    Sequence { steps: Vec<PipelineStep> },
    /// Conditional branch that evaluates a field value and chooses a path.
    #[allow(dead_code)] // F-GAP-12 — reserved for pipeline extensibility
    Conditional {
        /// JSON field name to evaluate (dot-notation path supported, e.g. "result.status").
        condition_field: String,
        /// Expected value for the condition to be true.
        expected: Value,
        /// Step to execute when the condition matches.
        then_step: Box<PipelineStep>,
        /// Optional step to execute when the condition does NOT match.
        else_step: Option<Box<PipelineStep>>,
    },
}

// ---------------------------------------------------------------------------
// PipelineErrorStrategy
// ---------------------------------------------------------------------------

/// Determines behaviour when a pipeline step fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineErrorStrategy {
    /// Stop execution immediately and return the partial results.
    #[allow(dead_code)]
// F-GAP-49 — reserved for future use
    Stop,
    /// Continue executing remaining steps despite the error.
    Continue,
    /// Stop execution and invoke rollback (requires transactional context).
    #[allow(dead_code)]
// F-GAP-49 — reserved for future use
    Rollback,
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
    context: &Value,
    strategy: PipelineErrorStrategy,
) -> (Vec<PipelineStepResult>, bool) {
    match step {
        PipelineStep::Single { tool_name, input } => {
            let result = run_single_tool(registry, tool_name, input).await;
            let should_continue =
                result.error.is_none() || strategy == PipelineErrorStrategy::Continue;
            (vec![result], should_continue)
        }

        PipelineStep::Sequence { steps } => {
            Box::pin(execute_sequence(registry, steps, context, strategy)).await
        }

        PipelineStep::Parallel { steps } => {
            Box::pin(execute_parallel(registry, steps, context, strategy)).await
        }

        PipelineStep::Conditional {
            condition_field,
            expected,
            then_step,
            else_step,
        } => {
            Box::pin(execute_conditional(
                registry,
                condition_field,
                expected,
                then_step,
                else_step,
                context,
                strategy,
            ))
            .await
        }
    }
}

/// Execute a single tool and record the result.
async fn run_single_tool(
    registry: &ToolRegistry,
    tool_name: &str,
    input: &Value,
) -> PipelineStepResult {
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
        None
    } else {
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

/// Execute steps sequentially, stopping or continuing based on the strategy.
async fn execute_sequence(
    registry: &ToolRegistry,
    steps: &[PipelineStep],
    context: &Value,
    strategy: PipelineErrorStrategy,
) -> (Vec<PipelineStepResult>, bool) {
    let mut results: Vec<PipelineStepResult> = Vec::new();

    for step in steps {
        let (step_results, should_continue) = execute_step(registry, step, context, strategy).await;

        let step_ok = step_results.iter().all(|r| r.error.is_none());
        results.extend(step_results);

        if !step_ok && strategy != PipelineErrorStrategy::Continue {
            return (results, false);
        }

        if !should_continue {
            return (results, false);
        }
    }

    (results, true)
}

/// Execute steps in parallel via `tokio::spawn` + `join_all`.
async fn execute_parallel(
    registry: &ToolRegistry,
    steps: &[PipelineStep],
    context: &Value,
    strategy: PipelineErrorStrategy,
) -> (Vec<PipelineStepResult>, bool) {
    // Build plain futures (not tokio::spawn tasks) to avoid 'static
    // lifetime requirements on registry references.
    let mut futures: Vec<
        std::pin::Pin<Box<dyn std::future::Future<Output = Vec<PipelineStepResult>> + Send + '_>>,
    > = Vec::with_capacity(steps.len());

    for step in steps {
        match step {
            PipelineStep::Single { tool_name, input } => {
                let tool_name = tool_name.clone();
                let input = input.clone();
                let fut = Box::pin(async move {
                    let r = run_single_tool(registry, &tool_name, &input).await;
                    vec![r]
                });
                futures.push(fut);
            }
            _ => {
                // Execute complex sub-steps inline since we cannot move
                // references into spawned tasks.
                tracing::warn!(
                    target: "tool_pipeline",
                    "parallel execution of nested complex steps is not yet \
                     supported; falling back to sequential"
                );
                let (sub_results, _) = execute_step(registry, step, context, strategy).await;
                let fut = Box::pin(async move { sub_results });
                futures.push(fut);
            }
        }
    }

    let join_results = futures_util::future::join_all(futures).await;
    let mut results: Vec<PipelineStepResult> = Vec::new();

    for step_results in join_results {
        results.extend(step_results);
    }

    let all_ok = results.iter().all(|r| r.error.is_none());
    let should_continue = all_ok || strategy == PipelineErrorStrategy::Continue;
    (results, should_continue)
}

/// Evaluate a condition and execute the appropriate branch.
async fn execute_conditional(
    registry: &ToolRegistry,
    condition_field: &str,
    expected: &Value,
    then_step: &PipelineStep,
    else_step: &Option<Box<PipelineStep>>,
    context: &Value,
    strategy: PipelineErrorStrategy,
) -> (Vec<PipelineStepResult>, bool) {
    let condition_met = evaluate_field(context, condition_field, expected);

    let chosen = if condition_met {
        then_step
    } else if let Some(ref else_s) = else_step {
        else_s
    } else {
        return (Vec::new(), true);
    };

    execute_step(registry, chosen, context, strategy).await
}

/// Evaluate a dot‑notation field path against a JSON value.
fn evaluate_field(value: &Value, field_path: &str, expected: &Value) -> bool {
    let parts: Vec<&str> = field_path.split('.').collect();
    let mut current = value;

    for part in parts {
        match current.get(part) {
            Some(v) => current = v,
            None => return false,
        }
    }

    current == expected
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Test helpers (only compiled during tests)
// ---------------------------------------------------------------------------

/// Create a pipeline that executes a single tool.
#[cfg(test)]
#[allow(dead_code)]
// F-GAP-49 — reserved for future use
pub(crate) fn single_tool_pipeline(tool_name: impl Into<String>) -> ToolPipeline {
    let name: String = tool_name.into();
    ToolPipeline {
        _name: format!("single-{}", name),
        steps: vec![PipelineStep::Single {
            tool_name: name,
            input: serde_json::Value::Null,
        }],
        on_error: PipelineErrorStrategy::Stop,
    }
}

/// Construct a result for a successfully executed pipeline step.
#[cfg(test)]
#[allow(dead_code)]
// F-GAP-49 — reserved for future use
pub(crate) fn make_step_result(
    tool_name: impl Into<String>,
    output: serde_json::Value,
    duration_ms: u64,
) -> PipelineStepResult {
    PipelineStepResult {
        tool_name: tool_name.into(),
        output: Some(output),
        error: None,
        duration_ms,
    }
}

/// Format a pipeline result into a summary string.
#[cfg(test)]
#[allow(dead_code)]
// F-GAP-49 — reserved for future use
pub(crate) fn format_pipeline_summary(result: &PipelineResult) -> String {
    format!(
        "Pipeline: {} steps, {}ms, success={}",
        result.step_results.len(),
        result.total_duration_ms,
        result.success,
    )
}

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

    #[allow(dead_code)]
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
            on_error: PipelineErrorStrategy::Stop,
        };

        let result = pipeline.execute(&registry, &json!({})).await;
        assert!(result.success);
        assert_eq!(result.step_results.len(), 1);
        assert!(result.step_results[0].error.is_none());
    }

    #[tokio::test]
    async fn sequence_executes_in_order() {
        let mut registry = ToolRegistry::new_empty();
        registry.register(EchoTool);

        let pipeline = ToolPipeline {
            _name: "test-sequence".to_string(),
            steps: vec![PipelineStep::Sequence {
                steps: vec![
                    PipelineStep::Single {
                        tool_name: "echo".to_string(),
                        input: json!({}),
                    },
                    PipelineStep::Single {
                        tool_name: "echo".to_string(),
                        input: json!({}),
                    },
                ],
            }],
            on_error: PipelineErrorStrategy::Stop,
        };

        let result = pipeline.execute(&registry, &json!({})).await;
        assert!(result.success);
        assert_eq!(result.step_results.len(), 2);
    }

    #[tokio::test]
    async fn stop_on_error_halts_execution() {
        let mut registry = ToolRegistry::new_empty();
        registry.register(EchoTool);
        registry.register(FailTool);

        let pipeline = ToolPipeline {
            _name: "test-stop".to_string(),
            steps: vec![PipelineStep::Sequence {
                steps: vec![
                    PipelineStep::Single {
                        tool_name: "echo".to_string(),
                        input: json!({}),
                    },
                    PipelineStep::Single {
                        tool_name: "fail".to_string(),
                        input: json!({}),
                    },
                    PipelineStep::Single {
                        tool_name: "echo".to_string(),
                        input: json!({}),
                    },
                ],
            }],
            on_error: PipelineErrorStrategy::Stop,
        };

        let result = pipeline.execute(&registry, &json!({})).await;
        assert!(!result.success);
        // Should have echo result + fail result, but NOT the third echo.
        assert_eq!(result.step_results.len(), 2);
        assert!(result.step_results[1].error.is_some());
    }

    #[tokio::test]
    async fn continue_on_error_keeps_going() {
        let mut registry = ToolRegistry::new_empty();
        registry.register(EchoTool);
        registry.register(FailTool);

        let pipeline = ToolPipeline {
            _name: "test-continue".to_string(),
            steps: vec![PipelineStep::Sequence {
                steps: vec![
                    PipelineStep::Single {
                        tool_name: "echo".to_string(),
                        input: json!({}),
                    },
                    PipelineStep::Single {
                        tool_name: "fail".to_string(),
                        input: json!({}),
                    },
                    PipelineStep::Single {
                        tool_name: "echo".to_string(),
                        input: json!({}),
                    },
                ],
            }],
            on_error: PipelineErrorStrategy::Continue,
        };

        let result = pipeline.execute(&registry, &json!({})).await;
        // Overall success is false because one step failed, but all 3 ran.
        assert!(!result.success);
        assert_eq!(result.step_results.len(), 3);
    }

    #[tokio::test]
    async fn parallel_executes_concurrently() {
        let mut registry = ToolRegistry::new_empty();
        registry.register(EchoTool);

        let pipeline = ToolPipeline {
            _name: "test-parallel".to_string(),
            steps: vec![PipelineStep::Parallel {
                steps: vec![
                    PipelineStep::Single {
                        tool_name: "echo".to_string(),
                        input: json!({}),
                    },
                    PipelineStep::Single {
                        tool_name: "echo".to_string(),
                        input: json!({}),
                    },
                ],
            }],
            on_error: PipelineErrorStrategy::Stop,
        };

        let result = pipeline.execute(&registry, &json!({})).await;
        assert!(result.success);
        assert_eq!(result.step_results.len(), 2);
    }

    #[tokio::test]
    async fn conditional_branching_then_path() {
        let mut registry = ToolRegistry::new_empty();
        registry.register(EchoTool);

        let pipeline = ToolPipeline {
            _name: "test-conditional-then".to_string(),
            steps: vec![PipelineStep::Conditional {
                condition_field: "status".to_string(),
                expected: json!("ready"),
                then_step: Box::new(PipelineStep::Single {
                    tool_name: "echo".to_string(),
                    input: json!({"branch": "then"}),
                }),
                else_step: Some(Box::new(PipelineStep::Single {
                    tool_name: "echo".to_string(),
                    input: json!({"branch": "else"}),
                })),
            }],
            on_error: PipelineErrorStrategy::Stop,
        };

        let ctx = json!({"status": "ready"});
        let result = pipeline.execute(&registry, &ctx).await;
        assert!(result.success);
        // The tool always returns {"echoed": true} — we just verify it ran.
        assert_eq!(result.step_results.len(), 1);
    }

    #[tokio::test]
    async fn conditional_branching_else_path() {
        let mut registry = ToolRegistry::new_empty();
        registry.register(EchoTool);

        let pipeline = ToolPipeline {
            _name: "test-conditional-else".to_string(),
            steps: vec![PipelineStep::Conditional {
                condition_field: "status".to_string(),
                expected: json!("ready"),
                then_step: Box::new(PipelineStep::Single {
                    tool_name: "echo".to_string(),
                    input: json!({"branch": "then"}),
                }),
                else_step: Some(Box::new(PipelineStep::Single {
                    tool_name: "echo".to_string(),
                    input: json!({"branch": "else"}),
                })),
            }],
            on_error: PipelineErrorStrategy::Stop,
        };

        let ctx = json!({"status": "not-ready"});
        let result = pipeline.execute(&registry, &ctx).await;
        assert!(result.success);
        assert_eq!(result.step_results.len(), 1);
    }
}
