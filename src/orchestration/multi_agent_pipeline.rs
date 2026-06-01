//! Multi-Agent Pipeline — GAP-B54-003
//!
//! Orchestrates multi-agent task execution by:
//! 1. Decomposing a task into subtasks using `TaskDecomposer` (LLM or template-based)
//! 2. Dispatching subtasks to registered agents in parallel (phase by phase)
//! 3. Synthesizing results into a unified response
//!
//! Each execution phase runs in parallel using `tokio::task::JoinSet`, with
//! configurable per-subtask timeout (default 60 seconds).

use crate::agent::{Agent, AgentRegistry, AgentTaskEnvelope};
use crate::orchestration::task_decomposer::{Subtask, TaskDecomposer, TaskDecomposition};
use crate::orchestration::task_router::TaskCharacteristics;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::task::JoinSet;
use tokio::time::{timeout, Duration};

/// Result produced by executing a single subtask via an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtaskResult {
    /// ID of the subtask
    pub subtask_id: String,
    /// Whether the subtask executed successfully
    pub success: bool,
    /// The agent name that handled this subtask
    pub agent_name: String,
    /// Optional output from the agent
    pub output: Option<serde_json::Value>,
    /// Optional error message on failure
    pub error: Option<String>,
    /// Wall-clock duration in milliseconds
    pub duration_ms: u64,
}

/// Final result produced by the multi-agent pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineResult {
    /// Original task ID
    pub task_id: String,
    /// Results from each subtask
    pub subtask_results: Vec<SubtaskResult>,
    /// Merged/synthesized output combining all subtask results
    pub merged_output: serde_json::Value,
    /// Total wall-clock duration in milliseconds
    pub total_duration_ms: u64,
    /// Number of subtasks that succeeded
    pub succeeded_count: usize,
    /// Number of subtasks that failed
    pub failed_count: usize,
}

/// Strategy for assigning agents to subtasks.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum AgentAssignment {
    /// Use a specific agent by name for all subtasks
    Fixed(String),
    /// Use agents from the registry; the pipeline picks available agents
    /// round-robin across subtasks within each phase
    RoundRobin,
    /// Use only agents whose names contain any of the given substrings
    Filtered(Vec<String>),
}

/// Multi-agent pipeline for parallel task execution.
pub struct MultiAgentPipeline {
    /// Agent registry for agent lookups
    registry: Arc<AgentRegistry>,
    /// How to assign agents to subtasks
    assignment: AgentAssignment,
    /// Per-subtask timeout in seconds (default: 60)
    subtask_timeout_seconds: u64,
}

impl MultiAgentPipeline {
    /// Create a new multi-agent pipeline.
    ///
    /// # Arguments
    /// * `registry` - Agent registry for agent lookups
    /// * `assignment` - Strategy for assigning agents to subtasks
    pub fn new(registry: Arc<AgentRegistry>, assignment: AgentAssignment) -> Self {
        Self {
            registry,
            assignment,
            subtask_timeout_seconds: 60,
        }
    }

    /// Set a custom per-subtask timeout.
    #[allow(dead_code)]
    pub fn with_subtask_timeout(mut self, seconds: u64) -> Self {
        self.subtask_timeout_seconds = seconds;
        self
    }

    /// Execute a task through the multi-agent pipeline.
    ///
    /// # Arguments
    /// * `task_description` - Human-readable description of the overall task
    /// * `characteristics` - Structured task characteristics for decomposition
    /// * `llm_agent` - Optional LLM agent for AI-driven decomposition
    ///
    /// # Returns
    /// PipelineResult with per-subtask results and merged output
    pub async fn execute(
        &self,
        task_description: &str,
        characteristics: &TaskCharacteristics,
        llm_agent: Option<Arc<dyn Agent>>,
    ) -> PipelineResult {
        let start = Instant::now();

        // Step 1: Decompose the task into subtasks
        let decomposition = TaskDecomposer::decompose_with_llm(characteristics, llm_agent).await;

        let task_id = decomposition.task_id.clone();
        let mut all_subtask_results = Vec::new();

        // Step 2: Execute phases sequentially (dependencies between phases)
        for phase in &decomposition.execution_phases {
            let phase_subtasks: Vec<&Subtask> = decomposition
                .subtasks
                .iter()
                .filter(|s| phase.contains(&s.id))
                .collect();

            if phase_subtasks.is_empty() {
                continue;
            }

            // Dispatch all subtasks in this phase in parallel
            let phase_results = self
                .execute_phase(&phase_subtasks, task_description, &decomposition)
                .await;

            all_subtask_results.extend(phase_results);
        }

        let total_duration_ms = start.elapsed().as_millis() as u64;

        // Step 3: Synthesize results
        let succeeded_count = all_subtask_results.iter().filter(|r| r.success).count();
        let failed_count = all_subtask_results.iter().filter(|r| !r.success).count();
        let merged_output = Self::synthesize_results(&all_subtask_results);

        PipelineResult {
            task_id,
            subtask_results: all_subtask_results,
            merged_output,
            total_duration_ms,
            succeeded_count,
            failed_count,
        }
    }

    /// Execute all subtasks in a single phase in parallel.
    async fn execute_phase(
        &self,
        subtasks: &[&Subtask],
        task_description: &str,
        decomposition: &TaskDecomposition,
    ) -> Vec<SubtaskResult> {
        let mut join_set: JoinSet<SubtaskResult> = JoinSet::new();

        // Collect available agent names
        let agent_names = self.registry.names();
        let eligible_agents: Vec<String> = match &self.assignment {
            AgentAssignment::Fixed(name) => {
                vec![name.clone()]
            }
            AgentAssignment::RoundRobin => agent_names,
            AgentAssignment::Filtered(patterns) => agent_names
                .into_iter()
                .filter(|name| patterns.iter().any(|p| name.contains(p)))
                .collect(),
        };

        if eligible_agents.is_empty() {
            // No agents available — return failed results for all subtasks
            return subtasks
                .iter()
                .map(|subtask| SubtaskResult {
                    subtask_id: subtask.id.clone(),
                    success: false,
                    agent_name: "none".to_string(),
                    output: None,
                    error: Some("No eligible agents available in registry".to_string()),
                    duration_ms: 0,
                })
                .collect();
        }

        for (i, subtask) in subtasks.iter().enumerate() {
            let agent_name = eligible_agents[i % eligible_agents.len()].clone();
            let registry = Arc::clone(&self.registry);
            let subtask = (*subtask).clone();
            let desc = task_description.to_string();
            let task_id = decomposition.task_id.clone();
            let timeout_secs = self.subtask_timeout_seconds;

            join_set.spawn(async move {
                let sub_start = Instant::now();

                // Look up the agent
                let agent = match registry.get(&agent_name) {
                    Some(a) => a,
                    None => {
                        let err = format!("Agent '{}' not found in registry", agent_name);
                        return SubtaskResult {
                            subtask_id: subtask.id.clone(),
                            success: false,
                            agent_name,
                            output: None,
                            error: Some(err),
                            duration_ms: sub_start.elapsed().as_millis() as u64,
                        };
                    }
                };

                // Build a task envelope for this subtask
                let envelope = AgentTaskEnvelope {
                    task_id: format!("{}_{}", task_id, subtask.id),
                    phase: "execution".to_string(),
                    role: "agent".to_string(),
                    objective: format!("{} — subtask: {}", desc, subtask.description),
                    evidence: Some(format!(
                        "Subtask complexity: {}, priority: {}, dependencies: {:?}",
                        subtask.complexity, subtask.priority, subtask.dependencies
                    )),
                    constraints: Some(
                        "Complete this subtask accurately and efficiently.".to_string(),
                    ),
                    input: serde_json::json!({
                        "subtask_id": subtask.id,
                        "description": subtask.description,
                        "complexity": subtask.complexity,
                        "priority": subtask.priority,
                    }),
                };

                // Execute with timeout
                let result = timeout(Duration::from_secs(timeout_secs), async {
                    agent.run_task(envelope)
                })
                .await;

                let duration_ms = sub_start.elapsed().as_millis() as u64;

                match result {
                    Ok(Ok(task_result)) => SubtaskResult {
                        subtask_id: subtask.id,
                        success: task_result.success,
                        agent_name,
                        output: task_result.output,
                        error: task_result.error.map(|e| format!("{:?}", e)),
                        duration_ms,
                    },
                    Ok(Err(e)) => SubtaskResult {
                        subtask_id: subtask.id,
                        success: false,
                        agent_name,
                        output: None,
                        error: Some(format!("Agent error: {}", e)),
                        duration_ms,
                    },
                    Err(_elapsed) => SubtaskResult {
                        subtask_id: subtask.id,
                        success: false,
                        agent_name,
                        output: None,
                        error: Some(format!("Subtask timed out after {} seconds", timeout_secs)),
                        duration_ms,
                    },
                }
            });
        }

        // Collect all results from the join set
        let mut results = Vec::new();
        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(r) => results.push(r),
                Err(e) => {
                    // Task panicked — create a synthetic failure
                    results.push(SubtaskResult {
                        subtask_id: "unknown".to_string(),
                        success: false,
                        agent_name: "unknown".to_string(),
                        output: None,
                        error: Some(format!("Subtask panicked: {}", e)),
                        duration_ms: 0,
                    });
                }
            }
        }

        results
    }

    /// Synthesize individual subtask results into a unified merged output.
    fn synthesize_results(results: &[SubtaskResult]) -> serde_json::Value {
        let mut outputs_map: HashMap<String, serde_json::Value> = HashMap::new();
        let mut errors: Vec<serde_json::Value> = Vec::new();

        for result in results {
            if result.success {
                if let Some(ref output) = result.output {
                    outputs_map.insert(result.subtask_id.clone(), output.clone());
                } else {
                    outputs_map.insert(
                        result.subtask_id.clone(),
                        serde_json::json!({"status": "completed", "agent": result.agent_name}),
                    );
                }
            } else {
                errors.push(serde_json::json!({
                    "subtask_id": result.subtask_id,
                    "agent": result.agent_name,
                    "error": result.error,
                }));
            }
        }

        serde_json::json!({
            "subtask_outputs": outputs_map,
            "errors": errors,
            "total_subtasks": results.len(),
            "succeeded": results.iter().filter(|r| r.success).count(),
            "failed": results.iter().filter(|r| !r.success).count(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentTaskResult;
    use crate::orchestration::task_router::TaskType;

    /// Create a minimal mock agent for testing.
    struct MockAgent;

    #[async_trait::async_trait]
    impl Agent for MockAgent {
        async fn chat(
            &self,
            _messages: Vec<crate::agent::Message>,
            _principles: Option<Vec<String>>,
            _options: Option<HashMap<String, serde_json::Value>>,
            _sender: crate::agent::StreamingSender,
        ) -> crate::core::error::Result<()> {
            Ok(())
        }

        fn run_task(
            &self,
            _envelope: AgentTaskEnvelope,
        ) -> crate::core::error::Result<AgentTaskResult> {
            Ok(AgentTaskResult {
                success: true,
                output: Some(serde_json::json!({"status": "ok"})),
                error: None,
                audit_log: Some("mock execution".to_string()),
                pua_report: None,
            })
        }
    }

    #[tokio::test]
    async fn test_pipeline_executes_with_mock_agent() {
        // Register mock agent
        let mut reg = AgentRegistry::new();
        reg.register_arc("mock_agent", Arc::new(MockAgent));
        let registry = Arc::new(reg);

        let pipeline =
            MultiAgentPipeline::new(registry, AgentAssignment::Fixed("mock_agent".into()));

        let characteristics = TaskCharacteristics {
            description: "test task".to_string(),
            task_type: TaskType::BugFix,
            complexity: 2,
            required_capabilities: vec!["coding".to_string()],
            involves_multiple_modules: false,
            is_time_critical: false,
            needs_verification: true,
            has_safety_concerns: false,
        };

        let result = pipeline
            .execute("Test multi-agent execution", &characteristics, None)
            .await;

        // Should have results from the bug fix template (5 subtasks)
        assert_eq!(result.succeeded_count + result.failed_count, 5);
        // All should succeed with mock agent
        assert_eq!(result.succeeded_count, 5);
    }
}
