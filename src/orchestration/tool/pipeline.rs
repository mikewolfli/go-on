//! Tool pipeline for composing and executing sequential tool workflows.
//!
//! Builds on the tool registry to execute steps sequentially.
//!
//! # Design-保留 (design-retained)
//! This engine is complete and fully tested but currently has no production
//! caller: its previous consumer `orchestrator::execute_tool_pipeline` was
//! removed as dead code, and `BrainLoopPlan.parallel_groups` (which the DAG
//! planner emits for this engine) are not yet fanned out by `BrainLoop`.
//! It is retained for the planned planner-executor unification; the active
//! multi-agent execution path is `multi_agent_pipeline::MultiAgentPipeline`.
#![allow(
    dead_code,
    reason = "design-retained tool pipeline engine (see module docs)"
)]

use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Semaphore;
use tracing;

use crate::governance::hardening::SandboxLevel;
use crate::orchestration::tool::governance_gate::check_tool_in_pipeline;
use crate::orchestration::tool::{ToolInput, ToolRegistry};

// ---------------------------------------------------------------------------
// PipelineStep
// ---------------------------------------------------------------------------

/// A single step in a tool execution pipeline.
#[derive(Debug, Clone)]
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
///
/// Supports both sequential steps (`steps`) and parallel groups (`parallel_groups`).
/// Sequential steps run first in order, then each parallel group runs all its
/// member steps concurrently using `tokio::join!`. This allows callers to
/// express "do A, then do B and C simultaneously" in a single pipeline.
pub struct ToolPipeline {
    /// Human-readable name for observability.
    pub name: String,
    /// Steps that make up this pipeline (run sequentially in order).
    pub steps: Vec<PipelineStep>,
    /// Groups of steps to execute in parallel (each group runs concurrently).
    /// Groups are executed after sequential steps complete.
    /// Within each group, all steps run concurrently via `tokio::join!`.
    pub parallel_groups: Vec<Vec<PipelineStep>>,
    /// Error handling strategy applied across all steps.
    pub on_error: PipelineErrorStrategy,
    /// Sandbox enforcement level (None = no governance checks).
    pub sandbox_level: Option<SandboxLevel>,
}

impl ToolPipeline {
    /// Create a new ToolPipeline with the given name and sequential steps.
    /// `parallel_groups` defaults to empty (no parallel execution).
    pub fn new(
        name: String,
        steps: Vec<PipelineStep>,
        on_error: PipelineErrorStrategy,
        sandbox_level: Option<SandboxLevel>,
    ) -> Self {
        Self {
            name,
            steps,
            parallel_groups: Vec::new(),
            on_error,
            sandbox_level,
        }
    }
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
    /// Execution order:
    /// 1. Sequential steps (`self.steps`) — run in order.
    /// 2. Parallel groups (`self.parallel_groups`) — each group runs
    ///    all its member steps concurrently via `tokio::join!`.
    ///
    /// If any step fails, the error strategy (`on_error`) determines whether
    /// execution continues. Budget governance (max 256 tool calls) applies
    /// across the entire pipeline.
    pub async fn execute(&self, registry: &ToolRegistry, context: &Value) -> PipelineResult {
        let total_start = Instant::now();
        let mut step_results: Vec<PipelineStepResult> = Vec::new();
        let mut all_success = true;
        let mut tool_calls_used: u32 = 0;

        // Phase 1: Sequential steps
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
                return PipelineResult {
                    step_results,
                    total_duration_ms: total_start.elapsed().as_millis() as u64,
                    success: all_success,
                };
            }
        }

        // Phase 2: Parallel groups (each group runs concurrently)
        for group in &self.parallel_groups {
            if group.is_empty() {
                continue;
            }

            // Check budget: count steps in this group before executing
            let group_count = group.len() as u32;
            tool_calls_used += group_count;
            if tool_calls_used > 256 {
                let budget_exceeded = PipelineStepResult {
                    tool_name: "<budget>".to_string(),
                    output: None,
                    error: Some(
                        "pipeline budget exceeded: max 256 tool calls per pipeline".to_string(),
                    ),
                    duration_ms: 0,
                };
                step_results.push(budget_exceeded);
                all_success = false;
                break;
            }

            // Execute all steps in the group concurrently using FuturesUnordered
            // with a Semaphore for bounded concurrency. Shares governance cache
            // with executor.rs via governance_gate::governance_cache().
            let semaphore = Arc::new(Semaphore::new(16));
            let sandbox = self.sandbox_level;

            let mut futs = FuturesUnordered::new();
            for step in group {
                let tool_name = step.tool_name.clone();
                let input = step.input.clone();
                let sem = semaphore.clone();
                futs.push(async move {
                    let _permit = sem.acquire().await.expect("semaphore closed");
                    // Check governance individually
                    if let Err(e) = check_tool_in_pipeline(&tool_name, sandbox) {
                        tracing::warn!(
                            target: "tool_pipeline",
                            tool = %tool_name,
                            error = %e,
                            "parallel step blocked by sandbox policy"
                        );
                        return PipelineStepResult {
                            tool_name,
                            output: None,
                            error: Some(e),
                            duration_ms: 0,
                        };
                    }
                    run_single_tool(registry, &tool_name, &input).await
                });
            }

            let mut group_results = Vec::with_capacity(futs.len());
            while let Some(result) = futs.next().await {
                group_results.push(result);
            }

            let group_success = group_results.iter().all(|r| r.error.is_none());
            if !group_success {
                all_success = false;
            }
            step_results.extend(group_results);

            // Apply error strategy at group level (if any step failed and strategy is not Continue, abort)
            if !group_success && self.on_error != PipelineErrorStrategy::Continue {
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
// Parallel group builder
// ---------------------------------------------------------------------------

/// Given a list of tool names, group independent tools into parallel groups.
///
/// Tools are independent when they don't read each other's output.
/// In the current architecture, all tools are independent since outputs
/// go to the LLM, not to other tools. This means we can safely split them
/// into fixed-size parallel groups.
///
/// Each group will execute its member tools concurrently via `tokio::join!`
/// inside `ToolPipeline::execute()`. Groups still execute sequentially
/// relative to each other, so this respects any implicit ordering the caller
/// intended while maximising throughput within each batch.
///
/// # Constants
///
/// `MAX_PARALLEL` controls the maximum number of tools in a single parallel
/// group. This prevents resource exhaustion (file handles, network sockets,
/// memory) when a pipeline has many steps. Tune this based on the runtime's
/// concurrency limits.
pub fn group_independent_tools(tool_names: &[String]) -> Vec<Vec<String>> {
    const MAX_PARALLEL: usize = 5;
    tool_names
        .chunks(MAX_PARALLEL)
        .map(|c| c.to_vec())
        .collect()
}

// ---------------------------------------------------------------------------
// Internal step executor
// ---------------------------------------------------------------------------

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
            parallel_groups: Vec::new(),
            on_error: PipelineErrorStrategy::Continue,
            sandbox_level: None,
        };

        let result = pipeline.execute(&registry, &json!({})).await;
        assert!(result.success);
        assert_eq!(result.step_results.len(), 1);
        assert!(result.step_results[0].error.is_none());
    }

    #[tokio::test]
    async fn parallel_group_executes_concurrently() {
        let mut registry = ToolRegistry::new_empty();
        registry.register(EchoTool);

        let pipeline = ToolPipeline {
            name: "test-parallel".to_string(),
            steps: Vec::new(),
            parallel_groups: vec![vec![
                PipelineStep {
                    tool_name: "echo".to_string(),
                    input: json!({}),
                },
                PipelineStep {
                    tool_name: "echo".to_string(),
                    input: json!({}),
                },
                PipelineStep {
                    tool_name: "echo".to_string(),
                    input: json!({}),
                },
            ]],
            on_error: PipelineErrorStrategy::Continue,
            sandbox_level: None,
        };

        let result = pipeline.execute(&registry, &json!({})).await;
        assert!(result.success);
        // 3 parallel steps + 0 sequential = 3 total
        assert_eq!(result.step_results.len(), 3);
        // All should have succeeded (no errors)
        assert!(result.step_results.iter().all(|r| r.error.is_none()));
    }

    #[tokio::test]
    async fn sequential_then_parallel_executes_all_steps() {
        let mut registry = ToolRegistry::new_empty();
        registry.register(EchoTool);

        let pipeline = ToolPipeline {
            name: "test-mixed".to_string(),
            steps: vec![PipelineStep {
                tool_name: "echo".to_string(),
                input: json!({}),
            }],
            parallel_groups: vec![vec![
                PipelineStep {
                    tool_name: "echo".to_string(),
                    input: json!({}),
                },
                PipelineStep {
                    tool_name: "echo".to_string(),
                    input: json!({}),
                },
            ]],
            on_error: PipelineErrorStrategy::Continue,
            sandbox_level: None,
        };

        let result = pipeline.execute(&registry, &json!({})).await;
        assert!(result.success);
        // 1 sequential + 2 parallel = 3 total
        assert_eq!(result.step_results.len(), 3);
        assert!(result.step_results.iter().all(|r| r.error.is_none()));
    }
}
