//! Think-Act-Observe tool execution loop
//!
//! Full Think → Act → Observe orchestration loop (F-GAP-01):
//!
//! 1. Think:   Analyze task context, select the best tool candidate
//! 2. Act:     Execute tool call with fallback-chain support
//! 3. Observe: Validate output, decide next action (continue / retry /
//!    switch tool / complete / escalate)
//!
//! Loop termination:
//! - Tool succeeds and output verification passes
//! - All tool candidates exhausted (retry + fallback limits reached)
//! - Maximum iteration count reached

use crate::orchestration::tool::recommender;
use crate::orchestration::tool::{ToolInput, ToolOutput, ToolRegistry};
use anyhow::Result;
use glob::Pattern;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokio::task::block_in_place;
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

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

/// Result of a single iteration's observe phase — tells the caller what to do next.
#[derive(Debug)]
enum IterationAction {
    /// Continue to the next iteration.
    Continue,
    /// Tool completed successfully.
    Complete(ToolOutput),
    /// All candidates exhausted.
    Failed {
        reason: String,
        last_output: Option<ToolOutput>,
    },
    /// Escalate to human review.
    Escalate { reason: String, output: ToolOutput },
}

// ---------------------------------------------------------------------------
// File helpers
// ---------------------------------------------------------------------------

/// Recursively walk a directory tree and collect files matching the given
/// glob [`Pattern`]. Returns their full paths.
pub fn collect_matching_files(
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

/// Recursively walk a directory tree using `tokio::fs` and collect files
/// matching the given glob pattern. Returns their full paths.
pub async fn collect_matching_files_async(root: PathBuf, matcher: Pattern) -> Result<Vec<String>> {
    let mut files = Vec::new();
    let mut dirs_to_visit = vec![root.clone()];

    while let Some(dir) = dirs_to_visit.pop() {
        let mut rd = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = rd.next_entry().await? {
            let path = entry.path();
            if entry.file_type().await?.is_dir() {
                dirs_to_visit.push(path);
            } else {
                let relative = path.strip_prefix(&root).unwrap_or(&path);
                let candidate = relative.to_string_lossy().replace('\\', "/");
                if matcher.matches(&candidate) || matcher.matches_path(relative) {
                    files.push(path.to_string_lossy().to_string());
                }
            }
        }
    }

    Ok(files)
}

// ---------------------------------------------------------------------------
// Think / Observe / Handle helpers
// ---------------------------------------------------------------------------

/// Think phase: select the best tool candidate.
///
/// Selection strategy (in order of priority):
/// 1. If a `ToolRecommender` is available, consult it for task-based recommendations
///    and pick the highest-scoring candidate.
/// 2. Match tool names from keywords in the task description.
/// 3. Fall back to the tool with the fewest retries.
///
/// Returns `None` if no candidates are available.
fn think(
    task: &str,
    candidates: &[String],
    retry_counts: &HashMap<String, u32>,
    config: &LoopConfig,
    recommender: Option<&recommender::ToolRecommender>,
) -> Option<ThinkResult> {
    if candidates.is_empty() {
        return None;
    }

    // Phase 1: consult the ToolRecommender when available
    if let Some(rec) = recommender {
        let context: Vec<String> = Vec::new();
        let recommendations = rec.recommend(task, &context);
        if !recommendations.is_empty() {
            // Find the highest-scored recommendation that is in our candidate list
            // and hasn't exhausted its retries.
            for rec_candidate in &recommendations {
                if candidates.contains(&rec_candidate.tool_name) {
                    let retries = retry_counts
                        .get(&rec_candidate.tool_name)
                        .copied()
                        .unwrap_or(0);
                    if retries < config.max_retries_per_tool {
                        let confidence = (rec_candidate.relevance_score.min(1.0)
                            * (1.0
                                - (retries as f64 / config.max_retries_per_tool as f64).min(1.0)))
                        .max(0.1);
                        return Some(ThinkResult {
                            tool: rec_candidate.tool_name.clone(),
                            confidence,
                            rationale: format!(
                                "recommender task=\"{}\" tool={} score={:.3} retries={} reason={}",
                                task,
                                rec_candidate.tool_name,
                                rec_candidate.relevance_score,
                                retries,
                                rec_candidate.reason,
                            ),
                        });
                    }
                }
            }
        }
    }

    // Phase 2: try to match tool names from task description keywords
    if !task.is_empty() {
        let task_lower = task.to_lowercase();
        for candidate in candidates {
            if task_lower.contains(&candidate.to_lowercase()) {
                let retries = retry_counts.get(candidate).copied().unwrap_or(0);
                let confidence =
                    1.0 - (retries as f64 / config.max_retries_per_tool as f64).min(1.0);
                return Some(ThinkResult {
                    tool: candidate.clone(),
                    confidence,
                    rationale: format!(
                        "keyword_match task=\"{}\" tool={} retries={}",
                        task, candidate, retries,
                    ),
                });
            }
        }
    }

    // Phase 3: fall back to the tool with fewest retries
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

/// Shared post-Act phase: record the result, observe the output, and decide
/// the next action. Called by both `execute_loop` and `execute_loop_async`
/// to avoid duplicating the observe-and-match logic.
#[allow(clippy::too_many_arguments)]
fn handle_iteration(
    task: &str,
    trace: &mut LoopTrace,
    start: Instant,
    iteration: u32,
    tr: &ThinkResult,
    output: ToolOutput,
    act_duration_ms: u64,
    config: &LoopConfig,
    retry_counts: &mut HashMap<String, u32>,
) -> IterationAction {
    // Record the act phase in the trace.
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
    let observe_decision = observe(&output, &tr.tool, retry_counts, config, |tool, reason| {
        trace.iterations.push(LoopIteration {
            stage: "observe".to_string(),
            tool,
            success: false,
            duration_ms: 0,
            detail: reason,
        });
    });

    match observe_decision {
        LoopDecision::Continue => {
            trace.iterations.push(LoopIteration {
                stage: "think".to_string(),
                tool: tr.tool.clone(),
                success: true,
                duration_ms: 0,
                detail: "output ok, continuing".to_string(),
            });
            IterationAction::Continue
        }
        LoopDecision::Retry { tool, reason } => {
            trace.iterations.push(LoopIteration {
                stage: "think".to_string(),
                tool: tool.clone(),
                success: false,
                duration_ms: 0,
                detail: format!("retry: {}", reason),
            });
            IterationAction::Continue
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
            IterationAction::Continue
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
            IterationAction::Complete(output)
        }
        LoopDecision::Failed {
            reason,
            last_output,
        } => {
            trace.final_decision = "failed".to_string();
            trace.total_duration_ms = start.elapsed().as_millis() as u64;
            warn!(task, reason, "TAO: failed");
            IterationAction::Failed {
                reason,
                last_output,
            }
        }
        LoopDecision::Escalate { reason, output } => {
            trace.final_decision = "escalated".to_string();
            trace.total_duration_ms = start.elapsed().as_millis() as u64;
            warn!(task, reason, "TAO: escalated");
            IterationAction::Escalate { reason, output }
        }
    }
}

// ---------------------------------------------------------------------------
// Main loop entry points
// ---------------------------------------------------------------------------

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
/// NOTE: Prefer `execute_loop_async` for new code. This sync variant
/// is kept for backward compatibility in test environments where
/// an async runtime is not available.
pub fn execute_loop(
    task: &str,
    registry: &ToolRegistry,
    input: &ToolInput,
    preferred_tools: &[String],
    config: &LoopConfig,
    mut recommender: Option<&mut recommender::ToolRecommender>,
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
        let think_result = think(
            task,
            &tool_candidates,
            &retry_counts,
            config,
            recommender.as_deref(),
        );

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

        // Record usage statistics with the recommender when available.
        if let Some(rec) = &mut recommender {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            rec.record_usage(&tr.tool, output.success, act_duration_ms, now, &[]);
        }

        // ── Observe ──────────────────────────────────────────────
        match handle_iteration(
            task,
            &mut trace,
            start,
            iteration,
            &tr,
            output,
            act_duration_ms,
            config,
            &mut retry_counts,
        ) {
            IterationAction::Continue => continue,
            IterationAction::Complete(output) => {
                return (LoopDecision::Complete(output), trace);
            }
            IterationAction::Failed {
                reason,
                last_output,
            } => {
                return (
                    LoopDecision::Failed {
                        reason,
                        last_output,
                    },
                    trace,
                );
            }
            IterationAction::Escalate { reason, output } => {
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

/// Async version of `execute_loop`.
///
/// Has the exact same Think → Act → Observe logic as the synchronous version,
/// but executes tools via `run_with_fallback_async().await` instead of
/// `run_with_fallback()`, so it does not block the async runtime.
pub async fn execute_loop_async(
    task: &str,
    registry: &ToolRegistry,
    input: &ToolInput,
    preferred_tools: &[String],
    config: &LoopConfig,
    mut recommender: Option<&mut recommender::ToolRecommender>,
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
        let think_result = block_in_place(|| {
            think(
                task,
                &tool_candidates,
                &retry_counts,
                config,
                recommender.as_deref(),
            )
        });

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
                .run_with_fallback_async(&tr.tool, input)
                .await
                .unwrap_or_else(|e| ToolOutput {
                    success: false,
                    result: None,
                    error: Some(format!("tool '{}' error: {}", tr.tool, e)),
                    verification: None,
                    audit_log: None,
                    pua_report: None,
                })
        } else {
            match registry.get_arc(&tr.tool) {
                None => ToolOutput {
                    success: false,
                    result: None,
                    error: Some(format!("tool '{}' not found", tr.tool)),
                    verification: None,
                    audit_log: None,
                    pua_report: None,
                },
                Some(tool) => tool
                    .run_async(input.clone())
                    .await
                    .unwrap_or_else(|e| ToolOutput {
                        success: false,
                        result: None,
                        error: Some(format!("{}", e)),
                        verification: None,
                        audit_log: None,
                        pua_report: None,
                    }),
            }
        };
        let act_duration_ms = act_start.elapsed().as_millis() as u64;

        // Record usage statistics with the recommender when available.
        if let Some(rec) = &mut recommender {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            rec.record_usage(&tr.tool, output.success, act_duration_ms, now, &[]);
        }

        // ── Observe ──────────────────────────────────────────────
        match handle_iteration(
            task,
            &mut trace,
            start,
            iteration,
            &tr,
            output,
            act_duration_ms,
            config,
            &mut retry_counts,
        ) {
            IterationAction::Continue => continue,
            IterationAction::Complete(output) => {
                return (LoopDecision::Complete(output), trace);
            }
            IterationAction::Failed {
                reason,
                last_output,
            } => {
                return (
                    LoopDecision::Failed {
                        reason,
                        last_output,
                    },
                    trace,
                );
            }
            IterationAction::Escalate { reason, output } => {
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
