//! CapabilityBus agent selection for ACP chat runtime
//!
//! This module extracts the CapabilityBus sense/decide pipeline from the
//! chat request handler into a reusable helper.  It prunes the agent list
//! to only the capability-bus-recommended agent and records the decision
//! as an observable event.

use std::sync::Arc;

use serde_json::Value;

use crate::acp::helpers::autonomy_metrics::record_capability_selection_reason;
use crate::acp::r#impl::chat::RiskAssessment;
use crate::agent::{Agent, Message};
use crate::governance::pua::{TaskContext, TaskType};
use crate::intelligence::capability_bus::core::CapabilityBus;
use crate::orchestration::task_router::{TaskCharacteristics, TaskRouter};

/// Result of CapabilityBus agent selection.
pub(crate) struct CapabilitySelectionResult {
    pub capability_selected_agent: Option<String>,
    pub recommended_mode: Option<String>,
    pub candidate_count: usize,
    pub confidence: f64,
    pub capability_selection_reason: String,
    pub optimization_hint: Option<Value>,
}

/// Applies CapabilityBus agent selection to prune the agent list.
///
/// Uses the bus's sense/decide pipeline to refine or override the agent
/// list, mutates `agents` in place (pruning to the recommended agent),
/// pushes provenance entries into `routing_provenance`, and records the
/// decision as a CapabilityBus event.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn apply_capability_bus_selection(
    cb: &CapabilityBus,
    phase_name: &str,
    messages: &[Message],
    mode: &str,
    agents: &mut Vec<(String, Arc<dyn Agent>)>,
    risk_assessment: &RiskAssessment,
    request_id: &str,
    routing_provenance: &mut Vec<String>,
) -> CapabilitySelectionResult {
    let task_ctx = TaskContext {
        // Classify the task from the last user message via the authoritative
        // `TaskRouter::analyze_task` (formerly hardcoded to `TaskType::Other`,
        // which froze task_fit_score at 0.60, the recent-outcome target_task
        // at "Other", the workflow preset at "general", and the UCB context
        // task dimension at "Other" for every request).
        task_type: map_pua_task_type(&TaskRouter::analyze_task(&latest_user_message_content(
            messages,
        ))),
        // NOTE: `file_count` here is the message count (pre-existing
        // semantics) — the field name means "changed file count" in the
        // request-path context (see `infer_file_count` in request.rs), so the
        // token estimate in `sense()` is based on conversation size rather
        // than files touched. Impact is limited to the token-usage estimate.
        file_count: messages.len(),
        risk_score: (risk_assessment.score as f64 / 4.0).clamp(0.1, 1.0),
    };
    let sensing = cb.sense(&task_ctx);
    let decision = cb.decide(&task_ctx, &sensing).await;
    let capability_selected_agent = decision.selected_agent.clone();
    let recommended_mode = Some(decision.recommended_mode.clone());
    let candidate_count = sensing.capability_agent_count;
    let confidence = decision.confidence;
    let capability_selection_reason: String;
    let optimization_hint: Option<Value> = if cfg!(feature = "sub-bus-optimization") {
        let opt = cb.optimization_recommendation(
            phase_name,
            (messages.len() as u64).saturating_mul(512),
            if mode.eq_ignore_ascii_case("full_auto") {
                "high"
            } else {
                "balanced"
            },
        );
        Some(serde_json::json!({
            "suggested_agent": opt.suggested_agent,
            "estimated_cost": opt.estimated_cost,
            "estimated_duration_ms": opt.estimated_duration_ms,
            "reliability_score": opt.reliability_score,
            "confidence": opt.confidence,
        }))
    } else {
        None
    };

    if let Some(ref agent) = decision.selected_agent {
        // Prune to only the capability-bus-recommended agent. If the agent is
        // not in the list, fall through to the phase-level agents unchanged.
        if agents.iter().any(|(name, _)| name == agent) {
            agents.retain(|(name, _)| name == agent);
            capability_selection_reason = "capability_bus_selected".to_string();
            routing_provenance.push("capability_bus_selected_agent_applied".to_string());
            record_capability_selection_reason("capability_bus_selected");
        } else {
            capability_selection_reason = "capability_bus_no_match".to_string();
            routing_provenance.push("capability_bus_selected_agent_not_in_candidates".to_string());
            record_capability_selection_reason("capability_bus_no_match");
        }
        // retain() above already reduced the candidate list to the single
        // capability-bus-recommended agent, so no reordering is needed.
    } else {
        capability_selection_reason = "capability_bus_none".to_string();
        routing_provenance.push("capability_bus_no_selected_agent".to_string());
        record_capability_selection_reason("capability_bus_none");
    }

    // Record the routing decision as an observable event
    cb.record_event(
        "sense",
        decision.selected_agent.clone(),
        Some(request_id.to_string()),
        "success",
        serde_json::json!({
            "candidate_count": sensing.capability_agent_count,
            "confidence": decision.confidence,
            "duration_ms": decision.duration_ms,
            "recommended_mode": decision.recommended_mode,
            "high_risk": risk_assessment.is_high_risk,
            "risk_reasons": risk_assessment.reasons,
            "optimization": optimization_hint,
        }),
    );

    CapabilitySelectionResult {
        capability_selected_agent,
        recommended_mode,
        candidate_count,
        confidence,
        capability_selection_reason,
        optimization_hint,
    }
}

/// Content of the last `user` message, falling back to the last message when
/// no user message exists, and to an empty string when `messages` is empty.
///
/// Single source of truth: delegates to `extract_task_description` (chat.rs)
/// — this module previously re-implemented the identical extraction inline,
/// so the two could drift.
fn latest_user_message_content(messages: &[Message]) -> String {
    crate::acp::r#impl::chat::extract_task_description(messages)
}

/// Map the authoritative `TaskRouter::analyze_task` classification onto the
/// PUA `TaskType` vocabulary (BugFix/FeatureAdd/Refactor/SecurityPatch/Other).
///
/// `analyze_task` has no SecurityPatch variant; its `has_safety_concerns` flag
/// (security/safe/memory/delete/drop) is the primary gate for that mapping,
/// narrowed by an explicit `security`/`patch` keyword so plain destructive
/// tasks ("delete", "drop") are not mislabeled as security patches. The
/// security check runs before the BugFix match so "fix security vulnerability"
/// classifies as SecurityPatch, not BugFix.
fn map_pua_task_type(chars: &TaskCharacteristics) -> TaskType {
    use crate::orchestration::task_router::TaskType as RouterTaskType;
    let lower = chars.description.to_lowercase();
    if chars.has_safety_concerns && (lower.contains("security") || lower.contains("patch")) {
        TaskType::SecurityPatch
    } else {
        match chars.task_type {
            RouterTaskType::BugFix => TaskType::BugFix,
            RouterTaskType::FeatureImplementation => TaskType::FeatureAdd,
            RouterTaskType::Refactoring => TaskType::Refactor,
            // TestImplementation / Documentation / ArchitectureDesign /
            // PerformanceOptimization / CodeReview / Unknown → Other: the PUA
            // vocabulary has no finer-grained buckets for these.
            _ => TaskType::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_msg(content: &str) -> Message {
        Message {
            role: "user".to_string(),
            content: content.to_string(),
        }
    }

    fn map(desc: &str) -> TaskType {
        map_pua_task_type(&TaskRouter::analyze_task(desc))
    }

    #[test]
    fn maps_bug_fix_message() {
        assert_eq!(map("fix the login bug"), TaskType::BugFix);
    }

    #[test]
    fn maps_feature_message() {
        assert_eq!(
            map("implement a new feature for the dashboard"),
            TaskType::FeatureAdd
        );
    }

    #[test]
    fn maps_refactor_message() {
        assert_eq!(map("refactor the routing module"), TaskType::Refactor);
    }

    #[test]
    fn maps_security_patch_message() {
        // "fix security vulnerability" is classified BugFix by analyze_task but
        // must map to SecurityPatch — the safety gate takes precedence.
        assert_eq!(
            map("apply a security patch to the auth module"),
            TaskType::SecurityPatch
        );
        assert_eq!(
            map("fix security vulnerability in the login flow"),
            TaskType::SecurityPatch
        );
    }

    #[test]
    fn maps_plain_and_empty_to_other() {
        assert_eq!(map("update the documentation"), TaskType::Other);
        assert_eq!(map(""), TaskType::Other);
        // Destructive-but-not-security keywords stay Other (has_safety_concerns
        // alone does not imply a security patch).
        assert_eq!(map("delete the stale rows"), TaskType::Other);
    }

    #[test]
    fn latest_user_message_extraction_falls_back_gracefully() {
        // Empty messages → empty description → Other.
        assert!(latest_user_message_content(&[]).is_empty());
        assert_eq!(
            map_pua_task_type(&TaskRouter::analyze_task(&latest_user_message_content(&[]))),
            TaskType::Other
        );

        // Last user message wins over earlier ones / trailing non-user messages.
        let msgs = vec![
            user_msg("fix the login bug"),
            Message {
                role: "assistant".to_string(),
                content: "refactor the routing module".to_string(),
            },
            user_msg("implement a new feature"),
        ];
        let desc = latest_user_message_content(&msgs);
        assert_eq!(desc, "implement a new feature");
        assert_eq!(
            map_pua_task_type(&TaskRouter::analyze_task(&desc)),
            TaskType::FeatureAdd
        );
    }
}
