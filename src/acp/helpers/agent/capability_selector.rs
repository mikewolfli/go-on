//! CapabilityBus agent selection for ACP chat runtime
//!
//! This module extracts the CapabilityBus sense/decide pipeline from the
//! chat request handler into a reusable helper.  It prunes the agent list
//! to only the capability-bus-recommended agent and records the decision
//! as an observable event.

use std::sync::Arc;

use serde_json::Value;

use crate::acp::helpers::autonomy_metrics::record_capability_selection_reason;
use crate::acp::r#impl::chat::{reorder_agents_with_priority, RiskAssessment};
use crate::agent::{Agent, Message};
use crate::governance::pua::{TaskContext, TaskType};
use crate::intelligence::capability_bus::core::CapabilityBus;

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
        task_type: TaskType::Other,
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
        // Prune to only the capability-bus-recommended agent.
        // SAFETY: retain before reorder — if the agent is not in the list
        // we fall through to the phase-level agents unchanged.
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
        // reorder is now redundant since retain already reduced to one,
        // but kept for clarity in case retain logic changes in future.
        let _ = reorder_agents_with_priority(agents, agent);
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
