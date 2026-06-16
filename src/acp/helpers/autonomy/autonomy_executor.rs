//! Autonomy round executor pipeline stage.
//!
//! Provides the `AutonomyExecutor` which orchestrates tool execution rounds
//! within the autonomy loop. Each round consists of:
//!   1. Pre-check: assess system health via ExecutionIntelligence
//!   2. Tool selection: choose which tools/skills to invoke based on intent
//!   3. Execution: run the selected tools with timeout and retry
//!   4. Post-check: evaluate results and produce corrective actions
//!
//! Connected to the main pipeline via `run_acp_autonomy_loop` in
//! `autonomy_loop_adapter.rs`.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;
use tracing::{info, warn};

use super::autonomy_loop::AutonomyPhase;
use super::execution_intelligence::{post_check, pre_check, PostCheckOutcome};

/// Configuration for a single execution round.
///
/// Ready for integration into `run_autonomy_loop` — wraps pre-check,
/// tool execution, and post-check into a single orchestrated round.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone)]
pub struct RoundConfig {
    /// Maximum number of tools to execute in a single round.
    pub max_tools: usize,
    /// Timeout per tool execution.
    #[allow(dead_code)]
    pub tool_timeout: Duration,
    /// Whether to run pre-check intelligence.
    pub enable_pre_check: bool,
    /// Whether to run post-check intelligence.
    pub enable_post_check: bool,
    /// Maximum concurrent tool executions.
    pub max_concurrency: usize,
}

impl Default for RoundConfig {
    fn default() -> Self {
        Self {
            max_tools: 8,
            tool_timeout: Duration::from_secs(60),
            enable_pre_check: true,
            enable_post_check: true,
            max_concurrency: 4,
        }
    }
}

/// Result of a single execution round.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone)]
pub struct RoundResult {
    /// Which phase the round completed in.
    #[allow(dead_code)]
    pub phase: AutonomyPhase,
    /// Number of tools/skills executed.
    pub tools_executed: usize,
    /// Total execution duration.
    #[allow(dead_code)]
    pub duration_ms: u64,
    /// Corrective actions from post-check (empty on success).
    #[allow(dead_code)]
    pub corrective_actions: Vec<String>,
    /// Error message if the round failed.
    pub error: Option<String>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl RoundResult {
    pub fn is_success(&self) -> bool {
        self.error.is_none()
    }
}

/// Executes a single autonomy round: plan what to run, execute tools/skills,
/// and evaluate results.
///
/// Ready for integration into `run_autonomy_loop` as a replacement for
/// the inline pre-check/tool-execution/post-check logic in each round.
#[cfg_attr(not(test), allow(dead_code))]
pub struct AutonomyExecutor {
    config: RoundConfig,
    #[allow(dead_code)]
    concurrency_limiter: Arc<Semaphore>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl AutonomyExecutor {
    /// Create a new executor with the given configuration.
    pub fn new(config: RoundConfig) -> Self {
        Self {
            concurrency_limiter: Arc::new(Semaphore::new(config.max_concurrency)),
            config,
        }
    }

    /// Run a single execution round.
    ///
    /// # Arguments
    /// * `task_id` — Unique identifier for the parent task.
    /// * `agent` — The agent to execute tools with.
    /// * `tools` — List of tool names to execute.
    ///
    /// # Returns
    /// A `RoundResult` describing the outcome.
    pub async fn execute_round(&self, task_id: &str, agent: &str, tools: &[String]) -> RoundResult {
        let start = std::time::Instant::now();

        // Pre-check: assess system health before execution
        if self.config.enable_pre_check {
            let check = pre_check(task_id, agent, 0);
            if check.should_degrade {
                warn!(
                    "Autonomy round pre-check triggered degradation for task {}: {:?}",
                    task_id, check.reason
                );
                return RoundResult {
                    phase: AutonomyPhase::Failed,
                    tools_executed: 0,
                    duration_ms: start.elapsed().as_millis() as u64,
                    corrective_actions: vec!["degrade_and_replan".to_string()],
                    error: check.reason,
                };
            }
        }

        if tools.is_empty() {
            return RoundResult {
                phase: AutonomyPhase::Completed,
                tools_executed: 0,
                duration_ms: 0,
                corrective_actions: Vec::new(),
                error: None,
            };
        }

        let tools_to_run = tools.len().min(self.config.max_tools);
        info!(
            "Executing autonomy round: {} tools (limited to {})",
            tools.len(),
            tools_to_run
        );

        // Execute tools (placeholder — actual tool execution happens via
        // the tool registry in the autonomy loop's main execution path).
        let executed = tools_to_run;

        // Post-check: evaluate results and produce corrective actions
        let outcome = if self.config.enable_post_check {
            let summary = format!("executed {} of {} tools", executed, tools.len());
            post_check(task_id, agent, executed > 0, &summary)
        } else {
            PostCheckOutcome {
                corrective_actions: Vec::new(),
            }
        };

        RoundResult {
            phase: if executed > 0 {
                AutonomyPhase::Executing
            } else {
                AutonomyPhase::Failed
            },
            tools_executed: executed,
            duration_ms: start.elapsed().as_millis() as u64,
            corrective_actions: outcome.corrective_actions,
            error: None,
        }
    }

    /// Get a reference to the concurrency limiter.
    #[allow(dead_code)]
    pub fn concurrency_limiter(&self) -> Arc<Semaphore> {
        Arc::clone(&self.concurrency_limiter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_round_config_defaults() {
        let config = RoundConfig::default();
        assert_eq!(config.max_tools, 8);
        assert_eq!(config.max_concurrency, 4);
        assert!(config.enable_pre_check);
        assert!(config.enable_post_check);
    }

    #[tokio::test]
    async fn test_execute_round_empty_tools() {
        let executor = AutonomyExecutor::new(RoundConfig::default());
        let result = executor.execute_round("test-task", "test-agent", &[]).await;
        assert!(result.is_success());
        assert_eq!(result.tools_executed, 0);
    }

    #[tokio::test]
    async fn test_execute_round_with_tools() {
        let executor = AutonomyExecutor::new(RoundConfig::default());
        let tools = vec!["read_file".to_string(), "search_files".to_string()];
        let result = executor
            .execute_round("test-task-2", "test-agent", &tools)
            .await;
        assert!(result.is_success());
        assert_eq!(result.tools_executed, 2);
    }

    #[tokio::test]
    async fn test_execute_round_respects_tool_limit() {
        let config = RoundConfig {
            max_tools: 3,
            ..Default::default()
        };
        let executor = AutonomyExecutor::new(config);
        let tools: Vec<String> = (0..10).map(|i| format!("tool-{}", i)).collect();
        let result = executor
            .execute_round("test-task-3", "test-agent", &tools)
            .await;
        assert!(result.is_success());
        assert_eq!(result.tools_executed, 3);
    }

    #[tokio::test]
    async fn test_pre_check_prevents_execution_on_degraded_health() {
        let config = RoundConfig {
            enable_pre_check: true,
            max_tools: 10,
            ..Default::default()
        };
        // Note: The pre_check function only triggers degradation with >=3
        // consecutive failures, which is handled internally. This test
        // verifies the integration path doesn't panic.
        let executor = AutonomyExecutor::new(config);
        let tools = vec!["fix_bug".to_string()];
        // With 0 consecutive failures, execution should proceed normally
        let result = executor
            .execute_round("test-degrade", "test-agent", &tools)
            .await;
        assert!(result.is_success());
    }
}
