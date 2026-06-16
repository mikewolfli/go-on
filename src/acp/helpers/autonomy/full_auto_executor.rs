//! Full-auto executor pipeline stage.
//!
//! Provides the `FullAutoExecutor` which discovers and executes skills
//! for a given task in a fully automated fashion. This bridges the
//! `orchestration::full_auto::FullAutoFlow` with the ACP autonomy
//! pipeline via `run_full_auto_flow` in `autonomy_loop_adapter.rs`.
//!
//! The executor:
//!   1. Accepts a task description
//!   2. Discovers matching skills from the SkillRegistry
//!   3. Executes each matched skill in priority order
//!   4. Collects results, errors, and execution timing into a report

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tracing::{info, warn};

use crate::orchestration::full_auto::FullAutoFlow;
use crate::orchestration::skill::SkillRegistry;
use crate::orchestration::tool::ToolRegistry;

/// Configuration for the full-auto executor.
///
/// Ready for integration into `run_full_auto_execution` in `chat.rs`.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone)]
pub struct FullAutoExecutorConfig {
    /// Whether to enable fast-path cache for repeated tasks.
    pub enable_fast_path: bool,
    /// Whether to enable threshold learning for adaptive skill matching.
    pub enable_threshold_learning: bool,
    /// Whether to enable the skill market integration.
    pub enable_skill_market: bool,
    /// Timeout for the entire execution flow.
    pub flow_timeout: Duration,
    /// Maximum number of skills to execute.
    pub max_skills: usize,
}

impl Default for FullAutoExecutorConfig {
    fn default() -> Self {
        Self {
            enable_fast_path: true,
            enable_threshold_learning: false,
            enable_skill_market: false,
            flow_timeout: Duration::from_secs(300),
            max_skills: 10,
        }
    }
}

/// A single step in the full-auto execution log.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone)]
pub struct ExecutionStep {
    /// Name of the skill that was executed.
    #[allow(dead_code)]
    pub skill_name: String,
    /// Whether execution succeeded.
    #[allow(dead_code)]
    pub success: bool,
    /// Error message (on failure).
    #[allow(dead_code)]
    pub error: Option<String>,
    /// Duration of this step.
    #[allow(dead_code)]
    pub duration_ms: u64,
}

/// Result of a full-auto execution flow.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone)]
pub struct FullAutoExecutionResult {
    /// Number of skills successfully executed.
    pub success_count: usize,
    /// Number of skills that failed.
    #[allow(dead_code)]
    pub failure_count: usize,
    /// Whether all skills executed successfully.
    #[allow(dead_code)]
    pub is_success: bool,
    /// Execution steps.
    #[allow(dead_code)]
    pub steps: Vec<ExecutionStep>,
    /// Total duration.
    #[allow(dead_code)]
    pub total_duration_ms: u64,
    /// Final output summary.
    #[allow(dead_code)]
    pub output: String,
}

/// Executes tasks in full-auto mode by discovering and running skills.
///
/// Ready for integration into `run_full_auto_execution` in `chat.rs`.
#[cfg_attr(not(test), allow(dead_code))]
pub struct FullAutoExecutor {
    config: FullAutoExecutorConfig,
}

#[cfg_attr(not(test), allow(dead_code))]
impl FullAutoExecutor {
    /// Create a new full-auto executor.
    pub fn new(config: FullAutoExecutorConfig) -> Self {
        Self { config }
    }

    /// Execute a task in full-auto mode.
    ///
    /// Creates a `FullAutoFlow` with the given registries, runs it against
    /// the task text, and returns a structured result.
    pub async fn execute(
        &self,
        skill_registry: Arc<Mutex<SkillRegistry>>,
        tool_registry: Arc<ToolRegistry>,
        task_text: &str,
    ) -> FullAutoExecutionResult {
        let start = std::time::Instant::now();
        info!("FullAutoExecutor: starting flow for task ({} chars)", task_text.len());

        let mut flow = FullAutoFlow::new(skill_registry, tool_registry);

        if self.config.enable_skill_market {
            flow.enable_skill_market();
        }

        match tokio::time::timeout(self.config.flow_timeout, flow.run(task_text)).await {
            Ok(report) => {
                let success_count = report.success_count();
                let failure_count = report.failure_count();
                let is_success = report.is_success();

                let steps: Vec<ExecutionStep> = report
                    .execution_log
                    .iter()
                    .map(|step| ExecutionStep {
                        skill_name: step.skill_name.clone(),
                        success: step.success,
                        error: step.error.clone(),
                        duration_ms: step.duration_ms,
                    })
                    .collect();

                info!(
                    "FullAutoExecutor: flow completed — {} success, {} failure in {}ms",
                    success_count,
                    failure_count,
                    start.elapsed().as_millis()
                );

                FullAutoExecutionResult {
                    success_count,
                    failure_count,
                    is_success,
                    steps,
                    total_duration_ms: start.elapsed().as_millis() as u64,
                    output: report.final_output.unwrap_or_default(),
                }
            }
            Err(_elapsed) => {
                warn!("FullAutoExecutor: flow timed out after {}s", self.config.flow_timeout.as_secs());
                FullAutoExecutionResult {
                    success_count: 0,
                    failure_count: 0,
                    is_success: false,
                    steps: Vec::new(),
                    total_duration_ms: start.elapsed().as_millis() as u64,
                    output: "Flow timed out".to_string(),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_executor_config_defaults() {
        let config = FullAutoExecutorConfig::default();
        assert!(config.enable_fast_path);
        assert!(!config.enable_threshold_learning);
        assert_eq!(config.max_skills, 10);
    }

    #[tokio::test]
    async fn test_execute_with_empty_registries() {
        let config = FullAutoExecutorConfig::default();
        let executor = FullAutoExecutor::new(config);

        let skill_registry = Arc::new(Mutex::new(SkillRegistry::default()));
        let tool_registry = Arc::new(ToolRegistry::new_empty());

        let result = executor
            .execute(skill_registry, tool_registry, "fix the login bug")
            .await;

        // With empty registries, the flow should complete without errors
        // but with zero successes (no matching skills).
        assert_eq!(result.success_count, 0);
    }
}
