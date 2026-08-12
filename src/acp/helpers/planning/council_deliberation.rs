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
//!
//! Reputation-safety note: every ballot here is a self-endorsement (no LLM
//! ballot exists), so outcomes are deliberately NOT recorded into council
//! member reputation (`record_outcome` is not called) — scoring self-
//! endorsement accuracy against the reputation-ranked winner would be a
//! self-produced feedback loop (see `run_council_route_deliberation`).

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
/// reflect that real recorded data. The primary route signal is the
/// reputation ranking; when two or more candidates share the top reputation
/// score (a genuine tie), the tally outcome (`winning_option`) decides the
/// winner — the tally is reputation-weighted via the council's
/// `effective_voting_power`, so it carries real information even though every
/// ballot is a self-endorsement.
///
/// Reputation-learning guard: because every ballot is a self-endorsement,
/// `council.record_outcome` is intentionally NOT called for these proposals.
/// Recording an outcome would mark each member's self-endorsement as
/// "accurate iff it equals the reputation-ranked winner" — i.e. the members'
/// own prior ranking would manufacture their accuracy, a self-reinforcing
/// loop that would inflate the winner's council influence (agent_selector's
/// `total_votes >= 3` boost) and drive `auto_eject_low_performers`
/// (quorum/consensus.rs) against the rest without any real evidence. Votes
/// are still cast (tally/quorum semantics preserved) but member reputation
/// records stay at their seeded state (`total_votes == 0`), so both
/// mechanisms remain dormant until a real, non-self-endorsement ballot path
/// exists.
fn run_council_route_deliberation(
    cb: &CapabilityBus,
    phase_name: &str,
    candidate_agents: &[String],
    reputation_scores: &HashMap<String, f64>,
) -> Option<(String, Value)> {
    if candidate_agents.len() < 2 {
        return None;
    }

    let now_ms = crate::shared::timestamps::now_ts_ms_u64();
    let proposal_id = format!("route-{}-{}", phase_name, now_ms);

    // Reputation-ranked route selection (primary source of truth for the winner).
    let score_of = |name: &str| {
        reputation_scores
            .get(name)
            .copied()
            .unwrap_or(crate::acp::helpers::agent_selector::DEFAULT_REPUTATION_SCORE)
    };
    let top_score = candidate_agents
        .iter()
        .map(|a| score_of(a))
        .fold(f64::NEG_INFINITY, f64::max);
    let reputation_tied: Vec<&String> = candidate_agents
        .iter()
        .filter(|a| score_of(a) == top_score)
        .collect();
    let reputation_tie = reputation_tied.len() > 1;
    // Deterministic fallback tiebreak among equal scores: `b.cmp(a)` below
    // preserves the original `max_by` behavior (alphabetically-earlier name
    // wins among equal scores).
    let winner_by_reputation = candidate_agents.iter().cloned().max_by(|a, b| {
        score_of(a)
            .partial_cmp(&score_of(b))
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
                winner_by_reputation.clone(),
                json!({
                    "proposal_id": proposal_id,
                    "winner": winner_by_reputation,
                    "selection": "reputation_ranked",
                    "reputation_tie": reputation_tie,
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
    // Votes are genuine records: they seed the council reputation map
    // (`ensure_reputation`) and the tally below is reputation-weighted, so
    // quorum/tally semantics are real. No outcome is recorded for these
    // ballots — see the reputation-learning guard in the doc comment above.
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

    // Route selection. Reputation ranking is the primary signal; when the top
    // reputation score is shared by several candidates, the tally outcome
    // genuinely breaks the tie (winning_option is reputation-weighted via
    // `effective_voting_power`). If the tally has no winner (all-tie or
    // quorum failure) or its winner is not among the reputation-tied
    // candidates, fall back to the deterministic reputation tiebreak.
    let (winner, selection) = if reputation_tie {
        match vote_result.winning_option.as_deref() {
            Some(w) if reputation_tied.iter().any(|c| c.as_str() == w) => {
                (w.to_string(), "council_tally_tiebreak")
            }
            _ => (winner_by_reputation.clone(), "reputation_ranked"),
        }
    } else {
        (winner_by_reputation.clone(), "reputation_ranked")
    };

    // Deliberately NOT recording an outcome for these ballots.
    //
    // Every vote here is a self-endorsement (member votes for itself), so
    // `council.record_outcome(&submitted_id, Some(&winner))` would score each
    // member's accuracy as "did I pick the reputation-ranked winner" — a
    // self-produced signal: the reputation-ranked winner is marked accurate
    // and every other member inaccurate, feeding `agent_selector`'s council
    // boost (total_votes >= 3) and `auto_eject_low_performers`
    // (quorum/consensus.rs) purely from the members' own prior ranking. That
    // is a self-reinforcing loop, not competence evidence, so self-
    // endorsement must not advance member reputation. `get_reputation(...)
    // .total_votes` therefore stays 0 and both mechanisms stay dormant.
    // Observability is preserved through the decision payload returned below
    // and the council's proposal/vote records.

    Some((
        winner.clone(),
        json!({
            "proposal_id": submitted_id,
            "winner": winner,
            "selection": selection,
            "reputation_tie": reputation_tie,
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

#[cfg(all(
    test,
    any(
        feature = "sub-bus-tool",
        feature = "simple-server",
        feature = "multi-users-server"
    )
))]
mod tests {
    use super::*;
    use crate::governance::harness_bus::default_harness_bus;
    use std::collections::HashMap;
    use std::sync::Arc;

    /// Self-endorsement ballots must not advance council member reputation:
    /// the routed winner (chosen by reputation ranking) must not receive an
    /// accuracy/boost record and the losers must not be penalized — otherwise
    /// the council's own prior ranking manufactures accuracy (pseudo-signal)
    /// that feeds `agent_selector`'s boost and `auto_eject_low_performers`.
    #[tokio::test]
    async fn self_endorsement_does_not_boost_member_reputation() {
        let harness = Arc::new(default_harness_bus());
        let bus = CapabilityBus::new_default(harness, None);

        let candidates = vec![
            "candidate-a".to_string(),
            "candidate-b".to_string(),
            "candidate-c".to_string(),
        ];
        let mut reputation_scores = HashMap::new();
        reputation_scores.insert("candidate-a".to_string(), 0.9);
        reputation_scores.insert("candidate-b".to_string(), 0.5);
        reputation_scores.insert("candidate-c".to_string(), 0.5);

        let (winner, decision) =
            run_council_route_deliberation(&bus, "test-phase", &candidates, &reputation_scores)
                .expect("deliberation should return a winner");

        // Decision result unchanged: reputation-ranked winner still selected.
        assert_eq!(winner, "candidate-a");
        assert_eq!(decision["winner"], "candidate-a");
        // Tally semantics preserved: 3 members × 100 nominal voting power each
        // (reputation weighting is inert during warmup).
        assert_eq!(decision["total_votes"], 300, "tally semantics preserved");
        assert_eq!(decision["candidate_count"], 3);

        let council = bus
            .council
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for agent in &candidates {
            let record = council
                .get_reputation(agent)
                .unwrap_or_else(|| panic!("{} should have a seeded reputation record", agent));
            assert_eq!(
                record.total_votes, 0,
                "self-endorsement must not be recorded as a competence outcome for {}",
                agent
            );
            assert_eq!(
                record.warmup_remaining, 5,
                "warmup must not be consumed by self-endorsement for {}",
                agent
            );
            assert_eq!(
                record.influence_multiplier, 1.0,
                "{} must stay neutral",
                agent
            );
        }
    }
}
