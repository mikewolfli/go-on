//! Task Decomposer: Automatic task breakdown into subtasks (Phase 10+)
//!
//! Analyzes complex tasks and automatically decomposes them into manageable
//! subtasks with identified dependencies, parallel execution opportunities,
//! and optimal execution order.

use crate::task_router::{TaskCharacteristics, TaskType};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// A single subtask
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subtask {
    /// Unique subtask ID
    pub id: String,
    /// Subtask description
    pub description: String,
    /// Complexity level (1-5)
    pub complexity: u8,
    /// List of subtask IDs this depends on
    pub dependencies: HashSet<String>,
    /// Estimated duration in seconds
    pub estimated_duration_seconds: u32,
    /// Priority (1-5, 5=highest)
    pub priority: u8,
}

/// Task decomposition result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDecomposition {
    /// Original task ID
    pub task_id: String,
    /// List of subtasks
    pub subtasks: Vec<Subtask>,
    /// Execution phases (groups of subtasks that can be parallelized)
    pub execution_phases: Vec<Vec<String>>, // phase -> [subtask_ids]
    /// Total estimated duration in seconds
    pub total_duration_estimated: u32,
}

/// Task decomposer for breaking down complex tasks
pub struct TaskDecomposer;

impl TaskDecomposer {
    /// Decompose a task into subtasks
    ///
    /// # Arguments
    /// * `characteristics` - The task characteristics to decompose
    ///
    /// # Returns
    /// TaskDecomposition with identified subtasks and dependencies
    pub fn decompose(characteristics: &TaskCharacteristics) -> TaskDecomposition {
        let task_id = format!(
            "task_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );

        let subtasks = match characteristics.task_type {
            TaskType::BugFix => Self::decompose_bug_fix(characteristics),
            TaskType::FeatureImplementation => Self::decompose_feature(characteristics),
            TaskType::Refactoring => Self::decompose_refactoring(characteristics),
            TaskType::ArchitectureDesign => Self::decompose_architecture(characteristics),
            TaskType::TestImplementation => Self::decompose_testing(characteristics),
            _ => Self::decompose_generic(characteristics),
        };

        // Compute execution phases (topological sort)
        let execution_phases = Self::compute_execution_phases(&subtasks);

        // Calculate total duration
        let total_duration_estimated = execution_phases
            .iter()
            .map(|phase| {
                phase
                    .iter()
                    .filter_map(|id| subtasks.iter().find(|s| &s.id == id))
                    .map(|s| s.estimated_duration_seconds)
                    .max()
                    .unwrap_or(0)
            })
            .sum();

        TaskDecomposition {
            task_id,
            subtasks,
            execution_phases,
            total_duration_estimated,
        }
    }

    // ==================== Task-Specific Decompositions ====================

    fn decompose_bug_fix(characteristics: &TaskCharacteristics) -> Vec<Subtask> {
        vec![
            Subtask {
                id: "analyze_bug".to_string(),
                description: "Analyze bug symptoms and root cause".to_string(),
                complexity: characteristics.complexity.max(2),
                dependencies: HashSet::new(),
                estimated_duration_seconds: 300,
                priority: 5,
            },
            Subtask {
                id: "locate_bug".to_string(),
                description: "Locate the bug in the codebase".to_string(),
                complexity: characteristics.complexity,
                dependencies: ["analyze_bug".to_string()].iter().cloned().collect(),
                estimated_duration_seconds: 600,
                priority: 5,
            },
            Subtask {
                id: "implement_fix".to_string(),
                description: "Implement the fix".to_string(),
                complexity: characteristics.complexity,
                dependencies: ["locate_bug".to_string()].iter().cloned().collect(),
                estimated_duration_seconds: 900,
                priority: 5,
            },
            Subtask {
                id: "write_test".to_string(),
                description: "Write test to prevent regression".to_string(),
                complexity: (characteristics.complexity as i32 - 1).max(1) as u8,
                dependencies: ["implement_fix".to_string()].iter().cloned().collect(),
                estimated_duration_seconds: 600,
                priority: 4,
            },
            Subtask {
                id: "verify_fix".to_string(),
                description: "Verify the fix works".to_string(),
                complexity: 2,
                dependencies: ["implement_fix".to_string(), "write_test".to_string()]
                    .iter()
                    .cloned()
                    .collect(),
                estimated_duration_seconds: 300,
                priority: 5,
            },
        ]
    }

    fn decompose_feature(characteristics: &TaskCharacteristics) -> Vec<Subtask> {
        vec![
            Subtask {
                id: "design_api".to_string(),
                description: "Design feature API and interfaces".to_string(),
                complexity: characteristics.complexity.max(2),
                dependencies: HashSet::new(),
                estimated_duration_seconds: 600,
                priority: 5,
            },
            Subtask {
                id: "implement_core".to_string(),
                description: "Implement core feature logic".to_string(),
                complexity: characteristics.complexity,
                dependencies: ["design_api".to_string()].iter().cloned().collect(),
                estimated_duration_seconds: 1800,
                priority: 5,
            },
            Subtask {
                id: "implement_edge_cases".to_string(),
                description: "Implement error handling and edge cases".to_string(),
                complexity: characteristics.complexity.saturating_sub(1),
                dependencies: ["implement_core".to_string()].iter().cloned().collect(),
                estimated_duration_seconds: 900,
                priority: 4,
            },
            Subtask {
                id: "write_documentation".to_string(),
                description: "Write user and developer documentation".to_string(),
                complexity: 2,
                dependencies: ["implement_core".to_string()].iter().cloned().collect(),
                estimated_duration_seconds: 600,
                priority: 3,
            },
            Subtask {
                id: "add_tests".to_string(),
                description: "Add comprehensive tests".to_string(),
                complexity: characteristics.complexity.saturating_sub(1),
                dependencies: ["implement_edge_cases".to_string()]
                    .iter()
                    .cloned()
                    .collect(),
                estimated_duration_seconds: 1200,
                priority: 4,
            },
        ]
    }

    fn decompose_refactoring(characteristics: &TaskCharacteristics) -> Vec<Subtask> {
        vec![
            Subtask {
                id: "analyze_current".to_string(),
                description: "Analyze current code structure and identify improvements".to_string(),
                complexity: characteristics.complexity,
                dependencies: HashSet::new(),
                estimated_duration_seconds: 900,
                priority: 5,
            },
            Subtask {
                id: "plan_refactor".to_string(),
                description: "Plan refactoring strategy and changes".to_string(),
                complexity: characteristics.complexity.max(2),
                dependencies: ["analyze_current".to_string()].iter().cloned().collect(),
                estimated_duration_seconds: 600,
                priority: 5,
            },
            Subtask {
                id: "refactor_code".to_string(),
                description: "Execute refactoring".to_string(),
                complexity: characteristics.complexity,
                dependencies: ["plan_refactor".to_string()].iter().cloned().collect(),
                estimated_duration_seconds: 1800,
                priority: 5,
            },
            Subtask {
                id: "update_tests".to_string(),
                description: "Update and run all tests".to_string(),
                complexity: characteristics.complexity.saturating_sub(1),
                dependencies: ["refactor_code".to_string()].iter().cloned().collect(),
                estimated_duration_seconds: 900,
                priority: 4,
            },
            Subtask {
                id: "performance_verify".to_string(),
                description: "Verify no performance regressions".to_string(),
                complexity: 3,
                dependencies: ["update_tests".to_string()].iter().cloned().collect(),
                estimated_duration_seconds: 600,
                priority: 4,
            },
        ]
    }

    fn decompose_architecture(characteristics: &TaskCharacteristics) -> Vec<Subtask> {
        vec![
            Subtask {
                id: "research".to_string(),
                description: "Research and analyze existing patterns".to_string(),
                complexity: 3,
                dependencies: HashSet::new(),
                estimated_duration_seconds: 1200,
                priority: 5,
            },
            Subtask {
                id: "design".to_string(),
                description: "Design new architecture".to_string(),
                complexity: characteristics.complexity.max(3),
                dependencies: ["research".to_string()].iter().cloned().collect(),
                estimated_duration_seconds: 1800,
                priority: 5,
            },
            Subtask {
                id: "prototype".to_string(),
                description: "Build prototype/POC".to_string(),
                complexity: characteristics.complexity.max(2),
                dependencies: ["design".to_string()].iter().cloned().collect(),
                estimated_duration_seconds: 1200,
                priority: 4,
            },
            Subtask {
                id: "document".to_string(),
                description: "Document architecture".to_string(),
                complexity: 2,
                dependencies: ["design".to_string()].iter().cloned().collect(),
                estimated_duration_seconds: 900,
                priority: 4,
            },
        ]
    }

    fn decompose_testing(_characteristics: &TaskCharacteristics) -> Vec<Subtask> {
        vec![
            Subtask {
                id: "identify_cases".to_string(),
                description: "Identify all test cases needed".to_string(),
                complexity: 2,
                dependencies: HashSet::new(),
                estimated_duration_seconds: 600,
                priority: 5,
            },
            Subtask {
                id: "write_unit_tests".to_string(),
                description: "Write unit tests".to_string(),
                complexity: 2,
                dependencies: ["identify_cases".to_string()].iter().cloned().collect(),
                estimated_duration_seconds: 1200,
                priority: 5,
            },
            Subtask {
                id: "write_integration_tests".to_string(),
                description: "Write integration tests".to_string(),
                complexity: 2,
                dependencies: ["identify_cases".to_string()].iter().cloned().collect(),
                estimated_duration_seconds: 1200,
                priority: 4,
            },
            Subtask {
                id: "run_all_tests".to_string(),
                description: "Run and verify all tests".to_string(),
                complexity: 1,
                dependencies: [
                    "write_unit_tests".to_string(),
                    "write_integration_tests".to_string(),
                ]
                .iter()
                .cloned()
                .collect(),
                estimated_duration_seconds: 300,
                priority: 5,
            },
        ]
    }

    fn decompose_generic(characteristics: &TaskCharacteristics) -> Vec<Subtask> {
        let num_subtasks = (characteristics.complexity as usize).clamp(2, 5);

        (0..num_subtasks)
            .map(|i| {
                let mut deps = HashSet::new();
                if i > 0 {
                    deps.insert(format!("step_{}", i - 1));
                }

                Subtask {
                    id: format!("step_{}", i),
                    description: format!("Execute step {} of {}", i + 1, num_subtasks),
                    complexity: characteristics.complexity,
                    dependencies: deps,
                    estimated_duration_seconds: 600,
                    priority: (5 - (i as u8)).max(1),
                }
            })
            .collect()
    }

    // ==================== Execution Planning ====================

    fn compute_execution_phases(subtasks: &[Subtask]) -> Vec<Vec<String>> {
        let mut phases = Vec::new();
        let mut completed = HashSet::new();

        while completed.len() < subtasks.len() {
            let mut current_phase = Vec::new();

            for subtask in subtasks {
                if completed.contains(&subtask.id) {
                    continue;
                }

                // Check if all dependencies are satisfied
                if subtask
                    .dependencies
                    .iter()
                    .all(|dep| completed.contains(dep))
                {
                    current_phase.push(subtask.id.clone());
                }
            }

            if current_phase.is_empty() {
                break; // Avoid infinite loop
            }

            for id in &current_phase {
                completed.insert(id.clone());
            }

            phases.push(current_phase);
        }

        phases
    }
}
