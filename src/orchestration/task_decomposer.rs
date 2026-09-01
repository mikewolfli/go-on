//! Task Decomposer: Automatic task breakdown into subtasks (Phase 10+)
//!
//! Analyzes complex tasks and automatically decomposes them into manageable
//! subtasks with identified dependencies, parallel execution opportunities,
//! and optimal execution order.

use crate::agent::Agent;
use crate::task_router::{TaskCharacteristics, TaskType};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

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

/// Build a dependency set from string literals.
///
/// The rule-based decomposers previously repeated
/// `["x".to_string()].iter().cloned().collect()` per subtask (~17 copies);
/// this is the single shared construction.
fn dep_set(items: &[&str]) -> HashSet<String> {
    items.iter().map(|s| s.to_string()).collect()
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
    /// Whether LLM was used for decomposition.
    /// `true` = LLM produced the result; `false` = fell back to rule-based.
    #[serde(default)]
    pub llm_used: bool,
}

/// Task decomposer for breaking down complex tasks
pub struct TaskDecomposer;

impl TaskDecomposer {
    /// Decompose a task using an LLM agent for AI-driven decomposition.
    ///
    /// When an LLM agent is provided, it generates a decomposition dynamically
    /// based on the task characteristics. Otherwise falls back to the
    /// rule-based `decompose()` method.
    ///
    /// # Arguments
    /// * `characteristics` - The task characteristics to decompose
    /// * `llm_agent` - Optional LLM agent for AI-driven decomposition
    ///
    /// # Returns
    /// TaskDecomposition with identified subtasks and dependencies
    pub async fn decompose_with_llm(
        characteristics: &TaskCharacteristics,
        llm_agent: Option<Arc<dyn Agent>>,
    ) -> TaskDecomposition {
        if let Some(agent) = llm_agent {
            // Attempt LLM-based decomposition
            let now = std::time::SystemTime::now();
            let ts = now
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();

            let prompt = format!(
                r##"You are a task decomposition specialist. Break the following task into subtasks.

Task: {}
Type: {:?}
Complexity: {}
Capabilities: {:?}

Respond with a JSON object containing:
- "subtasks": array of {{"id": "step_N", "description": "...", "complexity": 1-5, "dependencies": ["step_M", ...], "estimated_duration_seconds": N, "priority": 1-5}}
- "execution_phases": array of arrays of subtask IDs (each phase runs in parallel)

Return ONLY valid JSON, no markdown formatting.
"##,
                characteristics.description,
                characteristics.task_type,
                characteristics.complexity,
                characteristics.required_capabilities
            );

            let envelope = crate::agent::AgentTaskEnvelope {
                task_id: format!("decomp_{}", ts),
                phase: "decomposition".to_string(),
                role: "decomposer".to_string(),
                objective: prompt,
                evidence: None,
                constraints: Some("Return only valid JSON.".to_string()),
                input: serde_json::json!({
                    "description": characteristics.description,
                    "task_type": format!("{:?}", characteristics.task_type),
                    "complexity": characteristics.complexity,
                }),
            };

            match agent.run_task(envelope).await {
                Ok(result) => {
                    if let Some(ref output) = result.output {
                        // Try to parse the LLM output as a TaskDecomposition
                        if let Ok(decomp) =
                            serde_json::from_value::<TaskDecomposition>(output.clone())
                        {
                            // Ensure the execution_phases are populated even if LLM
                            // returned subtasks without explicit phases
                            if decomp.execution_phases.is_empty() && !decomp.subtasks.is_empty() {
                                let phases = Self::compute_execution_phases(&decomp.subtasks);
                                return TaskDecomposition {
                                    execution_phases: phases,
                                    llm_used: true,
                                    ..decomp
                                };
                            }
                            return TaskDecomposition {
                                llm_used: true,
                                ..decomp
                            };
                        }
                        // Try wrapping: LLM might return the object directly without task_id
                        if let Ok(decomp) =
                            serde_json::from_value::<serde_json::Value>(output.clone())
                        {
                            if let Some(subtasks_val) = decomp.get("subtasks") {
                                if let Ok(subtasks) =
                                    serde_json::from_value::<Vec<Subtask>>(subtasks_val.clone())
                                {
                                    let task_id = format!("task_{}", ts);
                                    let execution_phases =
                                        Self::compute_execution_phases(&subtasks);
                                    let total_duration_estimated: u32 =
                                        subtasks.iter().map(|s| s.estimated_duration_seconds).sum();
                                    return TaskDecomposition {
                                        task_id,
                                        subtasks,
                                        execution_phases,
                                        total_duration_estimated,
                                        llm_used: true,
                                    };
                                }
                            }
                        }
                    }
                    tracing::warn!(
                        "LLM decomposition failed to produce valid output, falling back to rule-based"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "LLM agent returned error: {}, falling back to rule-based decomposition",
                        e
                    );
                }
            }
        }
        // Fallback: rule-based decomposition
        Self::decompose(characteristics)
    }

    /// Decompose a task into subtasks using rule-based keyword matching
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
            llm_used: false,
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
                dependencies: dep_set(&["analyze_bug"]),
                estimated_duration_seconds: 600,
                priority: 5,
            },
            Subtask {
                id: "implement_fix".to_string(),
                description: "Implement the fix".to_string(),
                complexity: characteristics.complexity,
                dependencies: dep_set(&["locate_bug"]),
                estimated_duration_seconds: 900,
                priority: 5,
            },
            Subtask {
                id: "write_test".to_string(),
                description: "Write test to prevent regression".to_string(),
                complexity: (characteristics.complexity as i32 - 1).max(1) as u8,
                dependencies: dep_set(&["implement_fix"]),
                estimated_duration_seconds: 600,
                priority: 4,
            },
            Subtask {
                id: "verify_fix".to_string(),
                description: "Verify the fix works".to_string(),
                complexity: 2,
                dependencies: dep_set(&["implement_fix", "write_test"]),
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
                dependencies: dep_set(&["design_api"]),
                estimated_duration_seconds: 1800,
                priority: 5,
            },
            Subtask {
                id: "implement_edge_cases".to_string(),
                description: "Implement error handling and edge cases".to_string(),
                complexity: characteristics.complexity.saturating_sub(1),
                dependencies: dep_set(&["implement_core"]),
                estimated_duration_seconds: 900,
                priority: 4,
            },
            Subtask {
                id: "write_documentation".to_string(),
                description: "Write user and developer documentation".to_string(),
                complexity: 2,
                dependencies: dep_set(&["implement_core"]),
                estimated_duration_seconds: 600,
                priority: 3,
            },
            Subtask {
                id: "add_tests".to_string(),
                description: "Add comprehensive tests".to_string(),
                complexity: characteristics.complexity.saturating_sub(1),
                dependencies: dep_set(&["implement_edge_cases"]),
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
                dependencies: dep_set(&["analyze_current"]),
                estimated_duration_seconds: 600,
                priority: 5,
            },
            Subtask {
                id: "refactor_code".to_string(),
                description: "Execute refactoring".to_string(),
                complexity: characteristics.complexity,
                dependencies: dep_set(&["plan_refactor"]),
                estimated_duration_seconds: 1800,
                priority: 5,
            },
            Subtask {
                id: "update_tests".to_string(),
                description: "Update and run all tests".to_string(),
                complexity: characteristics.complexity.saturating_sub(1),
                dependencies: dep_set(&["refactor_code"]),
                estimated_duration_seconds: 900,
                priority: 4,
            },
            Subtask {
                id: "performance_verify".to_string(),
                description: "Verify no performance regressions".to_string(),
                complexity: 3,
                dependencies: dep_set(&["update_tests"]),
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
                dependencies: dep_set(&["research"]),
                estimated_duration_seconds: 1800,
                priority: 5,
            },
            Subtask {
                id: "prototype".to_string(),
                description: "Build prototype/POC".to_string(),
                complexity: characteristics.complexity.max(2),
                dependencies: dep_set(&["design"]),
                estimated_duration_seconds: 1200,
                priority: 4,
            },
            Subtask {
                id: "document".to_string(),
                description: "Document architecture".to_string(),
                complexity: 2,
                dependencies: dep_set(&["design"]),
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
                dependencies: dep_set(&["identify_cases"]),
                estimated_duration_seconds: 1200,
                priority: 5,
            },
            Subtask {
                id: "write_integration_tests".to_string(),
                description: "Write integration tests".to_string(),
                complexity: 2,
                dependencies: dep_set(&["identify_cases"]),
                estimated_duration_seconds: 1200,
                priority: 4,
            },
            Subtask {
                id: "run_all_tests".to_string(),
                description: "Run and verify all tests".to_string(),
                complexity: 1,
                dependencies: dep_set(&["write_unit_tests", "write_integration_tests"]),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_characteristics(task_type: TaskType, complexity: u8) -> TaskCharacteristics {
        TaskCharacteristics {
            description: "test task".to_string(),
            task_type,
            complexity,
            required_capabilities: vec!["coding".to_string()],
            involves_multiple_modules: false,
            is_time_critical: false,
            needs_verification: true,
            has_safety_concerns: false,
        }
    }

    #[test]
    fn test_decompose_bug_fix_produces_subtasks() {
        let chars = make_characteristics(TaskType::BugFix, 3);
        let result = TaskDecomposer::decompose(&chars);
        assert!(!result.subtasks.is_empty());
        // Bug fix should produce 5 subtasks.
        assert_eq!(result.subtasks.len(), 5);
        // Execution phases should be non-empty.
        assert!(!result.execution_phases.is_empty());
    }

    #[test]
    fn test_decompose_feature_produces_subtasks() {
        let chars = make_characteristics(TaskType::FeatureImplementation, 4);
        let result = TaskDecomposer::decompose(&chars);
        assert_eq!(result.subtasks.len(), 5);
        // design_api should be in the first phase (no dependencies).
        let first_phase = &result.execution_phases[0];
        assert!(first_phase.contains(&"design_api".to_string()));
    }

    #[test]
    fn test_decompose_unknown_uses_generic() {
        let chars = make_characteristics(TaskType::Unknown, 2);
        let result = TaskDecomposer::decompose(&chars);
        // Complexity 2 => clamp(2,2,5) => 2 subtasks.
        assert_eq!(result.subtasks.len(), 2);
        // Generic subtasks should follow "step_0", "step_1" naming.
        assert!(result.subtasks.iter().any(|s| s.id == "step_0"));
        assert!(result.subtasks.iter().any(|s| s.id == "step_1"));
    }
}
