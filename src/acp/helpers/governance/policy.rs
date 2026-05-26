//! Policy helper functions for ACP server
//!
//! This module provides utility functions for managing review policies,
//! work grade decisions, optimization policies, and agent ranking.

use serde::Serialize;
use serde_json::Value;

use crate::config::PhaseOptions;
use crate::orchestration::roles::role_registry_keywords_for;
use crate::orchestration::task_router::TaskCharacteristics;
use crate::reinforcement::{
    recommend_reattach_modules_from_policy_history, ActionCheckKind, ArtifactLedger,
    ExecutionDecisionCandidate,
};

// Helper functions from original acp/helpers module
// These are defined in acp/helpers/misc.rs via include! macro
// For now, we'll copy their implementations
fn extra_u64(options: Option<&PhaseOptions>, key: &str) -> Option<u64> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|v| v.as_u64())
}

#[allow(dead_code)] // F-GAP-17 — reserved for future f64 extra policy parameters
fn extra_f64(options: Option<&PhaseOptions>, key: &str) -> Option<f64> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|v| v.as_f64())
}

fn extra_string(options: Option<&PhaseOptions>, key: &str) -> Option<String> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
}

fn extra_bool(options: Option<&PhaseOptions>, key: &str) -> Option<bool> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|v| v.as_bool())
}

fn extra_string_list(options: Option<&PhaseOptions>, key: &str) -> Option<Vec<String>> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.as_str())
                .map(|item| item.to_string())
                .collect::<Vec<_>>()
        })
}

/// Review policy configuration
#[derive(Debug, Clone, Serialize)]
pub struct ReviewPolicy {
    /// Minimum review level (standard/enhanced)
    pub min_review_level: String,
    /// Number of required reviews
    pub required_reviews: usize,
    /// Required check kinds
    pub required_checks: Vec<String>,
    /// Timeout policy (reject/approve/escalate)
    pub timeout_policy: String,
    /// Whether to enforce dual review
    pub enforce_dual_review: bool,
    /// Whether to enforce action gates
    pub enforce_action_gates: bool,
}

/// Resolve review policy based on options, characteristics, and context
pub fn resolve_review_policy(
    options: Option<&PhaseOptions>,
    characteristics: Option<&TaskCharacteristics>,
    is_workflow_execute: bool,
    requested_dual_review: bool,
) -> ReviewPolicy {
    let inferred_enhanced = characteristics
        .map(|c| c.complexity >= 4 || c.has_safety_concerns)
        .unwrap_or(false)
        || is_workflow_execute;

    let min_review_level = extra_string(options, "review_min_level").unwrap_or_else(|| {
        if inferred_enhanced {
            "enhanced".to_string()
        } else {
            "standard".to_string()
        }
    });
    let required_reviews = extra_u64(options, "review_required_reviews")
        .map(|v: u64| v.max(1) as usize)
        .unwrap_or_else(|| {
            if min_review_level.eq_ignore_ascii_case("enhanced") {
                2
            } else {
                1
            }
        });
    let required_checks =
        extra_string_list(options, "review_required_checks").unwrap_or_else(|| {
            if is_workflow_execute {
                vec!["qa".to_string(), "retest".to_string(), "final".to_string()]
            } else {
                Vec::new()
            }
        });
    let timeout_policy =
        extra_string(options, "review_timeout_policy").unwrap_or_else(|| "reject".to_string());
    let enforce_dual_review = requested_dual_review
        || required_reviews >= 2
        || min_review_level.eq_ignore_ascii_case("enhanced");
    let enforce_action_gates = !required_checks.is_empty();

    ReviewPolicy {
        min_review_level,
        required_reviews,
        required_checks,
        timeout_policy,
        enforce_dual_review,
        enforce_action_gates,
    }
}

/// Convert required check names to ActionCheckKind enum values
#[allow(dead_code)] // F-GAP-17 — reserved for action gate integration
pub fn action_check_kinds_from_policy(required_checks: &[String]) -> Vec<ActionCheckKind> {
    if required_checks.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    for name in required_checks {
        if let Some(kind) = ActionCheckKind::parse(name) {
            if !out.contains(&kind) {
                out.push(kind);
            }
        }
    }
    out
}

/// Work grade classification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkGrade {
    Ask,
    Edit,
    Agent,
    Safeguard,
    FullAuto,
}

impl WorkGrade {
    /// Parse work grade from string
    pub fn parse(raw: Option<&str>) -> Option<Self> {
        let value = raw?.trim().to_ascii_lowercase();
        match value.as_str() {
            "ask" => Some(Self::Ask),
            "edit" => Some(Self::Edit),
            "agent" => Some(Self::Agent),
            "safeguard" => Some(Self::Safeguard),
            "full_auto" | "full-auto" | "auto" => Some(Self::FullAuto),
            _ => None,
        }
    }

    /// Convert work grade to string representation
    #[allow(dead_code)] // F-GAP-17 — reserved for serialization in policy reports
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Edit => "edit",
            Self::Agent => "agent",
            Self::Safeguard => "safeguard",
            Self::FullAuto => "full_auto",
        }
    }

    /// Get rank of work grade (higher = more capable)
    pub fn rank(&self) -> u8 {
        match self {
            Self::Ask => 0,
            Self::Edit => 1,
            Self::Agent => 2,
            Self::Safeguard => 3,
            Self::FullAuto => 4,
        }
    }
}

/// Work grade decision with reasoning
#[derive(Debug, Clone)]
pub struct WorkGradeDecision {
    /// Requested work grade
    pub requested: WorkGrade,
    /// Decided work grade
    pub decided: WorkGrade,
    /// Decision action (upgraded/downgraded/unchanged)
    pub decision_action: String,
    /// Reasons for decision
    pub reasons: Vec<String>,
    /// Risk score (0.0 to 1.0)
    pub risk_score: f64,
}

/// Determine action based on requested vs decided work grade
pub fn work_grade_action(requested: WorkGrade, decided: WorkGrade) -> String {
    if decided.rank() > requested.rank() {
        "upgraded".to_string()
    } else if decided.rank() < requested.rank() {
        "downgraded".to_string()
    } else {
        "unchanged".to_string()
    }
}

/// Decide appropriate work grade based on task characteristics and context
pub fn decide_work_grade(
    requested_grade: Option<&str>,
    plan: &crate::reinforcement::TaskPlanArtifact,
    is_workflow_execute: bool,
    runtime_healthy: bool,
    force_fail_fast: bool,
) -> WorkGradeDecision {
    let requested = WorkGrade::parse(requested_grade).unwrap_or({
        if is_workflow_execute {
            WorkGrade::FullAuto
        } else {
            WorkGrade::Agent
        }
    });

    let mut decided = requested;
    let mut reasons = Vec::new();

    let risk_score = ((plan.characteristics.complexity.min(5) as f64 / 5.0) * 0.4
        + if plan.characteristics.has_safety_concerns {
            0.25
        } else {
            0.0
        }
        + if plan.characteristics.involves_multiple_modules {
            0.15
        } else {
            0.0
        }
        + ((1.0_f64 - plan.routing.predicted_success_rate as f64).clamp(0.0, 1.0)) * 0.2
        + if runtime_healthy { 0.0 } else { 0.1 })
    .clamp(0.0, 1.0);

    if force_fail_fast || plan.characteristics.has_safety_concerns || risk_score >= 0.75 {
        decided = WorkGrade::Safeguard;
        reasons.push(
            "high-risk posture detected (safety/fail_fast/high risk score), enforce safeguard"
                .to_string(),
        );
    } else if is_workflow_execute && plan.characteristics.complexity >= 3 {
        decided = WorkGrade::FullAuto;
        reasons
            .push("workflow.execute with moderate+ complexity, promote to full_auto".to_string());
    } else if plan.characteristics.complexity >= 3 {
        decided = WorkGrade::Agent;
        reasons.push("multi-step complexity, promote to agent execution".to_string());
    } else if plan.characteristics.complexity <= 1
        && !plan.characteristics.has_safety_concerns
        && plan.routing.predicted_success_rate >= 0.90
    {
        decided = WorkGrade::Edit;
        reasons.push("low-risk simple task, downgrade to edit for efficiency".to_string());
    }

    let decision_action = work_grade_action(requested, decided);
    WorkGradeDecision {
        requested,
        decided,
        decision_action,
        reasons,
        risk_score,
    }
}

/// Optimization policy report
#[derive(Debug, Clone, Serialize)]
pub struct OptimizationPolicyReport {
    /// Whether auto-attach is enabled
    pub auto_attach: bool,
    /// Whether auto-detach is enabled
    pub auto_detach: bool,
    /// Whether runtime is healthy
    pub runtime_healthy: bool,
    /// Whether anomaly was detected
    pub anomaly_detected: bool,
    /// Requested optimization modules
    pub requested_modules: Vec<String>,
    /// Attached optimization modules
    pub attached_modules: Vec<String>,
    /// Detached optimization modules
    pub detached_modules: Vec<String>,
    /// Reattached optimization modules
    pub reattached_modules: Vec<String>,
    /// Reasons for reattachment
    pub reattach_reasons: Vec<String>,
    /// Reasons for detachment
    pub detachment_reasons: Vec<String>,
    /// Module impact descriptions
    pub module_impacts: Vec<String>,
    /// Recovery conditions
    pub recovery_conditions: Vec<String>,
    /// Recommendations
    pub recommendations: Vec<String>,
    /// Phase parallelism cap
    pub phase_parallelism_cap: Option<usize>,
    /// Whether to force fail-fast mode
    pub force_fail_fast: bool,
    /// Risk assessment data
    pub risk_assessment: Value,
    /// Resource budget data
    pub resource_budget: Value,
    /// Dynamic parameters data
    pub dynamic_parameters: Value,
    /// Reliability data
    pub reliability: Value,
    /// Speed data
    pub speed: Value,
    /// Cost data
    pub cost: Value,
    /// Anomaly detection data
    pub anomaly: Value,
}

/// Optimization policy outcome
#[derive(Debug, Clone)]
pub struct OptimizationPolicyOutcome {
    /// Policy report
    pub report: OptimizationPolicyReport,
    /// Phase parallelism cap
    pub phase_parallelism_cap: Option<usize>,
    /// Whether to force fail-fast mode
    pub force_fail_fast: bool,
}

/// Check if module name is supported
pub fn is_supported_optimization_module(name: &str) -> bool {
    matches!(
        name,
        "workflow_optimizer"
            | "adaptive_selector"
            | "advanced_modules"
            | "cost_optimizer"
            | "speed_optimizer"
            | "reliability_optimizer"
            | "failure_prevention"
    )
}

/// Evaluate optimization policy based on task characteristics and runtime state
pub fn evaluate_optimization_policy(
    ledger: &ArtifactLedger,
    _task: &str,
    _plan: &crate::reinforcement::TaskPlanArtifact,
    options: Option<&PhaseOptions>,
    runtime_healthy: bool,
    is_workflow_execute: bool,
) -> OptimizationPolicyOutcome {
    let auto_attach = extra_bool(options, "auto_attach").unwrap_or(is_workflow_execute);
    let auto_detach = extra_bool(options, "auto_detach").unwrap_or(is_workflow_execute);

    let requested_modules = extra_string_list(options, "optimization_modules")
        .map(|modules: Vec<String>| {
            modules
                .into_iter()
                .map(|name: String| name.trim().to_ascii_lowercase())
                .filter(|name: &String| is_supported_optimization_module(name))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mut attached_modules = if auto_attach {
        if requested_modules.is_empty() {
            vec![
                "workflow_optimizer".to_string(),
                "advanced_modules".to_string(),
                "reliability_optimizer".to_string(),
                "failure_prevention".to_string(),
                "speed_optimizer".to_string(),
                "cost_optimizer".to_string(),
                "adaptive_selector".to_string(),
            ]
        } else {
            requested_modules.clone()
        }
    } else {
        Vec::new()
    };

    attached_modules.sort();
    attached_modules.dedup();

    let mut detached_modules = Vec::new();
    let mut reattached_modules = Vec::new();
    let mut reattach_reasons = Vec::new();
    let detachment_reasons = Vec::new();
    let mut module_impacts = Vec::new();
    let mut recovery_conditions = Vec::new();
    let recommendations = Vec::new();
    let phase_parallelism_cap = None;
    let force_fail_fast = false;

    let risk_assessment = Value::Null;
    let resource_budget = Value::Null;
    let dynamic_parameters = Value::Null;
    let reliability = Value::Null;
    let speed = Value::Null;
    let cost = Value::Null;
    let anomaly = Value::Null;
    let anomaly_detected = false;

    if auto_attach && auto_detach {
        let recoverable = recommend_reattach_modules_from_policy_history(ledger, 2, 40);
        for module in recoverable {
            if is_supported_optimization_module(&module)
                && !attached_modules.iter().any(|attached| attached == &module)
            {
                attached_modules.push(module.clone());
                reattached_modules.push(module.clone());
                reattach_reasons.push(format!(
                    "reattached {} after policy history reported two consecutive healthy, anomaly-free executions",
                    module
                ));
                module_impacts.push(format!(
                    "{} reattached to restore optimization depth under healthy runtime conditions",
                    module
                ));
            }
        }
    }

    // Simplified implementation - in full version, this would call various optimizers
    // For migration purposes, we keep the structure but simplify the implementation

    detached_modules.sort();
    detached_modules.dedup();
    reattached_modules.sort();
    reattached_modules.dedup();
    reattach_reasons.sort();
    reattach_reasons.dedup();
    module_impacts.sort();
    module_impacts.dedup();
    recovery_conditions.sort();
    recovery_conditions.dedup();
    attached_modules.retain(|module| !detached_modules.iter().any(|detached| detached == module));

    let report = OptimizationPolicyReport {
        auto_attach,
        auto_detach,
        runtime_healthy,
        anomaly_detected,
        requested_modules,
        attached_modules,
        detached_modules,
        reattached_modules,
        reattach_reasons,
        detachment_reasons,
        module_impacts,
        recovery_conditions,
        recommendations,
        phase_parallelism_cap,
        force_fail_fast,
        risk_assessment,
        resource_budget,
        dynamic_parameters,
        reliability,
        speed,
        cost,
        anomaly,
    };

    OptimizationPolicyOutcome {
        phase_parallelism_cap,
        force_fail_fast,
        report,
    }
}

/// Get role keywords for agent ranking.
/// For built-in roles returns static keyword slices.
/// For custom roles (unrecognised names), returns no static keywords so the
/// dynamic path in `rank_execution_agents` handles them via the registry.
pub fn role_keywords_for(role: &str) -> Vec<&'static str> {
    match role {
        "planner" => vec!["planner", "plan", "architect"],
        "researcher" => vec!["researcher", "research", "analysis"],
        "coder" => vec!["coder", "code", "implement", "dev"],
        "tester" => vec!["tester", "test", "qa", "verify"],
        "reviewer" => vec!["reviewer", "review", "audit"],
        // Custom roles: return empty – dynamic keyword lookup happens elsewhere.
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::task_router::{RoutingDecision, TaskCharacteristics, TaskType};
    use crate::roles::AgentRole;
    use crate::reinforcement::TaskPlanArtifact;

    fn make_task_plan(complexity: u8, safety: bool, multi_module: bool, success_rate: f32) -> TaskPlanArtifact {
        TaskPlanArtifact {
            generated_at: 0,
            task: "test task".to_string(),
            characteristics: TaskCharacteristics {
                description: "test".to_string(),
                task_type: TaskType::BugFix,
                complexity,
                required_capabilities: vec![],
                involves_multiple_modules: multi_module,
                is_time_critical: false,
                needs_verification: false,
                has_safety_concerns: safety,
            },
            routing: RoutingDecision {
                roles: vec![AgentRole::Coder],
                requirements: vec![],
                predicted_success_rate: success_rate,
                estimated_duration_seconds: 30,
                can_parallelize: vec![],
                risk_factors: vec![],
                recommended_safeguards: vec![],
                pua_enforcement: crate::pua::PuaEnforcementPlan {
                    escalation_level: String::new(),
                    mandatory_roles: vec![],
                    red_lines: vec![],
                    quality_compass: vec![],
                    mandatory_safeguards: vec![],
                    mandatory_evidence: vec![],
                    stage_requirements: vec![],
                },
            },
            decomposition: None,
            planned_subtasks: vec![],
            sub_agent_recommended: false,
            activation_reasons: vec![],
            action_checks_required: vec![],
        }
    }

    // ── WorkGrade ──────────────────────────────────────────────────────

    #[test]
    fn test_work_grade_parse_known_variants() {
        assert_eq!(WorkGrade::parse(Some("ask")), Some(WorkGrade::Ask));
        assert_eq!(WorkGrade::parse(Some("edit")), Some(WorkGrade::Edit));
        assert_eq!(WorkGrade::parse(Some("agent")), Some(WorkGrade::Agent));
        assert_eq!(WorkGrade::parse(Some("safeguard")), Some(WorkGrade::Safeguard));
        assert_eq!(WorkGrade::parse(Some("full_auto")), Some(WorkGrade::FullAuto));
        assert_eq!(WorkGrade::parse(Some("auto")), Some(WorkGrade::FullAuto));
    }

    #[test]
    fn test_work_grade_parse_unknown_returns_none() {
        assert_eq!(WorkGrade::parse(Some("unknown")), None);
        assert_eq!(WorkGrade::parse(Some("")), None);
        assert_eq!(WorkGrade::parse(None), None);
    }

    #[test]
    fn test_work_grade_rank_ordering() {
        assert!(WorkGrade::FullAuto.rank() > WorkGrade::Safeguard.rank());
        assert!(WorkGrade::Safeguard.rank() > WorkGrade::Agent.rank());
        assert!(WorkGrade::Agent.rank() > WorkGrade::Edit.rank());
        assert!(WorkGrade::Edit.rank() > WorkGrade::Ask.rank());
    }

    #[test]
    fn test_work_grade_action_detects_differences() {
        assert_eq!(work_grade_action(WorkGrade::Ask, WorkGrade::Agent), "upgraded");
        assert_eq!(work_grade_action(WorkGrade::Agent, WorkGrade::Edit), "downgraded");
        assert_eq!(work_grade_action(WorkGrade::Agent, WorkGrade::Agent), "unchanged");
    }

    // ── ReviewPolicy ───────────────────────────────────────────────────

    #[test]
    fn test_resolve_review_policy_standard_defaults() {
        let policy = resolve_review_policy(None, None, false, false);
        assert_eq!(policy.min_review_level, "standard");
        assert_eq!(policy.required_reviews, 1);
        assert!(policy.required_checks.is_empty());
        assert!(!policy.enforce_dual_review);
        assert!(!policy.enforce_action_gates);
    }

    #[test]
    fn test_resolve_review_policy_workflow_enforces_enhanced() {
        let policy = resolve_review_policy(None, None, true, false);
        assert_eq!(policy.min_review_level, "enhanced");
        assert_eq!(policy.required_reviews, 2);
        assert!(policy.enforce_dual_review);
    }

    // ── decide_work_grade ──────────────────────────────────────────────

    #[test]
    fn test_decide_work_grade_high_safety_escalates_to_safeguard() {
        let plan = make_task_plan(3, true, false, 0.8);
        let decision = decide_work_grade(Some("agent"), &plan, false, true, false);
        // Safeguard has rank 3 > Agent rank 2, so it's an upgrade
        assert_eq!(decision.decided, WorkGrade::Safeguard);
        assert_eq!(decision.decision_action, "upgraded");
    }

    #[test]
    fn test_decide_work_grade_low_complexity_promotes_to_edit() {
        let plan = make_task_plan(1, false, false, 0.95);
        let decision = decide_work_grade(Some("agent"), &plan, false, true, false);
        assert_eq!(decision.decided, WorkGrade::Edit);
        assert_eq!(decision.decision_action, "downgraded");
    }

    #[test]
    fn test_decide_work_grade_workflow_promotes_to_full_auto() {
        let plan = make_task_plan(3, false, false, 0.8);
        let decision = decide_work_grade(None, &plan, true, true, false);
        assert_eq!(decision.decided, WorkGrade::FullAuto);
    }

    // ── role_keywords_for ─────────────────────────────────────────────

    #[test]
    fn test_role_keywords_for_known_roles() {
        assert!(!role_keywords_for("planner").is_empty());
        assert!(!role_keywords_for("researcher").is_empty());
        assert!(!role_keywords_for("coder").is_empty());
        assert!(!role_keywords_for("tester").is_empty());
        assert!(!role_keywords_for("reviewer").is_empty());
    }

    #[test]
    fn test_role_keywords_for_unknown_role_returns_empty() {
        assert!(role_keywords_for("custom_role").is_empty());
    }

    // ── rank_execution_agents ─────────────────────────────────────────

    #[test]
    fn test_rank_execution_agents_returns_candidates() {
        let agents = vec![
            "coder-agent".to_string(),
            "tester-agent".to_string(),
            "researcher-agent".to_string(),
        ];
        let candidates = rank_execution_agents(&agents, Some("coder"), 0, 0);
        assert_eq!(candidates.len(), 3);
        for c in &candidates {
            assert!(c.score >= 0.0 && c.score <= 1.0, "score should be in [0, 1]");
        }
        // Should be sorted descending by score
        for i in 1..candidates.len() {
            assert!(
                candidates[i - 1].score >= candidates[i].score,
                "candidates should be sorted by score descending"
            );
        }
    }

    #[test]
    fn test_rank_execution_agents_empty_list() {
        let agents: Vec<String> = vec![];
        let candidates = rank_execution_agents(&agents, Some("coder"), 0, 0);
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_rank_execution_agents_no_role_constraint() {
        let agents = vec!["agent-a".to_string(), "agent-b".to_string()];
        let candidates = rank_execution_agents(&agents, None, 0, 0);
        assert_eq!(candidates.len(), 2);
    }

    // ── is_supported_optimization_module ───────────────────────────────

    #[test]
    fn test_is_supported_optimization_module() {
        assert!(is_supported_optimization_module("workflow_optimizer"));
        assert!(is_supported_optimization_module("adaptive_selector"));
        assert!(is_supported_optimization_module("cost_optimizer"));
        assert!(is_supported_optimization_module("failure_prevention"));
        assert!(!is_supported_optimization_module("unknown_module"));
    }
}

/// Rank execution agents based on role match and rotation
pub fn rank_execution_agents(
    agent_names: &[String],
    desired_role: Option<&str>,
    phase_index: usize,
    task_index: usize,
) -> Vec<ExecutionDecisionCandidate> {
    if agent_names.is_empty() {
        return Vec::new();
    }

    let total = agent_names.len() as f64;
    let mut ranked = agent_names
        .iter()
        .enumerate()
        .map(|(idx, agent_name)| {
            let lower = agent_name.to_ascii_lowercase();
            let history_order_score =
                ((agent_names.len().saturating_sub(idx)) as f64 / total) * 0.55;

            let (role_match_score, role_reason) = if let Some(role) = desired_role {
                let role = role.to_ascii_lowercase();
                let keywords = role_keywords_for(role.as_str());
                let dynamic_keywords = if keywords.is_empty() {
                    role_registry_keywords_for(role.as_str())
                } else {
                    Vec::new()
                };
                let static_match =
                    !keywords.is_empty() && keywords.iter().any(|keyword| lower.contains(keyword));
                let dynamic_match = !dynamic_keywords.is_empty()
                    && dynamic_keywords
                        .iter()
                        .any(|keyword| lower.contains(&keyword.to_ascii_lowercase()));
                if static_match || dynamic_match {
                    (0.35f64, format!("role match for {}", role))
                } else if keywords.is_empty() {
                    // Custom or unknown role: neutral score (no penalty)
                    (0.0f64, format!("custom role neutral for {}", role))
                } else {
                    (-0.12f64, format!("no explicit role match for {}", role))
                }
            } else {
                (0.08f64, "no role constraint".to_string())
            };

            let rotation_target = (phase_index + task_index) % agent_names.len();
            let spread_score = if idx == rotation_target { 0.10 } else { 0.02 };
            let score = (history_order_score + role_match_score + spread_score).clamp(0.0, 1.0);

            ExecutionDecisionCandidate {
                agent: agent_name.clone(),
                score,
                reason: format!(
                    "history_order={:.3}, {}, spread_score={:.3}",
                    history_order_score, role_reason, spread_score
                ),
            }
        })
        .collect::<Vec<_>>();

    ranked.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.agent.cmp(&b.agent))
    });
    ranked
}
