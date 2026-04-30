//! Task Router: Intelligent automatic agent role assignment
//!
//! This module analyzes incoming tasks and automatically selects the optimal
//! combination of agent roles (Planner/Researcher/Coder/Tester/Reviewer) needed
//! to complete the task successfully.
//!
//! Phase 10 enhancement: Takes the existing role definitions and makes them
//! automatically-selected based on task characteristics.

use crate::orchestration::workflow_registry::WorkflowRegistry;
use crate::pua::{build_enforcement_plan, PuaEnforcementPlan};
use crate::roles::{role_registry, AgentRole, RoleSpecification, RoleSpecifications};
use serde::{Deserialize, Serialize};

/// Task characteristics extracted from request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskCharacteristics {
    /// Task description
    pub description: String,
    /// Extracted task type (e.g., "bug_fix", "feature", "refactor", "test")
    pub task_type: TaskType,
    /// Complexity level (1-5): 1=trivial, 5=very complex
    pub complexity: u8,
    /// Required capabilities for this task
    pub required_capabilities: Vec<String>,
    /// Whether task involves multiple files/modules
    pub involves_multiple_modules: bool,
    /// Whether task is time-critical
    pub is_time_critical: bool,
    /// Whether task needs verification/testing
    pub needs_verification: bool,
    /// Whether task involves security/safety concerns
    pub has_safety_concerns: bool,
}

/// Task type classification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum TaskType {
    /// Bug fix/diagnosis
    BugFix,
    /// New feature implementation
    FeatureImplementation,
    /// Code refactoring
    Refactoring,
    /// Test implementation/improvement
    TestImplementation,
    /// Documentation
    Documentation,
    /// Architecture/design task
    ArchitectureDesign,
    /// Performance optimization
    PerformanceOptimization,
    /// Code review
    CodeReview,
    /// Unknown/generic
    Unknown,
}

/// Role requirement specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleRequirement {
    /// The role needed
    pub role: AgentRole,
    /// Priority: "critical", "important", "optional"
    pub priority: String,
    /// Estimated position in execution order (0=first)
    pub sequence_position: usize,
    /// Why this role is needed (explanation)
    pub justification: String,
}

/// Result of task routing analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingDecision {
    /// Selected roles in execution order
    pub roles: Vec<AgentRole>,
    /// Detailed requirements for each role
    pub requirements: Vec<RoleRequirement>,
    /// Estimated probability of success with selected roles
    pub predicted_success_rate: f32,
    /// Estimated total execution time in seconds
    pub estimated_duration_seconds: u32,
    /// Whether parallel execution is recommended for any roles
    pub can_parallelize: Vec<(AgentRole, AgentRole)>, // pairs that can run in parallel
    /// Key risk factors identified
    pub risk_factors: Vec<String>,
    /// Recommended safeguards
    pub recommended_safeguards: Vec<String>,
    /// PUA enforcement plan that must be honored downstream
    pub pua_enforcement: PuaEnforcementPlan,
}

/// Task router that performs automatic role routing
pub struct TaskRouter;

impl TaskRouter {
    /// Analyze task and generate routing decision
    ///
    /// # Arguments
    /// * `task_description` - User's task request
    ///
    /// # Returns
    /// * `TaskCharacteristics` - Analyzed task characteristics
    pub fn analyze_task(task_description: &str) -> TaskCharacteristics {
        let lower = task_description.to_lowercase();

        // Classify task type
        let task_type = if lower.contains("fix") || lower.contains("bug") {
            TaskType::BugFix
        } else if lower.contains("feature") || lower.contains("implement") {
            TaskType::FeatureImplementation
        } else if lower.contains("refactor") || lower.contains("optimize") {
            TaskType::Refactoring
        } else if lower.contains("test") {
            TaskType::TestImplementation
        } else if lower.contains("doc") || lower.contains("comment") {
            TaskType::Documentation
        } else if lower.contains("architecture") || lower.contains("design") {
            TaskType::ArchitectureDesign
        } else if lower.contains("performance") || lower.contains("speed") {
            TaskType::PerformanceOptimization
        } else if lower.contains("review") {
            TaskType::CodeReview
        } else {
            TaskType::Unknown
        };

        // Analyze complexity
        let complexity = Self::estimate_complexity(task_description);

        // Extract required capabilities
        let required_capabilities = Self::extract_capabilities(&lower);

        // Analyze characteristics
        let involves_multiple_modules = lower.contains("multiple")
            || lower.contains("modules")
            || lower.contains("package")
            || lower.contains("cross-");

        let is_time_critical = lower.contains("urgent")
            || lower.contains("asap")
            || lower.contains("quick")
            || lower.contains("fast");

        let needs_verification = matches!(
            task_type,
            TaskType::BugFix | TaskType::FeatureImplementation | TaskType::PerformanceOptimization
        );

        let has_safety_concerns = lower.contains("security")
            || lower.contains("safe")
            || lower.contains("memory")
            || lower.contains("delete")
            || lower.contains("drop");

        TaskCharacteristics {
            description: task_description.to_string(),
            task_type,
            complexity,
            required_capabilities,
            involves_multiple_modules,
            is_time_critical,
            needs_verification,
            has_safety_concerns,
        }
    }

    /// Route task to optimal role combination
    ///
    /// # Arguments
    /// * `characteristics` - Analyzed task characteristics
    ///
    /// # Returns
    /// * `RoutingDecision` - Recommended roles and execution strategy
    pub fn route_task(characteristics: &TaskCharacteristics) -> RoutingDecision {
        let mut roles = Vec::new();
        let mut requirements = Vec::new();

        // Always start with Planner for complex tasks
        if characteristics.complexity >= 3 || characteristics.involves_multiple_modules {
            roles.push(AgentRole::Planner);
            requirements.push(RoleRequirement {
                role: AgentRole::Planner,
                priority: "critical".to_string(),
                sequence_position: requirements.len(),
                justification: "Complex task requires planning and decomposition".to_string(),
            });
        }

        // Add Researcher for architectural/refactoring tasks
        if matches!(
            characteristics.task_type,
            TaskType::ArchitectureDesign | TaskType::Refactoring
        ) {
            roles.push(AgentRole::Researcher);
            requirements.push(RoleRequirement {
                role: AgentRole::Researcher,
                priority: "important".to_string(),
                sequence_position: requirements.len(),
                justification: "Need to analyze impact and existing patterns".to_string(),
            });
        }

        // Add Coder for implementation tasks
        if matches!(
            characteristics.task_type,
            TaskType::BugFix
                | TaskType::FeatureImplementation
                | TaskType::Refactoring
                | TaskType::PerformanceOptimization
        ) {
            roles.push(AgentRole::Coder);
            requirements.push(RoleRequirement {
                role: AgentRole::Coder,
                priority: "critical".to_string(),
                sequence_position: requirements.len(),
                justification: "Implementation of code changes".to_string(),
            });
        }

        // Add Tester if verification is needed
        if characteristics.needs_verification {
            roles.push(AgentRole::Tester);
            requirements.push(RoleRequirement {
                role: AgentRole::Tester,
                priority: "important".to_string(),
                sequence_position: requirements.len(),
                justification: "Verify changes work correctly".to_string(),
            });
        }

        // Add Reviewer for safety-critical or high-complexity tasks
        if characteristics.has_safety_concerns || characteristics.complexity >= 4 {
            roles.push(AgentRole::Reviewer);
            requirements.push(RoleRequirement {
                role: AgentRole::Reviewer,
                priority: "critical".to_string(),
                sequence_position: requirements.len(),
                justification: "Review for quality, security, and correctness".to_string(),
            });
        }

        // If no roles selected (simple task), just use Coder
        if roles.is_empty() {
            roles.push(AgentRole::Coder);
            requirements.push(RoleRequirement {
                role: AgentRole::Coder,
                priority: "critical".to_string(),
                sequence_position: 0,
                justification: "Simple implementation task".to_string(),
            });
        }

        let pua_enforcement = build_enforcement_plan(
            &characteristics.description,
            characteristics.complexity,
            characteristics.needs_verification,
            characteristics.has_safety_concerns,
            characteristics.involves_multiple_modules,
        );

        for role in &pua_enforcement.mandatory_roles {
            if !roles.contains(role) {
                roles.push(role.clone());
                requirements.push(RoleRequirement {
                    role: role.clone(),
                    priority: "critical".to_string(),
                    sequence_position: requirements.len(),
                    justification: format!(
                        "PUA enforcement requires {:?} coverage for proof and accountability",
                        role
                    ),
                });
            }
        }

        // Calculate success rate based on role combination and task type
        let predicted_success_rate = Self::predict_success_rate(characteristics, &roles);

        // Estimate execution time
        let estimated_duration_seconds = Self::estimate_execution_duration(characteristics, &roles);

        // Identify parallelizable pairs
        let can_parallelize = Self::identify_parallel_opportunities(&roles);

        // Identify risks
        let risk_factors = Self::identify_risk_factors(characteristics);

        // Recommend safeguards
        let mut recommended_safeguards = Self::recommend_safeguards(characteristics, &risk_factors);
        recommended_safeguards.extend(pua_enforcement.mandatory_safeguards.clone());
        Self::dedupe_strings(&mut recommended_safeguards);

        RoutingDecision {
            roles,
            requirements,
            predicted_success_rate,
            estimated_duration_seconds,
            can_parallelize,
            risk_factors,
            recommended_safeguards,
            pua_enforcement,
        }
    }

    /// Get role specifications for the selected roles
    pub fn get_role_specs(roles: &[AgentRole]) -> Vec<RoleSpecification> {
        roles
            .iter()
            .map(|role| match role {
                AgentRole::Planner => RoleSpecifications::planner(),
                AgentRole::Researcher => RoleSpecifications::researcher(),
                AgentRole::Coder => RoleSpecifications::coder(),
                AgentRole::Tester => RoleSpecifications::tester(),
                AgentRole::Reviewer => RoleSpecifications::reviewer(),
                AgentRole::Custom(name) => {
                    // Try RoleRegistry first; fall back to default coder spec
                    let registry = role_registry();
                    if let Ok(guard) = registry.read() {
                        if let Some(def) = guard.get(name) {
                            RoleSpecification {
                                role: AgentRole::Custom(name.clone()),
                                tier: "primary".to_string(),
                                allowed_tools: def.allowed_tools.clone(),
                                max_tool_calls: def.max_tool_calls,
                                token_budget: def.token_budget,
                                timeout_seconds: def.timeout_seconds,
                            }
                        } else {
                            RoleSpecifications::coder()
                        }
                    } else {
                        RoleSpecifications::coder()
                    }
                }
            })
            .collect()
    }

    /// Route a task using a WorkflowRegistry preset lookup.
    /// If a matching preset is found (by name or by task type match),
    /// its phases override the default routing decision's role selection.
    pub fn route_task_with_workflow(
        characteristics: &TaskCharacteristics,
        workflow_registry: &WorkflowRegistry,
    ) -> RoutingDecision {
        let mut decision = Self::route_task(characteristics);

        // Try to match task characteristics to a workflow preset
        let task_type_str = format!("{:?}", characteristics.task_type).to_lowercase();
        let preset = workflow_registry.find(&task_type_str);

        if let Some(p) = preset {
            decision.recommended_safeguards.push(format!(
                "workflow_preset:{} ({} phases)",
                p.name,
                p.phases.len()
            ));
        } else {
            // Fallback: check all presets for a general-purpose one
            for preset in workflow_registry.list() {
                if preset.name == "general" || preset.name == "autopilot" {
                    decision.recommended_safeguards.push(format!(
                        "workflow_fallback:{} ({} phases)",
                        preset.name,
                        preset.phases.len()
                    ));
                    break;
                }
            }
        }

        decision
    }

    // ==================== Private Helper Methods ====================

    fn estimate_complexity(description: &str) -> u8 {
        let lower = description.to_lowercase();
        let mut score = 2u8; // baseline

        // Increase complexity for complex keywords
        if lower.contains("complex")
            || lower.contains("rewrite")
            || lower.contains("redesign")
            || lower.contains("algorithm")
        {
            score += 2;
        }
        if lower.contains("performance")
            || lower.contains("optimization")
            || lower.contains("concurrent")
        {
            score += 1;
        }

        // Decrease for simple keywords
        if lower.contains("simple") || lower.contains("trivial") {
            score = score.saturating_sub(1);
        }

        // Cap at 5
        score.min(5)
    }

    fn extract_capabilities(task_lower: &str) -> Vec<String> {
        let mut capabilities = Vec::new();

        if task_lower.contains("api") || task_lower.contains("rest") {
            capabilities.push("api_design".to_string());
        }
        if task_lower.contains("database") || task_lower.contains("sql") {
            capabilities.push("database".to_string());
        }
        if task_lower.contains("async") || task_lower.contains("concurrency") {
            capabilities.push("concurrency".to_string());
        }
        if task_lower.contains("security")
            || task_lower.contains("auth")
            || task_lower.contains("encrypt")
            || task_lower.contains("safe")
        {
            capabilities.push("security".to_string());
        }
        if task_lower.contains("test") {
            capabilities.push("testing".to_string());
        }
        if task_lower.contains("memory")
            || task_lower.contains("leak")
            || task_lower.contains("gc")
            || task_lower.contains("performance")
        {
            capabilities.push("memory".to_string());
        }
        if task_lower.contains("ui") || task_lower.contains("ux") {
            capabilities.push("user_interface".to_string());
        }

        capabilities
    }

    fn predict_success_rate(characteristics: &TaskCharacteristics, roles: &[AgentRole]) -> f32 {
        let mut base_rate = 0.75; // 75% base

        // Complexity impact
        base_rate -= (characteristics.complexity as f32 - 2.5) * 0.08; // -8% per complexity level

        // Role diversity helps
        let role_count = roles.len();
        base_rate += (role_count as f32 - 1.0) * 0.08; // +8% per additional role (up to 5)

        // Historical proxy: use current role-task fit and risk profile as a stable estimator.
        let role_fit = Self::estimate_role_task_fit(characteristics, roles);
        base_rate += role_fit;

        if characteristics.has_safety_concerns && !roles.contains(&AgentRole::Reviewer) {
            base_rate -= 0.12;
        }
        if characteristics.needs_verification && !roles.contains(&AgentRole::Tester) {
            base_rate -= 0.08;
        }

        base_rate.clamp(0.2, 0.99)
    }

    fn estimate_role_task_fit(characteristics: &TaskCharacteristics, roles: &[AgentRole]) -> f32 {
        let mut score: f32 = 0.0;

        let has_coder = roles.contains(&AgentRole::Coder);
        let has_tester = roles.contains(&AgentRole::Tester);
        let has_reviewer = roles.contains(&AgentRole::Reviewer);
        let has_researcher = roles.contains(&AgentRole::Researcher);
        let has_planner = roles.contains(&AgentRole::Planner);

        match characteristics.task_type {
            TaskType::BugFix => {
                if has_coder {
                    score += 0.06;
                }
                if has_tester {
                    score += 0.05;
                }
            }
            TaskType::FeatureImplementation => {
                if has_coder {
                    score += 0.06;
                }
                if has_planner {
                    score += 0.04;
                }
            }
            TaskType::Refactoring => {
                if has_researcher {
                    score += 0.05;
                }
                if has_reviewer {
                    score += 0.04;
                }
            }
            TaskType::TestImplementation => {
                if has_tester {
                    score += 0.08;
                }
                if has_coder {
                    score += 0.03;
                }
            }
            TaskType::ArchitectureDesign => {
                if has_planner {
                    score += 0.08;
                }
                if has_researcher {
                    score += 0.04;
                }
            }
            TaskType::PerformanceOptimization => {
                if has_researcher {
                    score += 0.05;
                }
                if has_tester {
                    score += 0.04;
                }
            }
            TaskType::CodeReview => {
                if has_reviewer {
                    score += 0.08;
                }
            }
            TaskType::Documentation | TaskType::Unknown => {
                if has_planner {
                    score += 0.03;
                }
            }
        }

        if characteristics.involves_multiple_modules && has_planner {
            score += 0.03;
        }

        score.clamp(-0.15, 0.2)
    }

    fn estimate_execution_duration(
        characteristics: &TaskCharacteristics,
        roles: &[AgentRole],
    ) -> u32 {
        let base_minutes = match characteristics.complexity {
            1 => 2,
            2 => 5,
            3 => 15,
            4 => 30,
            5 => 60,
            _ => 10,
        };

        // Adjust for task type
        let mut multiplier = match characteristics.task_type {
            TaskType::BugFix => 1.2,
            TaskType::FeatureImplementation => 1.5,
            TaskType::Refactoring => 1.3,
            TaskType::TestImplementation => 1.0,
            TaskType::ArchitectureDesign => 2.0,
            TaskType::PerformanceOptimization => 1.8,
            TaskType::CodeReview => 0.8,
            _ => 1.0,
        };

        // Additional time for coordination with multiple roles
        if roles.len() > 1 {
            multiplier += (roles.len() as f32 - 1.0) * 0.15;
        }

        (base_minutes as f32 * multiplier * 60.0) as u32
    }

    fn identify_parallel_opportunities(roles: &[AgentRole]) -> Vec<(AgentRole, AgentRole)> {
        // Researcher can run in parallel with Planner (parallel analysis)
        // Both Coder and Tester can run after Planner, potentially in parallel if subtasks split
        let mut pairs = Vec::new();

        // Planner and Researcher are independent (parallel is beneficial)
        if roles.contains(&AgentRole::Planner) && roles.contains(&AgentRole::Researcher) {
            pairs.push((AgentRole::Planner, AgentRole::Researcher));
        }

        pairs
    }

    fn identify_risk_factors(characteristics: &TaskCharacteristics) -> Vec<String> {
        let mut risks = Vec::new();

        if characteristics.complexity >= 5 {
            risks.push("Very high complexity - multiple review cycles likely needed".to_string());
        }

        if characteristics.has_safety_concerns {
            risks.push("Safety/security concerns - requires careful verification".to_string());
        }

        if characteristics.involves_multiple_modules {
            risks.push("Multi-module changes - high integration risk".to_string());
        }

        if characteristics.is_time_critical {
            risks.push("Time-critical - time pressure may increase error rate".to_string());
        }

        risks
    }

    fn recommend_safeguards(
        characteristics: &TaskCharacteristics,
        risk_factors: &[String],
    ) -> Vec<String> {
        let mut safeguards = Vec::new();

        if !risk_factors.is_empty() {
            safeguards.push("Use SafeGuard mode - require approval at high-risk nodes".to_string());
        }

        if characteristics.complexity >= 4 {
            safeguards.push("Enable double-review process".to_string());
        }

        if characteristics.involves_multiple_modules {
            safeguards.push("Run full test suite before merging".to_string());
        }

        if characteristics.has_safety_concerns {
            safeguards.push("Enable security scanning in the verification step".to_string());
        }

        safeguards
    }

    fn dedupe_strings(values: &mut Vec<String>) {
        let mut deduped = Vec::new();
        for value in values.drain(..) {
            if !deduped.contains(&value) {
                deduped.push(value);
            }
        }
        *values = deduped;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analyze_task_bug_fix() {
        let task = "Fix the memory leak in the parser module";
        let characteristics = TaskRouter::analyze_task(task);

        assert_eq!(characteristics.task_type, TaskType::BugFix);
        assert!(characteristics.has_safety_concerns);
        assert!(characteristics
            .required_capabilities
            .contains(&"memory".to_string()));
    }

    #[test]
    fn test_analyze_task_feature() {
        let task = "Implement new API endpoint for user authentication";
        let characteristics = TaskRouter::analyze_task(task);

        assert_eq!(characteristics.task_type, TaskType::FeatureImplementation);
        assert!(characteristics
            .required_capabilities
            .contains(&"api_design".to_string()));
        assert!(characteristics
            .required_capabilities
            .contains(&"security".to_string()));
    }

    #[test]
    fn test_route_simple_task() {
        let characteristics = TaskCharacteristics {
            description: "Add print statement".to_string(),
            task_type: TaskType::BugFix,
            complexity: 1,
            required_capabilities: vec![],
            involves_multiple_modules: false,
            is_time_critical: false,
            needs_verification: true,
            has_safety_concerns: false,
        };

        let decision = TaskRouter::route_task(&characteristics);
        assert!(!decision.roles.is_empty());
        assert!(decision.roles.contains(&AgentRole::Coder));
        assert!(!decision.pua_enforcement.quality_compass.is_empty());
    }

    #[test]
    fn test_route_complex_task() {
        let characteristics = TaskCharacteristics {
            description: "Refactor concurrency model across 5 modules".to_string(),
            task_type: TaskType::Refactoring,
            complexity: 5,
            required_capabilities: vec!["concurrency".to_string()],
            involves_multiple_modules: true,
            is_time_critical: false,
            needs_verification: true,
            has_safety_concerns: true,
        };

        let decision = TaskRouter::route_task(&characteristics);
        assert!(decision.roles.len() >= 3);
        assert!(decision.roles.contains(&AgentRole::Planner));
        assert!(decision.roles.contains(&AgentRole::Reviewer));
        assert!((0.2..=0.99).contains(&decision.predicted_success_rate));
        assert!(!decision.recommended_safeguards.is_empty());
        assert_eq!(decision.pua_enforcement.escalation_level, "L3");
    }
}
