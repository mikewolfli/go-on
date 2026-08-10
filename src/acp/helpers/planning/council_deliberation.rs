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
use crate::orchestration::council::{
    CouncilMember, CouncilProposal, CouncilVote, ProposalStatus, VoteResult,
};
use crate::shared::option_bool;

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
                if crate::acp::r#impl::chat::reorder_agents_with_priority(agents, &winner) {
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
/// Honest semantics: without an LLM-backed ballot, the "council" cannot
/// produce independent per-member votes. Each candidate member therefore
/// casts a real self-endorsement vote (member votes for itself with nominal
/// voting power), the proposal is tallied, and the tally fields
/// (`total_votes`/`option_tallies`/`passed`/`tie`) in the decision payload
/// reflect that real recorded data. The route itself is still selected by
/// reputation ranking (the source of truth — the tally cannot decide
/// between self-endorsements), and vote-accuracy learning is not simulated
/// (`record_vote_accuracy` was removed as unwired).
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

    // Reputation-ranked route selection (source of truth for the winner).
    let winner = candidate_agents.iter().cloned().max_by(|a, b| {
        let sa = reputation_scores.get(a).copied().unwrap_or(0.5);
        let sb = reputation_scores.get(b).copied().unwrap_or(0.5);
        sa.partial_cmp(&sb)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.cmp(a))
    })?;

    let council = cb.council.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("lock poisoned, recovering");
        poisoned.into_inner()
    });
    for agent in candidate_agents {
        // get-or-insert: repeated requests for the same agent must not spam
        // "member already exists" warnings on every high-risk request.
        if council.get_member(agent).is_err() {
            if let Err(e) = council.add_member(CouncilMember {
                id: agent.clone(),
                name: agent.clone(),
                role: "routing_member".to_string(),
                voting_power: 100,
                specializations: vec![phase_name.to_string()],
                is_active: true,
                joined_ms: now_ms,
            }) {
                warn!(
                    phase = %phase_name,
                    agent = %agent,
                    error = %e,
                    "council_deliberation: failed to add council member"
                );
            }
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
    // Best-effort observability record. Expire old proposals first so the
    // capped proposal store does not silently stop recording (the store was
    // previously unbounded-in-effect and rejected new records at max_proposals).
    let mut submitted_id = proposal_id.clone();
    if council.submit_proposal(proposal).is_err() {
        let _ = council.expire_old_proposals();
        let retry = CouncilProposal {
            id: format!("{}-{}-{}", phase_name, now_ms, candidate_agents.len()),
            title: format!("Route selection for phase {}", phase_name),
            description: "Select the best routing agent for this high-complexity request"
                .to_string(),
            submitted_by: "capability_bus".to_string(),
            options: candidate_agents.to_vec(),
            status: ProposalStatus::Active,
            created_ms: now_ms,
        };
        if council.submit_proposal(retry).is_err() {
            warn!(
                phase = %phase_name,
                "council_deliberation: proposal store full; routing continues with reputation ranking"
            );
            return Some((
                winner.clone(),
                json!({
                    "proposal_id": proposal_id,
                    "winner": winner,
                    "selection": "reputation_ranked",
                    "tie": false,
                    "passed": false,
                    "total_votes": 0,
                    "option_tallies": {},
                    "candidate_count": candidate_agents.len(),
                }),
            ));
        }
        submitted_id = format!("{}-{}-{}", phase_name, now_ms, candidate_agents.len());
    }

    // Cast real votes: each member endorses itself with its nominal voting
    // power (no LLM ballot exists, so a self-endorsement is the honest vote).
    // This seeds the council reputation map (`ensure_reputation`) so the
    // auto-ejection scan has real data, and `tally_votes` records the outcome.
    for agent in candidate_agents {
        if let Err(e) = council.cast_vote(CouncilVote {
            member_id: agent.clone(),
            proposal_id: submitted_id.clone(),
            selected_option: agent.clone(),
            weight: 100,
            vote_ms: now_ms,
            rationale: Some("self-endorsement (no LLM ballot)".to_string()),
        }) {
            warn!(
                phase = %phase_name,
                agent = %agent,
                error = %e,
                "council_deliberation: failed to cast self-endorsement vote"
            );
        }
    }
    let vote_result = match council.tally_votes(&submitted_id) {
        Ok(result) => result,
        Err(e) => {
            warn!(
                phase = %phase_name,
                error = %e,
                "council_deliberation: tally failed; reporting no-vote outcome"
            );
            VoteResult {
                proposal_id: submitted_id.clone(),
                option_tallies: HashMap::new(),
                total_votes: 0,
                passed: false,
                winning_option: None,
                tie: false,
            }
        }
    };

    Some((
        winner.clone(),
        json!({
            "proposal_id": submitted_id,
            "winner": winner,
            "selection": "reputation_ranked",
            "tie": vote_result.tie,
            "passed": vote_result.passed,
            "total_votes": vote_result.total_votes,
            "option_tallies": vote_result.option_tallies,
            "candidate_count": candidate_agents.len(),
        }),
    ))
}

// NOTE: `reorder_agents_with_priority` lives once in crate::acp::r#impl::chat
// (deduplicated from this module); council deliberation calls it via that path.
