//! Advanced enhancement modules: Parameter Tuner, Resource Allocator, Diagnostics, Learning

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Dynamic parameter configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicParameters {
    pub max_tool_calls: usize,
    pub timeout_seconds: u64,
    pub temperature: f32,
    pub approval_required: bool,
    pub max_retries: u32,
    pub context_window_size: usize,
}

/// Parameter tuner that adapts parameters based on task characteristics
pub struct DynamicParameterTuner {
    profiles: HashMap<String, DynamicParameters>,
}

impl DynamicParameterTuner {
    pub fn new() -> Self {
        Self {
            profiles: Self::initialize_profiles(),
        }
    }

    fn initialize_profiles() -> HashMap<String, DynamicParameters> {
        let mut profiles = HashMap::new();

        // Simple tasks
        profiles.insert(
            "simple".to_string(),
            DynamicParameters {
                max_tool_calls: 5,
                timeout_seconds: 30,
                temperature: 0.3,
                approval_required: false,
                max_retries: 1,
                context_window_size: 2048,
            },
        );

        // Medium complexity
        profiles.insert(
            "medium".to_string(),
            DynamicParameters {
                max_tool_calls: 20,
                timeout_seconds: 120,
                temperature: 0.5,
                approval_required: false,
                max_retries: 2,
                context_window_size: 4096,
            },
        );

        // Complex tasks
        profiles.insert(
            "complex".to_string(),
            DynamicParameters {
                max_tool_calls: 50,
                timeout_seconds: 300,
                temperature: 0.7,
                approval_required: true,
                max_retries: 3,
                context_window_size: 8192,
            },
        );

        // Code generation
        profiles.insert(
            "code_generation".to_string(),
            DynamicParameters {
                max_tool_calls: 30,
                timeout_seconds: 180,
                temperature: 0.3,
                approval_required: false,
                max_retries: 2,
                context_window_size: 6144,
            },
        );

        // Architectural design
        profiles.insert(
            "architecture".to_string(),
            DynamicParameters {
                max_tool_calls: 50,
                timeout_seconds: 600,
                temperature: 0.8,
                approval_required: true,
                max_retries: 3,
                context_window_size: 8192,
            },
        );

        profiles
    }

    /// Get parameters for task type and complexity
    pub fn select_parameters(&self, task_type: &str, complexity: u8) -> DynamicParameters {
        // Look for specific task type first
        if let Some(params) = self.profiles.get(task_type) {
            return params.clone();
        }

        // Fall back to complexity-based selection
        match complexity {
            1 => self.profiles.get("simple").cloned().unwrap_or_else(|| {
                tracing::warn!("DynamicParameterTuner: missing 'simple' profile, using default");
                DynamicParameters {
                    max_tool_calls: 5,
                    timeout_seconds: 30,
                    temperature: 0.3,
                    approval_required: false,
                    max_retries: 1,
                    context_window_size: 2048,
                }
            }),
            2 | 3 => self.profiles.get("medium").cloned().unwrap_or_else(|| {
                tracing::warn!("DynamicParameterTuner: missing 'medium' profile, using default");
                DynamicParameters {
                    max_tool_calls: 20,
                    timeout_seconds: 120,
                    temperature: 0.5,
                    approval_required: false,
                    max_retries: 2,
                    context_window_size: 4096,
                }
            }),
            4 | 5 => self.profiles.get("complex").cloned().unwrap_or_else(|| {
                tracing::warn!("DynamicParameterTuner: missing 'complex' profile, using default");
                DynamicParameters {
                    max_tool_calls: 50,
                    timeout_seconds: 300,
                    temperature: 0.7,
                    approval_required: true,
                    max_retries: 3,
                    context_window_size: 8192,
                }
            }),
            _ => self.profiles.get("medium").cloned().unwrap_or_else(|| {
                tracing::warn!(
                    "DynamicParameterTuner: missing fallback 'medium' profile, using default"
                );
                DynamicParameters {
                    max_tool_calls: 20,
                    timeout_seconds: 120,
                    temperature: 0.5,
                    approval_required: false,
                    max_retries: 2,
                    context_window_size: 4096,
                }
            }),
        }
    }

    /// Tune parameters based on execution history
    pub fn tune_parameters(&mut self, task_type: &str, success_rate: f32, avg_duration: u64) {
        if let Some(params) = self.profiles.get_mut(task_type) {
            // If taking too long, reduce tool calls
            if avg_duration > params.timeout_seconds / 2 {
                params.max_tool_calls = (params.max_tool_calls as f32 * 0.8) as usize;
            }

            // If failing frequently, increase timeout or approval
            if success_rate < 0.7 {
                params.timeout_seconds = (params.timeout_seconds as f32 * 1.2) as u64;
                params.approval_required = true;
            }

            // If succeeding too easily, can optimize
            if success_rate > 0.95 {
                params.max_tool_calls = (params.max_tool_calls as f32 * 0.9) as usize;
            }
        }
    }
}

impl Default for DynamicParameterTuner {
    fn default() -> Self {
        Self::new()
    }
}

/// Resource budget for task execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceBudget {
    pub token_budget: usize,
    pub time_budget_seconds: u64,
    pub api_cost_limit_cents: u32,
    pub max_parallel_tasks: usize,
}

/// Resource allocator
pub struct ResourceAllocator;

impl ResourceAllocator {
    /// Allocate resources based on task complexity and requirements
    /// Usage:
    /// let budget = ResourceAllocator::allocate_resources("feature", 3, 4);
    /// assert!(budget.token_budget > 0);
    ///
    /// # Panics
    /// Never panics; clamps and logs on invalid input.
    #[tracing::instrument(level = "debug", skip(_task_type))]
    pub fn allocate_resources(
        _task_type: &str,
        complexity: u8,
        num_subtasks: usize,
    ) -> ResourceBudget {
        let base_tokens = 2000u32;
        let base_time = 60u64;
        let base_cost = 100u32; // cents

        let complexity_multiplier = (complexity as f32) / 2.5;
        if complexity == 0 {
            tracing::warn!("ResourceAllocator: complexity=0, using minimum allocation");
        }
        if num_subtasks == 0 {
            tracing::warn!("ResourceAllocator: num_subtasks=0, using minimum parallelism");
        }

        ResourceBudget {
            token_budget: (base_tokens as f32 * complexity_multiplier.max(0.4)) as usize,
            time_budget_seconds: (base_time as f32 * complexity_multiplier.max(0.4)) as u64,
            api_cost_limit_cents: (base_cost as f32 * complexity_multiplier.max(0.4)) as u32,
            max_parallel_tasks: (num_subtasks / 2).clamp(1, 8),
        }
    }

    /// Check if resource usage is within budget
    pub fn check_budget(
        budget: &ResourceBudget,
        tokens_used: usize,
        time_used: u64,
        cost_used: u32,
    ) -> bool {
        tokens_used <= budget.token_budget
            && time_used <= budget.time_budget_seconds
            && cost_used <= budget.api_cost_limit_cents
    }

    /// Calculate remaining resources
    pub fn remaining_resources(
        budget: &ResourceBudget,
        tokens_used: usize,
        time_used: u64,
        cost_used: u32,
    ) -> ResourceBudget {
        ResourceBudget {
            token_budget: budget.token_budget.saturating_sub(tokens_used),
            time_budget_seconds: budget.time_budget_seconds.saturating_sub(time_used),
            api_cost_limit_cents: budget.api_cost_limit_cents.saturating_sub(cost_used),
            max_parallel_tasks: budget.max_parallel_tasks,
        }
    }
}

/// Workflow diagnostics and visualization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionDiagnostics {
    pub task_id: String,
    pub start_time: u64,
    pub end_time: u64,
    pub total_duration_ms: u64,
    pub phase_durations: HashMap<String, u64>,
    pub bottleneck_phase: Option<String>,
    pub resource_utilization: f32,
    pub efficiency_score: f32, // 0-1
}

/// Workflow diagnostics system
pub struct WorkflowDiagnostics;

impl WorkflowDiagnostics {
    /// Generate diagnostics report
    /// Usage:
    /// let diag = WorkflowDiagnostics::generate_report("task1", &[("phase1".to_string(), 100)], &budget, (100, 10, 1));
    /// assert!(diag.total_duration_ms > 0);
    ///
    /// # Panics
    /// Never panics; logs and uses default on time errors.
    #[tracing::instrument(level = "debug", skip(phases, resource_budget))]
    pub fn generate_report(
        task_id: &str,
        phases: &[(String, u64)], // (phase_name, duration_ms)
        resource_budget: &ResourceBudget,
        resources_used: (usize, u64, u32),
    ) -> ExecutionDiagnostics {
        let total_duration_ms: u64 = phases.iter().map(|(_, d)| d).sum();

        let (tokens_used, _time_used, _cost_used) = resources_used;
        let resource_utilization = if resource_budget.token_budget > 0 {
            (tokens_used as f32 / resource_budget.token_budget as f32).min(1.0)
        } else {
            tracing::warn!("WorkflowDiagnostics: token_budget=0, utilization forced to 1.0");
            1.0
        };

        // Find bottleneck
        let bottleneck_phase = phases
            .iter()
            .max_by_key(|(_, d)| d)
            .map(|(name, _)| name.clone());

        let efficiency_score = if total_duration_ms > 0 && resource_budget.time_budget_seconds > 0 {
            (1.0 - (total_duration_ms as f32 / (resource_budget.time_budget_seconds * 1000) as f32))
                .max(0.0)
        } else {
            tracing::warn!(
                "WorkflowDiagnostics: time_budget_seconds=0 or total_duration_ms=0, efficiency=0"
            );
            0.0
        };

        let mut phase_durations = HashMap::new();
        for (phase, duration) in phases {
            phase_durations.insert(phase.clone(), *duration);
        }

        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
        let now_secs = match now {
            Ok(d) => d.as_secs(),
            Err(e) => {
                tracing::error!("SystemTime error: {:?}", e);
                0
            }
        };

        ExecutionDiagnostics {
            task_id: task_id.to_string(),
            start_time: now_secs,
            end_time: now_secs,
            total_duration_ms,
            phase_durations,
            bottleneck_phase,
            resource_utilization,
            efficiency_score,
        }
    }
    // dead_code 检查：本模块所有结构体/方法均有测试覆盖或主流程调用，若后续移除请同步清理�?

    /// Generate optimization recommendations
    pub fn recommend_optimizations(diagnostics: &ExecutionDiagnostics) -> Vec<String> {
        let mut recommendations = Vec::new();

        if let Some(bottleneck) = &diagnostics.bottleneck_phase {
            recommendations.push(format!("Optimize '{}' phase (bottleneck)", bottleneck));
        }

        if diagnostics.resource_utilization > 0.9 {
            recommendations
                .push("Current resource limits may be tight - consider increasing".to_string());
        }

        if diagnostics.efficiency_score < 0.5 {
            recommendations
                .push("Execution efficiency is low - review parallel opportunities".to_string());
        }

        recommendations
    }
}

/// Continuous learning system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningArtifact {
    pub task_type: String,
    pub lesson: String,
    pub confidence: f32,
    pub application_count: u32,
}

/// Continuous learner
pub struct ContinuousLearner {
    lessons: Vec<LearningArtifact>,
    improvement_history: Vec<f32>,
}

impl ContinuousLearner {
    pub fn new() -> Self {
        Self {
            lessons: Vec::new(),
            improvement_history: Vec::new(),
        }
    }

    /// Record a lesson from execution
    pub fn record_lesson(&mut self, task_type: &str, lesson: &str, confidence: f32) {
        self.lessons.push(LearningArtifact {
            task_type: task_type.to_string(),
            lesson: lesson.to_string(),
            confidence,
            application_count: 0,
        });
    }

    /// Get applicable lessons for task type
    pub fn get_applicable_lessons(&self, task_type: &str) -> Vec<String> {
        self.lessons
            .iter()
            .filter(|l| l.task_type == task_type && l.confidence > 0.6)
            .map(|l| l.lesson.clone())
            .collect()
    }

    /// Track improvement metric
    pub fn record_improvement(&mut self, metric: f32) {
        self.improvement_history.push(metric);
    }

    /// Get learning curve
    pub fn get_learning_curve(&self) -> Vec<f32> {
        self.improvement_history.clone()
    }

    /// Estimate system maturity
    pub fn estimate_system_maturity(&self) -> f32 {
        if self.improvement_history.is_empty() {
            return 0.0;
        }

        let total_lessons = self.lessons.len() as f32;
        let avg_improvement =
            self.improvement_history.iter().sum::<f32>() / self.improvement_history.len() as f32;

        ((total_lessons / 100.0) * 0.5 + avg_improvement.clamp(0.0, 1.0) * 0.5).clamp(0.0, 1.0)
    }
}

impl Default for ContinuousLearner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parameter_selection() {
        let tuner = DynamicParameterTuner::new();
        let params = tuner.select_parameters("simple", 1);
        assert_eq!(params.max_tool_calls, 5);

        let params = tuner.select_parameters("complex", 5);
        assert!(params.approval_required);
    }

    #[test]
    fn test_resource_allocation() {
        let budget = ResourceAllocator::allocate_resources("feature", 3, 4);
        assert!(budget.token_budget > 0);
        assert!(budget.max_parallel_tasks <= 8);
    }

    #[test]
    fn test_continuous_learning() {
        let mut learner = ContinuousLearner::new();
        learner.record_lesson("bug_fix", "Use binary search for locating bugs", 0.85);
        learner.record_lesson("bug_fix", "Always add regression tests", 0.9);

        let lessons = learner.get_applicable_lessons("bug_fix");
        assert_eq!(lessons.len(), 2);
    }
}
