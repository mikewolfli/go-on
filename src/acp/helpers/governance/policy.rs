//! Policy helper functions for ACP server
//!
//! This module provides utility functions for managing review policies,
//! work grade decisions, optimization policies, and agent ranking.

use serde::Serialize;

use crate::acp::helpers::misc::{extra_string, extra_string_list, extra_u64};
use crate::config::PhaseOptions;
use crate::orchestration::roles::role_registry_keywords_for;
use crate::orchestration::task_router::TaskCharacteristics;
use crate::reinforcement::ExecutionDecisionCandidate;

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

#[cfg(test)]
mod tests {
    use super::*;

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
            assert!(
                c.score >= 0.0 && c.score <= 1.0,
                "score should be in [0, 1]"
            );
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

    // ── resolve_review_policy: edge cases ─────────────────────────────

    #[test]
    fn test_resolve_review_policy_no_options_returns_standard() {
        let policy = resolve_review_policy(None, None, false, false);
        assert_eq!(policy.min_review_level, "standard");
        assert_eq!(policy.required_reviews, 1);
        assert!(!policy.enforce_dual_review);
    }

    #[test]
    fn test_resolve_review_policy_workflow_execute_enhances() {
        let policy = resolve_review_policy(None, None, true, false);
        assert_eq!(policy.min_review_level, "enhanced");
        assert!(policy.enforce_action_gates);
    }

    // ── rank_execution_agents: edge cases ─────────────────────────────

    #[test]
    fn test_rank_execution_agents_with_roles() {
        let agents = [
            ("dev".to_string(), "developer".to_string()),
            ("rev".to_string(), "reviewer".to_string()),
        ];
        let ranked = rank_execution_agents(
            &agents.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
            Some("developer"),
            0,
            0,
        );
        // All agents are returned; dev should rank highest for developer role
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].agent, "dev");
    }

    #[test]
    fn test_rank_execution_agents_all_roles_filtered() {
        let agents = [
            ("dev".to_string(), "developer".to_string()),
            ("qa".to_string(), "tester".to_string()),
        ];
        let ranked = rank_execution_agents(
            &agents.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
            Some("nobody"),
            0,
            0,
        );
        // All agents are returned even when role has no match; scores are neutral
        assert_eq!(ranked.len(), 2);
        for c in &ranked {
            assert!(
                c.score >= 0.0 && c.score <= 1.0,
                "score should be in [0, 1]"
            );
        }
    }
}
