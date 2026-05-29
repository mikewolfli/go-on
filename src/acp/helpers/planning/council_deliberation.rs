//! Council deliberation and degraded fallback helpers for ACP chat
//!
//! This module extracts two related concerns from `process_chat_request`:
//!
//! 1. **Degraded fallback** — when all candidate agents are marked unhealthy
//!    by the CapabilityBus, forces the first candidate through so the request
//!    can make progress instead of failing fast.
//!
//! 2. **Council deliberation** — for high-risk, multi-agent requests with the
//!    `council_deliberation_enabled` option turned on, runs a council vote
//!    to select the best routing agent among candidates.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use tracing::warn;

use crate::acp::helpers::autonomy_metrics::record_fallback_reason;
use crate::agent::Agent;
use crate::intelligence::capability_bus::core::CapabilityBus;
use crate::orchestration::council::{CouncilMember, CouncilProposal, CouncilVote, ProposalStatus};

// ---------------------------------------------------------------------------
// Public entry-point
// ---------------------------------------------------------------------------

/// Runs the degraded-fallback and council-deliberation logic.
///
/// **Degraded fallback:** If all candidate agents are unhealthy according to the
/// CapabilityBus health checks, the first candidate is selected as the fallback
/// so that the request can still make progress.
///
/// **Council deliberation:** If the request is high-risk, has ≥2 agents,
/// and `council_deliberation_enabled` is true (and no specific model was
/// requested), a council-style vote is executed among candidate agents to
/// determine the preferred route.
///
/// # Returns
///
/// `(unhealthy_fallback_agent, fallback_reason, council_decision)`
///
/// * `unhealthy_fallback_agent` — The agent forced through in degraded mode, if any.
/// * `fallback_reason` — A machine-readable reason for the fallback, if triggered.
/// * `council_decision` — The JSON payload from the council deliberation, if run.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_council_deliberation_and_fallback(
    capability_bus: Option<&CapabilityBus>,
    is_high_risk: bool,
    model_is_specific: bool,
    agents: &mut Vec<(String, Arc<dyn Agent>)>,
    base_agent_options: &HashMap<String, Value>,
    phase_name: &str,
    reputation_scores: &HashMap<String, f64>,
    routing_provenance: &mut Vec<String>,
) -> (Option<String>, Option<String>, Option<Value>) {
    let mut fallback_reason: Option<String> = None;
    let mut council_decision: Option<Value> = None;

    // ── Degraded fallback ──────────────────────────────────────────────
    // If all candidates are marked unhealthy, still attempt the first
    // candidate so the request can make progress instead of failing fast.
    let unhealthy_fallback_agent = if let Some(cb) = capability_bus {
        let healthy_count = agents
            .iter()
            .filter(|(name, _)| cb.is_agent_healthy(name))
            .count();
        if healthy_count == 0 {
            let selected = agents.first().map(|(name, _)| name.clone());
            if let Some(ref name) = selected {
                warn!(
                    phase = %phase_name,
                    fallback_agent = %name,
                    "all candidate agents unhealthy; forcing degraded fallback attempt"
                );
                fallback_reason = Some("all_agents_unhealthy".to_string());
                routing_provenance.push("degraded_fallback_all_agents_unhealthy".to_string());
                record_fallback_reason("all_agents_unhealthy");
            }
            selected
        } else {
            None
        }
    } else {
        None
    };

    // ── Council deliberation ───────────────────────────────────────────
    // For high-risk, multi-agent requests where the option is enabled, run
    // a council-style deliberation to select the best routing agent.
    let should_use_council_deliberation = is_high_risk
        && !model_is_specific
        && agents.len() >= 2
        && option_bool(base_agent_options, "council_deliberation_enabled", true);
    if should_use_council_deliberation {
        if let Some(cb) = capability_bus {
            let candidate_names = agents
                .iter()
                .map(|(name, _)| name.clone())
                .collect::<Vec<_>>();
            if let Some((winner, decision)) =
                run_council_route_deliberation(cb, phase_name, &candidate_names, reputation_scores)
            {
                if reorder_agents_with_priority(agents, &winner) {
                    routing_provenance.push("council_deliberation_selected_route".to_string());
                }
                council_decision = Some(decision);
            }
        }
    }

    (unhealthy_fallback_agent, fallback_reason, council_decision)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Runs a council-style deliberation among candidate agents to select the
/// best routing agent for the current request.
///
/// Each candidate agent is added as a council member and casts a reputation-
/// weighted vote. The candidate with the highest tally wins.
fn run_council_route_deliberation(
    cb: &CapabilityBus,
    phase_name: &str,
    candidate_agents: &[String],
    reputation_scores: &HashMap<String, f64>,
) -> Option<(String, Value)> {
    if candidate_agents.len() < 2 {
        return None;
    }

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let proposal_id = format!("route-{}-{}", phase_name, now_ms);

    let council = cb.council.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("lock poisoned, recovering");
        poisoned.into_inner()
    });
    for agent in candidate_agents {
        if let Err(e) = council.add_member(CouncilMember {
            id: agent.clone(),
            name: agent.clone(),
            role: "routing_member".to_string(),
            voting_power: 100,
            specializations: vec![phase_name.to_string()],
            is_active: true,
            joined_ms: now_ms,
        }) {
            tracing::warn!(
                phase = %phase_name,
                agent = %agent,
                error = %e,
                "council_deliberation: failed to add council member"
            );
        }
    }

    let proposal = CouncilProposal {
        id: proposal_id.clone(),
        title: format!("Route selection for phase {}", phase_name),
        description: "Select the best routing agent for this high-complexity request".to_string(),
        submitted_by: "capability_bus".to_string(),
        options: candidate_agents.to_vec(),
        status: ProposalStatus::Active,
        created_ms: now_ms,
    };
    if council.submit_proposal(proposal).is_err() {
        return None;
    }

    let winner_guess = candidate_agents.iter().cloned().max_by(|a, b| {
        let sa = reputation_scores.get(a).copied().unwrap_or(0.5);
        let sb = reputation_scores.get(b).copied().unwrap_or(0.5);
        sa.partial_cmp(&sb)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.cmp(a))
    })?;

    for agent in candidate_agents {
        let weight = (reputation_scores.get(agent).copied().unwrap_or(0.5) * 100.0)
            .round()
            .clamp(1.0, 100.0) as u32;
        if let Err(e) = council.cast_vote(CouncilVote {
            member_id: agent.clone(),
            proposal_id: proposal_id.clone(),
            selected_option: winner_guess.clone(),
            weight,
            vote_ms: now_ms,
            rationale: Some("Reputation-weighted route deliberation".to_string()),
        }) {
            tracing::warn!(
                phase = %phase_name,
                agent = %agent,
                error = %e,
                "council_deliberation: failed to cast council vote"
            );
        }
    }

    let tally = council.tally_votes(&proposal_id).ok()?;
    let winner = tally
        .winning_option
        .clone()
        .unwrap_or_else(|| winner_guess.clone());

    // BLUE48 Step 17: Update council member reputations based on vote accuracy.
    // This enables the reputation learning system to improve voting quality over time.
    if let Err(e) = council.record_vote_accuracy(&proposal_id, &tally.winning_option) {
        tracing::warn!(
            phase = %phase_name,
            proposal_id = %proposal_id,
            error = %e,
            "council_deliberation: failed to record vote accuracy"
        );
    }
    Some((
        winner,
        json!({
            "proposal_id": proposal_id,
            "winner": tally.winning_option,
            "tie": tally.tie,
            "passed": tally.passed,
            "total_votes": tally.total_votes,
            "option_tallies": tally.option_tallies,
            "candidate_count": candidate_agents.len(),
        }),
    ))
}

/// Reads a boolean option from the agent-options map with a default fallback.
fn option_bool(options: &HashMap<String, Value>, key: &str, default: bool) -> bool {
    options
        .get(key)
        .and_then(|v| v.as_bool())
        .unwrap_or(default)
}

// ---------------------------------------------------------------------------
// Re-export the reordering helper used by council deliberation
// ---------------------------------------------------------------------------
fn reorder_agents_with_priority(
    agents: &mut Vec<(String, Arc<dyn Agent>)>,
    preferred: &str,
) -> bool {
    if let Some(index) = agents.iter().position(|(name, _)| name == preferred) {
        if index > 0 {
            let selected = agents.remove(index);
            agents.insert(0, selected);
        }
        return true;
    }
    false
}
