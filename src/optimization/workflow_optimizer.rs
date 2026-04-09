//! Workflow Optimizer: Learn and optimize execution workflows (Phase 11+)
//! Predictive Failure Handler: Predict and handle potential failures (Phase 11+)
//! Execution Optimizer: Parallel execution and critical path analysis (Phase 11+)

#![allow(dead_code)]

use crate::roles::AgentRole;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Workflow execution record for learning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowExecution {
    /// Execution ID
    pub id: String,
    /// Task type executed
    pub task_type: String,
    /// Phases executed in order
    pub phases_executed: Vec<String>,
    /// Total duration in seconds
    pub duration_seconds: u32,
    /// Whether execution succeeded
    pub success: bool,
    /// Timestamp
    pub timestamp: u64,
}

/// Workflow optimization metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowMetrics {
    /// Total executions tracked
    pub total_executions: u32,
    /// Successful executions
    pub successful_executions: u32,
    /// Average duration (seconds)
    pub avg_duration_seconds: u32,
    /// Success rate
    pub success_rate: f32,
    /// Optimal phase sequence (learned)
    pub optimal_phase_sequence: Vec<String>,
    /// Phases that can be safely skipped
    pub skippable_phases: Vec<String>,
}

/// Risk analysis for failure prediction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskAssessment {
    /// Estimated failure probability (0.0-1.0)
    pub failure_probability: f32,
    /// Key risk factors
    pub risk_factors: Vec<String>,
    /// Recommended preventive actions
    pub recommended_actions: Vec<String>,
    /// Recommended safeguard mode
    pub use_safeguard_mode: bool,
}

/// Workflow optimizer
pub struct WorkflowOptimizer {
    executions: Vec<WorkflowExecution>,
    metrics: HashMap<String, WorkflowMetrics>,
}

impl WorkflowOptimizer {
    pub fn new() -> Self {
        Self {
            executions: Vec::new(),
            metrics: HashMap::new(),
        }
    }

    /// Record a workflow execution
    pub fn record_execution(
        &mut self,
        task_type: &str,
        phases: Vec<String>,
        duration_seconds: u32,
        success: bool,
    ) {
        let execution = WorkflowExecution {
            id: format!("exec_{}", self.executions.len()),
            task_type: task_type.to_string(),
            phases_executed: phases,
            duration_seconds,
            success,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };

        self.executions.push(execution);
        self.update_metrics_for_task(task_type);
    }

    /// Get optimized phase sequence for task type
    pub fn get_optimal_phases(&self, task_type: &str) -> Option<Vec<String>> {
        self.metrics
            .get(task_type)
            .map(|m| m.optimal_phase_sequence.clone())
    }

    /// Get workflow metrics for task type
    pub fn get_metrics(&self, task_type: &str) -> Option<WorkflowMetrics> {
        self.metrics.get(task_type).cloned()
    }

    fn update_metrics_for_task(&mut self, task_type: &str) {
        let task_executions: Vec<_> = self
            .executions
            .iter()
            .filter(|e| e.task_type == task_type)
            .collect();

        if task_executions.is_empty() {
            return;
        }

        let total = task_executions.len() as u32;
        let successful = task_executions.iter().filter(|e| e.success).count() as u32;
        let avg_duration = task_executions
            .iter()
            .map(|e| e.duration_seconds as u64)
            .sum::<u64>() as u32
            / total;

        let mut metrics = WorkflowMetrics {
            total_executions: total,
            successful_executions: successful,
            avg_duration_seconds: avg_duration,
            success_rate: successful as f32 / total as f32,
            optimal_phase_sequence: Self::analyze_optimal_phases(&task_executions),
            skippable_phases: vec![],
        };

        metrics.skippable_phases = Self::identify_skippable_phases(&metrics.optimal_phase_sequence);

        self.metrics.insert(task_type.to_string(), metrics);
    }

    fn analyze_optimal_phases(executions: &[&WorkflowExecution]) -> Vec<String> {
        if executions.is_empty() {
            return vec![];
        }

        // Find the most common successful phase sequence
        let successful: Vec<_> = executions.iter().filter(|e| e.success).collect();
        if successful.is_empty() {
            return executions[0].phases_executed.clone();
        }

        successful[0].phases_executed.clone()
    }

    fn identify_skippable_phases(phases: &[String]) -> Vec<String> {
        let mut skippable = Vec::new();
        let potentially_skippable = [
            "code_review",
            "documentation",
            "performance_test",
            "security_scan",
            "integration_test",
        ];
        let critical_phases = [
            "initialization",
            "core_implementation",
            "implementation",
            "unit_test",
            "verification",
            "cleanup",
        ];
        for phase in phases {
            if potentially_skippable.contains(&phase.as_str())
                && !critical_phases.contains(&phase.as_str())
            {
                skippable.push(phase.clone());
            }
        }
        skippable
    }
}

impl Default for WorkflowOptimizer {
    fn default() -> Self {
        Self::new()
    }
}

/// Predictive failure handler
pub struct PredictiveFailureHandler;

impl PredictiveFailureHandler {
    /// Assess risk for a potential execution
    pub fn assess_risk(
        task_description: &str,
        complexity: u8,
        involves_multiple_modules: bool,
        has_safety_concerns: bool,
        recent_model_success_rate: f32,
    ) -> RiskAssessment {
        let mut risk_factors = Vec::new();
        let mut failure_probability: f32 = 0.05; // 5% baseline

        // Complexity risk
        if complexity >= 5 {
            failure_probability += 0.25;
            risk_factors.push("Very high complexity increases failure risk".to_string());
        } else if complexity >= 4 {
            failure_probability += 0.15;
            risk_factors.push("High complexity task".to_string());
        }

        // Model reliability risk
        if recent_model_success_rate < 0.7 {
            failure_probability += 0.2;
            risk_factors.push(format!(
                "Model success rate is only {:.0}%",
                recent_model_success_rate * 100.0
            ));
        }

        // Multi-module risk
        if involves_multiple_modules {
            failure_probability += 0.15;
            risk_factors
                .push("Changes span multiple modules (higher integration risk)".to_string());
        }

        // Safety risk
        if has_safety_concerns {
            failure_probability += 0.2;
            risk_factors.push("Task involves security/safety-critical operations".to_string());
        }

        // Keywords indicating risky operations
        let lower = task_description.to_lowercase();
        if lower.contains("delete")
            || lower.contains("drop")
            || lower.contains("remove")
            || lower.contains("truncate")
        {
            failure_probability += 0.15;
            risk_factors.push("High-risk destructive operation detected".to_string());
        }

        let mut recommended_actions = Vec::new();
        let use_safeguard_mode = failure_probability > 0.4;

        if use_safeguard_mode {
            recommended_actions
                .push("Use SafeGuard mode for approval at critical steps".to_string());
        }

        if complexity >= 4 {
            recommended_actions.push("Enable double-review process".to_string());
        }

        if involves_multiple_modules {
            recommended_actions.push("Run comprehensive test suite".to_string());
        }

        if recent_model_success_rate < 0.8 {
            recommended_actions.push("Consider using a more reliable model".to_string());
        }

        RiskAssessment {
            failure_probability: failure_probability.min(0.95),
            risk_factors,
            recommended_actions,
            use_safeguard_mode,
        }
    }

    /// Predict fallback strategy for potential failures
    pub fn predict_fallback_strategy(failed_role: &AgentRole, attempts: u32) -> Vec<AgentRole> {
        let mut fallback_chain = vec![failed_role.clone()];

        if attempts > 1 {
            // After multiple attempts, try different roles
            match failed_role {
                AgentRole::Coder => {
                    fallback_chain.push(AgentRole::Researcher);
                    fallback_chain.push(AgentRole::Reviewer);
                }
                AgentRole::Tester => {
                    fallback_chain.push(AgentRole::Coder);
                    fallback_chain.push(AgentRole::Reviewer);
                }
                AgentRole::Planner => {
                    fallback_chain.push(AgentRole::Researcher);
                    fallback_chain.push(AgentRole::Coder);
                }
                _ => {}
            }
        }

        fallback_chain
    }
}

/// Execution optimizer for parallel execution
pub struct ExecutionOptimizer;

impl ExecutionOptimizer {
    /// Compute critical path for execution DAG
    pub fn compute_critical_path(
        subtasks: &[(String, u32, Vec<String>)], // (id, duration, dependencies)
    ) -> Vec<String> {
        let mut critical_path = Vec::new();

        if subtasks.is_empty() {
            return critical_path;
        }

        // Find tasks with no dependencies (startpoints)
        for (id, _, deps) in subtasks {
            if deps.is_empty() {
                critical_path.push(id.clone());
                break;
            }
        }

        // Simple greedy approach: always follow highest duration path
        while let Some(current) = critical_path.last() {
            let mut next_task = None;
            let mut max_duration = 0;

            for (id, duration, deps) in subtasks {
                if deps.contains(current) && !critical_path.contains(id) && *duration > max_duration
                {
                    max_duration = *duration;
                    next_task = Some(id.clone());
                }
            }

            if let Some(next) = next_task {
                critical_path.push(next);
            } else {
                break;
            }
        }

        critical_path
    }

    /// Identify parallelizable task groups
    pub fn identify_parallel_groups(subtasks: &[(String, u32, Vec<String>)]) -> Vec<Vec<String>> {
        let mut groups = Vec::new();
        let mut completed = std::collections::HashSet::new();

        loop {
            let mut current_group = Vec::new();

            for (id, _, deps) in subtasks {
                if completed.contains(id) {
                    continue;
                }

                // Can run in parallel if all deps are completed
                if deps.iter().all(|dep| completed.contains(dep)) {
                    current_group.push(id.clone());
                }
            }

            if current_group.is_empty() {
                break;
            }

            for id in &current_group {
                completed.insert(id.clone());
            }

            groups.push(current_group);
        }

        groups
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workflow_metrics() {
        let mut optimizer = WorkflowOptimizer::new();
        optimizer.record_execution("bug_fix", vec!["analyze".to_string()], 100, true);
        optimizer.record_execution("bug_fix", vec!["analyze".to_string()], 120, true);

        let metrics = optimizer.get_metrics("bug_fix").unwrap();
        assert_eq!(metrics.total_executions, 2);
        assert_eq!(metrics.successful_executions, 2);
    }

    #[test]
    fn test_risk_assessment() {
        let assessment =
            PredictiveFailureHandler::assess_risk("Simple change", 1, false, false, 0.95);
        assert!(assessment.failure_probability < 0.15);

        let assessment =
            PredictiveFailureHandler::assess_risk("Complex delete operation", 5, true, true, 0.5);
        assert!(assessment.use_safeguard_mode);
    }

    #[test]
    fn test_parallelization() {
        let subtasks = vec![
            ("task1".to_string(), 100u32, vec![]),
            ("task2".to_string(), 100, vec!["task1".to_string()]),
            ("task3".to_string(), 100, vec!["task1".to_string()]),
        ];

        let groups = ExecutionOptimizer::identify_parallel_groups(&subtasks);
        assert_eq!(groups[0], vec!["task1"]);
        assert_eq!(groups[1].len(), 2); // task2 and task3 can run in parallel
    }
}
