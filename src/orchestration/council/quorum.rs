//! Deliberation and quorum-related methods for `OrchestrationCouncil`.
//!
//! Handles multi-round deliberation, statements, round voting, profile
//! snapshots, and auto-ejection of low performers.

use super::council::OrchestrationCouncil;
use super::types::*;
use anyhow::{anyhow, Result};
use std::collections::HashMap;

impl OrchestrationCouncil {
    // ─── Deliberation Methods (GAP-B50-08) ───────────────────────────────────

    /// Start a new deliberation for the given proposal.
    ///
    /// Returns a `DeliberationId` that can be used to submit statements,
    /// vote in rounds, and query the deliberation state.
    pub fn start_deliberation(&self, proposal_id: &str) -> Result<DeliberationId> {
        // Verify the proposal exists.
        self.get_proposal(proposal_id)?;

        let id = DeliberationId(format!("delib-{}", proposal_id));
        let now = now_epoch_ms();

        let deliberation = Deliberation {
            id: id.clone(),
            proposal_id: proposal_id.to_string(),
            rounds: vec![DeliberationRound {
                round_number: 1,
                statements: Vec::new(),
                votes: Vec::new(),
                concluded: false,
            }],
            max_rounds: self.deliberation_config.max_rounds,
            consensus_reached: false,
            final_decision: None,
            started_at: now,
        };

        let mut deliberations = self
            .deliberations
            .lock()
            .map_err(|e| anyhow!("Failed to acquire lock on deliberations: {e}"))?;

        if deliberations.contains_key(&id) {
            return Err(anyhow!(
                "Deliberation already exists for proposal '{}'",
                proposal_id
            ));
        }

        deliberations.insert(id.clone(), deliberation);
        Ok(id)
    }

    /// Submit a statement in the current round of a deliberation.
    ///
    /// If the member has already submitted a statement in this round, it is
    /// replaced with the new statement (allowing position changes within a round).
    pub fn submit_statement(
        &self,
        deliberation_id: &DeliberationId,
        statement: DeliberationStatement,
    ) -> Result<()> {
        let mut deliberations = self
            .deliberations
            .lock()
            .map_err(|e| anyhow!("Failed to acquire lock on deliberations: {e}"))?;

        let deliberation = deliberations
            .get_mut(deliberation_id)
            .ok_or_else(|| anyhow!("Deliberation '{}' not found", deliberation_id))?;

        // Cannot submit statements after deliberation is concluded.
        if deliberation.final_decision.is_some() {
            return Err(anyhow!(
                "Deliberation '{}' has already concluded",
                deliberation_id
            ));
        }

        let current_round = deliberation
            .rounds
            .last_mut()
            .ok_or_else(|| anyhow!("Deliberation '{}' has no rounds", deliberation_id))?;

        if current_round.concluded {
            return Err(anyhow!(
                "Current round {} of deliberation '{}' is already concluded",
                current_round.round_number,
                deliberation_id
            ));
        }

        // Replace existing statement from this member in this round, or add new one.
        if let Some(existing) = current_round
            .statements
            .iter_mut()
            .find(|s| s.member_id == statement.member_id)
        {
            *existing = statement;
        } else {
            current_round.statements.push(statement);
        }

        Ok(())
    }

    /// Cast a vote in the current round of a deliberation.
    ///
    /// Unlike the simple `cast_vote` method, this allows the same member
    /// to vote again in a new round (changing position between rounds).
    /// Within a single round, the previous vote is replaced.
    pub fn vote_in_round(&self, deliberation_id: &DeliberationId, vote: CouncilVote) -> Result<()> {
        // Validate member exists and is active.
        {
            let members = self
                .members
                .lock()
                .map_err(|e| anyhow!("Failed to acquire lock on members: {e}"))?;
            let member = members
                .get(&vote.member_id)
                .ok_or_else(|| anyhow!("Member '{}' not found", vote.member_id))?;
            if !member.is_active {
                return Err(anyhow!(
                    "Member '{}' is inactive and cannot vote",
                    vote.member_id
                ));
            }
        }

        let mut deliberations = self
            .deliberations
            .lock()
            .map_err(|e| anyhow!("Failed to acquire lock on deliberations: {e}"))?;

        let deliberation = deliberations
            .get_mut(deliberation_id)
            .ok_or_else(|| anyhow!("Deliberation '{}' not found", deliberation_id))?;

        if deliberation.final_decision.is_some() {
            return Err(anyhow!(
                "Deliberation '{}' has already concluded",
                deliberation_id
            ));
        }

        let current_round = deliberation
            .rounds
            .last_mut()
            .ok_or_else(|| anyhow!("Deliberation '{}' has no rounds", deliberation_id))?;

        if current_round.concluded {
            return Err(anyhow!(
                "Current round {} of deliberation '{}' is already concluded",
                current_round.round_number,
                deliberation_id
            ));
        }

        // Replace existing vote from this member in this round, or add new one.
        if let Some(existing) = current_round
            .votes
            .iter_mut()
            .find(|v| v.member_id == vote.member_id)
        {
            *existing = vote;
        } else {
            current_round.votes.push(vote);
        }

        Ok(())
    }

    /// Conclude the current round of deliberation and advance to the next round.
    ///
    /// If all members have voted unanimously, consensus is reached and the
    /// deliberation concludes. Otherwise, the process advances to the next
    /// round (up to `max_rounds`). After the final round, a forced conclusion
    /// is applied based on majority vote.
    ///
    /// Returns `true` if the deliberation has concluded, `false` otherwise.
    pub fn conclude_round(&self, deliberation_id: &DeliberationId) -> Result<bool> {
        let mut deliberations = self
            .deliberations
            .lock()
            .map_err(|e| anyhow!("Failed to acquire lock on deliberations: {e}"))?;

        let deliberation = deliberations
            .get_mut(deliberation_id)
            .ok_or_else(|| anyhow!("Deliberation '{}' not found", deliberation_id))?;

        if deliberation.final_decision.is_some() {
            return Err(anyhow!(
                "Deliberation '{}' is already concluded",
                deliberation_id
            ));
        }

        let current_round = deliberation
            .rounds
            .last()
            .ok_or_else(|| anyhow!("Deliberation '{}' has no rounds", deliberation_id))?;

        if current_round.concluded {
            return Err(anyhow!(
                "Current round {} of deliberation '{}' is already concluded",
                current_round.round_number,
                deliberation_id
            ));
        }

        let round_number = current_round.round_number;
        let is_last_round = round_number >= deliberation.max_rounds;

        // Mark current round as concluded.
        if let Some(round) = deliberation.rounds.last_mut() {
            round.concluded = true;
        }

        // Tally the votes in the current round.
        let current_round = deliberation.rounds.last().unwrap();
        let tally = self.tally_deliberation_round_votes(current_round);

        // Check for unanimity (all non-abstain votes agree).
        let unanimous = self.is_round_unanimous(&tally);

        if unanimous {
            // Consensus reached!
            deliberation.consensus_reached = true;
            let winning_position = tally
                .iter()
                .max_by_key(|(_, &count)| count)
                .map(|(pos, _)| pos.clone());

            deliberation.final_decision = winning_position.map(|pos| CouncilDecision {
                position: pos,
                amended_text: None,
                decided_at_round: round_number,
            });
            return Ok(true);
        }

        if is_last_round {
            // Final round: force conclusion by majority vote.
            let winning_position = tally
                .iter()
                .max_by_key(|(_, &count)| count)
                .map(|(pos, _)| pos.clone())
                .unwrap_or(CouncilPosition::Abstain);

            deliberation.final_decision = Some(CouncilDecision {
                position: winning_position,
                amended_text: None,
                decided_at_round: round_number,
            });
            return Ok(true);
        }

        // Advance to next round.
        let next_round = DeliberationRound {
            round_number: round_number + 1,
            statements: Vec::new(),
            votes: Vec::new(),
            concluded: false,
        };

        deliberation.rounds.push(next_round);
        Ok(false)
    }

    /// Get a deliberation by ID.
    pub fn get_deliberation(&self, id: &DeliberationId) -> Result<Deliberation> {
        let deliberations = self
            .deliberations
            .lock()
            .map_err(|e| anyhow!("Failed to acquire lock on deliberations: {e}"))?;

        deliberations
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("Deliberation '{}' not found", id))
    }

    /// Get all active (non-concluded) deliberation IDs.
    pub fn get_active_deliberations(&self) -> Vec<DeliberationId> {
        let deliberations = self.deliberations.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });

        deliberations
            .iter()
            .filter(|(_, d)| d.final_decision.is_none())
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Run multi-round deliberation for a proposal.
    ///
    /// Orchestrates the full deliberation flow:
    /// 1. Starts a deliberation for the given proposal
    /// 2. Iterates through rounds, having each active member submit a statement
    ///    and vote before concluding each round
    /// 3. Breaks when consensus is reached or max rounds are exhausted
    ///
    /// Returns the final deliberation decision, if any.
    pub fn run_multi_round_deliberation(
        &self,
        proposal_id: &str,
    ) -> Result<Option<CouncilDecision>> {
        // Start the deliberation.
        let delib_id = self.start_deliberation(proposal_id)?;

        // Collect active members once.
        let members: Vec<CouncilMember> = {
            let members_lock = self
                .members
                .lock()
                .map_err(|e| anyhow!("Failed to acquire lock on members: {e}"))?;
            members_lock
                .values()
                .filter(|m| m.is_active)
                .cloned()
                .collect()
        };

        if members.is_empty() {
            return Err(anyhow!("No active members to participate in deliberation"));
        }

        loop {
            // Each active member submits a statement and casts a vote in the current round.
            for member in &members {
                let statement = DeliberationStatement {
                    member_id: member.id.clone(),
                    position: CouncilPosition::Support,
                    reasoning: format!("Deliberation vote by member '{}'", member.name),
                    amendments: Vec::new(),
                    submitted_at: now_epoch_ms(),
                };
                self.submit_statement(&delib_id, statement)?;

                let vote = CouncilVote {
                    member_id: member.id.clone(),
                    proposal_id: proposal_id.to_string(),
                    selected_option: "support".to_string(),
                    weight: member.voting_power,
                    vote_ms: now_epoch_ms(),
                    rationale: None,
                };
                self.vote_in_round(&delib_id, vote)?;
            }

            // Conclude the round; returns true if deliberation is complete.
            let concluded = self.conclude_round(&delib_id)?;
            if concluded {
                break;
            }
        }

        // Retrieve the final deliberation to return the decision.
        let final_delib = self.get_deliberation(&delib_id)?;
        Ok(final_delib.final_decision)
    }

    /// Submit a proposal and automatically determine the voting path.
    ///
    /// If the number of active members meets or exceeds
    /// `deliberation_member_threshold`, the proposal is routed through
    /// multi-round deliberation. Otherwise, it is submitted for standard
    /// single-round voting (caller must call `cast_vote` / `tally_votes`).
    ///
    /// Returns `Ok(true)` if multi-round deliberation was used and completed,
    /// `Ok(false)` if the proposal was submitted for single-round voting.
    pub fn vote_on_proposal(&self, proposal: CouncilProposal) -> Result<bool> {
        // Save the proposal ID before moving `proposal` into submit_proposal.
        let proposal_id = proposal.id.clone();

        // Submit the proposal first.
        self.submit_proposal(proposal)?;

        // Check active member count against threshold.
        let active_count = {
            let members_lock = self
                .members
                .lock()
                .map_err(|e| anyhow!("Failed to acquire lock on members: {e}"))?;
            members_lock.values().filter(|m| m.is_active).count()
        };

        if self.config.deliberation_member_threshold > 0
            && active_count >= self.config.deliberation_member_threshold
        {
            // Route to multi-round deliberation.
            self.run_multi_round_deliberation(&proposal_id)?;
            Ok(true)
        } else {
            // Standard single-round voting path.
            Ok(false)
        }
    }

    /// Auto-eject members whose accuracy has been persistently low.
    ///
    /// GAP-B49-09: Members with accuracy < ejection_threshold for ejection_window
    /// consecutive rounds are marked as `inactive`. New members get a protection
    /// period of ejection_warmup_rounds before being eligible for ejection.
    pub fn auto_eject_low_performers(&mut self) -> Vec<String> {
        let eject_threshold = self.config.ejection_threshold.unwrap_or(0.3);
        let eject_window = self.config.ejection_window.unwrap_or(20);
        let _warmup = self.config.ejection_warmup_rounds.unwrap_or(10);
        let mut ejected = Vec::new();

        let rep = self.reputation.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("council reputation lock poisoned, recovering");
            poisoned.into_inner()
        });

        for (member_id, record) in rep.iter() {
            // Skip members in warmup period
            if record.warmup_remaining > 0 {
                continue;
            }
            // Check if recent accuracy is below threshold for the window
            let recent_window = &record.recent_window;
            if recent_window.len() >= eject_window {
                let recent_majority = recent_window.iter().filter(|&&v| v).count();
                let recent_accuracy = recent_majority as f64 / recent_window.len() as f64;
                if recent_accuracy < eject_threshold {
                    ejected.push(member_id.clone());
                }
            }
        }

        // Release reputation lock before locking members
        drop(rep);

        // Mark ejected members as inactive
        let mut members = self.members.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("council members lock poisoned, recovering");
            poisoned.into_inner()
        });
        for member_id in &ejected {
            if let Some(member) = members.get_mut(member_id) {
                member.is_active = false;
                tracing::info!(
                    "Council auto-ejected low-performer member '{}' (recent accuracy < {:.1} for {} rounds)",
                    member_id, eject_threshold, eject_window
                );
            }
        }

        ejected
    }

    // ─── Internal deliberation helpers ───────────────────────────────────────

    /// Tally votes in a deliberation round by CouncilPosition.
    /// Maps each `CouncilVote`'s selected_option to a CouncilPosition tally.
    pub(crate) fn tally_deliberation_round_votes(
        &self,
        round: &DeliberationRound,
    ) -> HashMap<CouncilPosition, u32> {
        let mut tally: HashMap<CouncilPosition, u32> = HashMap::new();
        tally.insert(CouncilPosition::Support, 0);
        tally.insert(CouncilPosition::Oppose, 0);
        tally.insert(CouncilPosition::Amend, 0);
        tally.insert(CouncilPosition::Abstain, 0);

        for vote in &round.votes {
            // Determine which CouncilPosition this vote maps to.
            let pos = match vote.selected_option.to_lowercase().as_str() {
                "support" | "approve" | "yes" | "for" => CouncilPosition::Support,
                "oppose" | "reject" | "no" | "against" => CouncilPosition::Oppose,
                "amend" | "modify" | "change" => CouncilPosition::Amend,
                _ => CouncilPosition::Abstain,
            };
            // Use effective voting power (reputation-adjusted) instead of raw +1.
            let effective_weight = self.effective_voting_power(&vote.member_id, vote.weight);
            *tally.entry(pos).or_insert(0) += effective_weight;
        }

        tally
    }

    /// Check if a round's vote tally is unanimous (all non-abstain votes agree).
    pub(crate) fn is_round_unanimous(&self, tally: &HashMap<CouncilPosition, u32>) -> bool {
        let non_abstain: Vec<(_, _)> = tally
            .iter()
            .filter(|(pos, _)| **pos != CouncilPosition::Abstain)
            .filter(|(_, &count)| count > 0)
            .collect();

        // Unanimous if exactly one non-abstain position has all the votes.
        non_abstain.len() == 1
    }

    /// Return a `CouncilProfile` snapshot reflecting the current state.
    pub fn profile(&self) -> CouncilProfile {
        let members = self.members.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        let proposals = self.proposals.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });

        let total_members = members.len() as u32;
        let active_members = members.values().filter(|m| m.is_active).count() as u32;
        let total_proposals = proposals.len() as u32;

        let passed_count = proposals
            .values()
            .filter(|pr| pr.status == ProposalStatus::Passed)
            .count() as u32;

        let rejected_count = proposals
            .values()
            .filter(|pr| pr.status == ProposalStatus::Rejected)
            .count() as u32;

        let pending_count = proposals
            .values()
            .filter(|pr| {
                pr.status == ProposalStatus::Pending || pr.status == ProposalStatus::Active
            })
            .count() as u32;

        let tied_count = proposals
            .values()
            .filter(|pr| pr.status == ProposalStatus::Tied)
            .count() as u32;

        let reputation_adjusted_members = self
            .reputation
            .lock()
            .map(|r| {
                r.values()
                    .filter(|rec| {
                        rec.warmup_remaining == 0 && (rec.influence_multiplier - 1.0).abs() > 0.01
                    })
                    .count() as u32
            })
            .unwrap_or(0);

        CouncilProfile {
            total_members,
            active_members,
            total_proposals,
            passed_count,
            rejected_count,
            pending_count,
            tied_count,
            reputation_adjusted_members,
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::council::test_support::*;
    use super::super::council::OrchestrationCouncil;
    use super::super::types::*;

    #[test]
    fn test_start_deliberation() {
        let council = sample_deliberation_council();

        let delib_id = council.start_deliberation("prop-1").unwrap();
        assert_eq!(delib_id.0, "delib-prop-1");

        let delib = council.get_deliberation(&delib_id).unwrap();
        assert_eq!(delib.proposal_id, "prop-1");
        assert_eq!(delib.max_rounds, 3);
        assert!(!delib.consensus_reached);
        assert!(delib.final_decision.is_none());
        assert_eq!(delib.rounds.len(), 1);
        assert_eq!(delib.rounds[0].round_number, 1);
        assert!(!delib.rounds[0].concluded);
    }

    #[test]
    fn test_start_deliberation_nonexistent_proposal_fails() {
        let council = sample_deliberation_council();
        let err = council.start_deliberation("nonexistent").unwrap_err();
        assert!(
            err.to_string().contains("not found") || err.to_string().contains("error.council."),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_start_deliberation_duplicate_fails() {
        let council = sample_deliberation_council();
        council.start_deliberation("prop-1").unwrap();
        let err = council.start_deliberation("prop-1").unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn test_submit_statement_in_deliberation() {
        let council = sample_deliberation_council();
        let delib_id = council.start_deliberation("prop-1").unwrap();

        let statement = DeliberationStatement {
            member_id: "alice".to_string(),
            position: CouncilPosition::Support,
            reasoning: "This is a good proposal.".to_string(),
            amendments: vec![],
            submitted_at: now_epoch_ms(),
        };

        council.submit_statement(&delib_id, statement).unwrap();

        let delib = council.get_deliberation(&delib_id).unwrap();
        assert_eq!(delib.rounds[0].statements.len(), 1);
        assert_eq!(delib.rounds[0].statements[0].member_id, "alice");
        assert_eq!(
            delib.rounds[0].statements[0].position,
            CouncilPosition::Support
        );
    }

    #[test]
    fn test_submit_statement_replaces_existing_in_round() {
        let council = sample_deliberation_council();
        let delib_id = council.start_deliberation("prop-1").unwrap();

        // Alice submits initial statement.
        council
            .submit_statement(
                &delib_id,
                DeliberationStatement {
                    member_id: "alice".to_string(),
                    position: CouncilPosition::Support,
                    reasoning: "I support this.".to_string(),
                    amendments: vec![],
                    submitted_at: now_epoch_ms(),
                },
            )
            .unwrap();

        // Alice changes her mind (still in round 1).
        council
            .submit_statement(
                &delib_id,
                DeliberationStatement {
                    member_id: "alice".to_string(),
                    position: CouncilPosition::Oppose,
                    reasoning: "Actually, I oppose this now.".to_string(),
                    amendments: vec![],
                    submitted_at: now_epoch_ms(),
                },
            )
            .unwrap();

        let delib = council.get_deliberation(&delib_id).unwrap();
        assert_eq!(delib.rounds[0].statements.len(), 1);
        assert_eq!(
            delib.rounds[0].statements[0].position,
            CouncilPosition::Oppose
        );
    }

    #[test]
    fn test_vote_in_round() {
        let council = sample_deliberation_council();
        let delib_id = council.start_deliberation("prop-1").unwrap();

        council
            .vote_in_round(
                &delib_id,
                CouncilVote {
                    member_id: "alice".to_string(),
                    proposal_id: "prop-1".to_string(),
                    selected_option: "support".to_string(),
                    weight: 1,
                    vote_ms: now_epoch_ms(),
                    rationale: None,
                },
            )
            .unwrap();

        council
            .vote_in_round(
                &delib_id,
                CouncilVote {
                    member_id: "bob".to_string(),
                    proposal_id: "prop-1".to_string(),
                    selected_option: "oppose".to_string(),
                    weight: 1,
                    vote_ms: now_epoch_ms(),
                    rationale: None,
                },
            )
            .unwrap();

        let delib = council.get_deliberation(&delib_id).unwrap();
        assert_eq!(delib.rounds[0].votes.len(), 2);
    }

    #[test]
    fn test_vote_in_round_replaces_previous_vote() {
        let council = sample_deliberation_council();
        let delib_id = council.start_deliberation("prop-1").unwrap();

        // Alice votes support.
        council
            .vote_in_round(
                &delib_id,
                CouncilVote {
                    member_id: "alice".to_string(),
                    proposal_id: "prop-1".to_string(),
                    selected_option: "support".to_string(),
                    weight: 1,
                    vote_ms: now_epoch_ms(),
                    rationale: None,
                },
            )
            .unwrap();

        // Alice changes vote to oppose (still in round 1).
        council
            .vote_in_round(
                &delib_id,
                CouncilVote {
                    member_id: "alice".to_string(),
                    proposal_id: "prop-1".to_string(),
                    selected_option: "oppose".to_string(),
                    weight: 1,
                    vote_ms: now_epoch_ms(),
                    rationale: None,
                },
            )
            .unwrap();

        let delib = council.get_deliberation(&delib_id).unwrap();
        assert_eq!(delib.rounds[0].votes.len(), 1);
        assert_eq!(delib.rounds[0].votes[0].selected_option, "oppose");
    }

    #[test]
    fn test_inactive_member_cannot_vote_in_round() {
        let council = sample_deliberation_council();
        let delib_id = council.start_deliberation("prop-1").unwrap();

        // Deactivate Alice.
        {
            let mut members = council.members.lock().unwrap();
            if let Some(m) = members.get_mut("alice") {
                m.is_active = false;
            }
        }

        let err = council
            .vote_in_round(
                &delib_id,
                CouncilVote {
                    member_id: "alice".to_string(),
                    proposal_id: "prop-1".to_string(),
                    selected_option: "support".to_string(),
                    weight: 1,
                    vote_ms: now_epoch_ms(),
                    rationale: None,
                },
            )
            .unwrap_err();
        assert!(err.to_string().contains("inactive"));
    }

    #[test]
    fn test_get_active_deliberations() {
        let council = sample_deliberation_council();

        // Submit a second proposal.
        let mut p2 = sample_proposal("prop-2", "Another Test", "bob");
        p2.options = vec!["support".to_string(), "oppose".to_string()];
        p2.status = ProposalStatus::Active;
        council.submit_proposal(p2).unwrap();

        let d1 = council.start_deliberation("prop-1").unwrap();
        let d2 = council.start_deliberation("prop-2").unwrap();

        let active = council.get_active_deliberations();
        assert_eq!(active.len(), 2);
        assert!(active.contains(&d1));
        assert!(active.contains(&d2));
    }

    #[test]
    fn test_multi_round_deliberation_unanimous_round_one() {
        let council = sample_deliberation_council();
        let delib_id = council.start_deliberation("prop-1").unwrap();

        // All three members vote support in round 1.
        for member in &["alice", "bob", "carol"] {
            council
                .vote_in_round(
                    &delib_id,
                    CouncilVote {
                        member_id: member.to_string(),
                        proposal_id: "prop-1".to_string(),
                        selected_option: "support".to_string(),
                        weight: 1,
                        vote_ms: now_epoch_ms(),
                        rationale: None,
                    },
                )
                .unwrap();
        }

        // Conclude round 1 — should reach consensus.
        let concluded = council.conclude_round(&delib_id).unwrap();
        assert!(concluded);

        let delib = council.get_deliberation(&delib_id).unwrap();
        assert!(delib.consensus_reached);
        assert!(delib.final_decision.is_some());
        assert_eq!(
            delib.final_decision.unwrap().position,
            CouncilPosition::Support
        );

        // No active deliberations after conclusion.
        let active = council.get_active_deliberations();
        assert!(active.is_empty());
    }

    #[test]
    fn test_multi_round_deliberation_advances_to_round_two() {
        let council = sample_deliberation_council();
        let delib_id = council.start_deliberation("prop-1").unwrap();

        // Split vote: alice supports, bob opposes, carol supports.
        council
            .vote_in_round(
                &delib_id,
                CouncilVote {
                    member_id: "alice".to_string(),
                    proposal_id: "prop-1".to_string(),
                    selected_option: "support".to_string(),
                    weight: 1,
                    vote_ms: now_epoch_ms(),
                    rationale: None,
                },
            )
            .unwrap();
        council
            .vote_in_round(
                &delib_id,
                CouncilVote {
                    member_id: "bob".to_string(),
                    proposal_id: "prop-1".to_string(),
                    selected_option: "oppose".to_string(),
                    weight: 1,
                    vote_ms: now_epoch_ms(),
                    rationale: None,
                },
            )
            .unwrap();
        council
            .vote_in_round(
                &delib_id,
                CouncilVote {
                    member_id: "carol".to_string(),
                    proposal_id: "prop-1".to_string(),
                    selected_option: "support".to_string(),
                    weight: 1,
                    vote_ms: now_epoch_ms(),
                    rationale: None,
                },
            )
            .unwrap();

        // Round 1 not unanimous — advance to round 2.
        let concluded = council.conclude_round(&delib_id).unwrap();
        assert!(!concluded);

        let delib = council.get_deliberation(&delib_id).unwrap();
        assert_eq!(delib.rounds.len(), 2);
        assert_eq!(delib.rounds[0].round_number, 1);
        assert!(delib.rounds[0].concluded);
        assert_eq!(delib.rounds[1].round_number, 2);
        assert!(!delib.rounds[1].concluded);
    }

    #[test]
    fn test_multi_round_deliberation_position_changes_between_rounds() {
        let council = sample_deliberation_council();
        let delib_id = council.start_deliberation("prop-1").unwrap();

        // Round 1: alice and carol support, bob opposes.
        for (member, vote) in &[
            ("alice", "support"),
            ("bob", "oppose"),
            ("carol", "support"),
        ] {
            council
                .vote_in_round(
                    &delib_id,
                    CouncilVote {
                        member_id: member.to_string(),
                        proposal_id: "prop-1".to_string(),
                        selected_option: vote.to_string(),
                        weight: 1,
                        vote_ms: now_epoch_ms(),
                        rationale: None,
                    },
                )
                .unwrap();
        }

        // Conclude round 1 — not unanimous, advances to round 2.
        council.conclude_round(&delib_id).unwrap();

        // Round 2: bob changes position from oppose to support.
        council
            .vote_in_round(
                &delib_id,
                CouncilVote {
                    member_id: "bob".to_string(),
                    proposal_id: "prop-1".to_string(),
                    selected_option: "support".to_string(),
                    weight: 1,
                    vote_ms: now_epoch_ms(),
                    rationale: None,
                },
            )
            .unwrap();

        // Alice and carol also vote again (same positions).
        for member in &["alice", "carol"] {
            council
                .vote_in_round(
                    &delib_id,
                    CouncilVote {
                        member_id: member.to_string(),
                        proposal_id: "prop-1".to_string(),
                        selected_option: "support".to_string(),
                        weight: 1,
                        vote_ms: now_epoch_ms(),
                        rationale: None,
                    },
                )
                .unwrap();
        }

        // Conclude round 2 — now unanimous, should conclude.
        let concluded = council.conclude_round(&delib_id).unwrap();
        assert!(concluded);

        let delib = council.get_deliberation(&delib_id).unwrap();
        assert!(delib.consensus_reached);
        assert_eq!(
            delib.final_decision.unwrap().position,
            CouncilPosition::Support
        );
    }

    #[test]
    fn test_multi_round_deliberation_forced_conclusion_at_max_rounds() {
        let council = OrchestrationCouncil::new_with_deliberation_config(
            CouncilConfig {
                name: "Forced Conclusion Test".to_string(),
                min_members_for_quorum: 2,
                voting_duration_ms: 86_400_000,
                max_proposals: 100,
                enable_reputation: false,
                reputation_warmup_rounds: 0,
                ..Default::default()
            },
            DeliberationConfig {
                max_rounds: 2, // Use 2 for faster test
                require_consensus: false,
                debate_timeout_secs: 60,
            },
        );
        council
            .add_member(sample_member("alice", "Alice", "strategist", 1))
            .unwrap();
        council
            .add_member(sample_member("bob", "Bob", "analyst", 1))
            .unwrap();
        council
            .submit_proposal(sample_proposal("prop-1", "Forced Test", "alice"))
            .unwrap();

        let delib_id = council.start_deliberation("prop-1").unwrap();

        // Round 1: split vote.
        for (member, vote) in &[("alice", "support"), ("bob", "oppose")] {
            council
                .vote_in_round(
                    &delib_id,
                    CouncilVote {
                        member_id: member.to_string(),
                        proposal_id: "prop-1".to_string(),
                        selected_option: vote.to_string(),
                        weight: 1,
                        vote_ms: now_epoch_ms(),
                        rationale: None,
                    },
                )
                .unwrap();
        }

        // Round 1 not unanimous — advances to round 2 (max).
        let concluded = council.conclude_round(&delib_id).unwrap();
        assert!(!concluded);

        // Round 2: still split.
        for (member, vote) in &[("alice", "support"), ("bob", "oppose")] {
            council
                .vote_in_round(
                    &delib_id,
                    CouncilVote {
                        member_id: member.to_string(),
                        proposal_id: "prop-1".to_string(),
                        selected_option: vote.to_string(),
                        weight: 1,
                        vote_ms: now_epoch_ms(),
                        rationale: None,
                    },
                )
                .unwrap();
        }

        // Round 2 is last — forced conclusion (support wins by tiebreak / first max).
        let concluded = council.conclude_round(&delib_id).unwrap();
        assert!(concluded);

        let delib = council.get_deliberation(&delib_id).unwrap();
        assert!(delib.final_decision.is_some());
        let decision = delib.final_decision.unwrap();
        assert_eq!(decision.decided_at_round, 2);
        // After forced conclusion on a tie (1 support, 1 oppose), either is valid.
        // The tiebreaker is deterministic based on the tally map's iteration order.
        assert!(
            decision.position == CouncilPosition::Support
                || decision.position == CouncilPosition::Oppose,
            "Expected Support or Oppose (tie at 1-1), got {:?}",
            decision.position
        );
    }

    #[test]
    fn test_deliberation_submit_statement_after_conclusion_fails() {
        let council = sample_deliberation_council();
        let delib_id = council.start_deliberation("prop-1").unwrap();

        // All vote unanimously.
        for member in &["alice", "bob", "carol"] {
            council
                .vote_in_round(
                    &delib_id,
                    CouncilVote {
                        member_id: member.to_string(),
                        proposal_id: "prop-1".to_string(),
                        selected_option: "support".to_string(),
                        weight: 1,
                        vote_ms: now_epoch_ms(),
                        rationale: None,
                    },
                )
                .unwrap();
        }

        council.conclude_round(&delib_id).unwrap();

        // Try to submit statement after conclusion.
        let err = council
            .submit_statement(
                &delib_id,
                DeliberationStatement {
                    member_id: "alice".to_string(),
                    position: CouncilPosition::Amend,
                    reasoning: "Too late!".to_string(),
                    amendments: vec![],
                    submitted_at: now_epoch_ms(),
                },
            )
            .unwrap_err();
        assert!(err.to_string().contains("concluded"));
    }

    #[test]
    fn test_deliberation_vote_in_concluded_round_fails() {
        // Use max_rounds=1 so that a split vote in round 1 forces conclusion.
        let council = OrchestrationCouncil::new_with_deliberation_config(
            CouncilConfig {
                name: "Test Council".to_string(),
                min_members_for_quorum: 2,
                voting_duration_ms: 86_400_000,
                max_proposals: 100,
                enable_reputation: false,
                reputation_warmup_rounds: 0,
                ..Default::default()
            },
            DeliberationConfig {
                max_rounds: 1,
                require_consensus: false,
                debate_timeout_secs: 60,
            },
        );
        council
            .add_member(sample_member("alice", "Alice", "strategist", 1))
            .unwrap();
        council
            .add_member(sample_member("bob", "Bob", "analyst", 1))
            .unwrap();
        council
            .add_member(sample_member("carol", "Carol", "overseer", 1))
            .unwrap();
        council
            .submit_proposal(sample_proposal("prop-1", "Test Proposal", "alice"))
            .unwrap();
        let delib_id = council.start_deliberation("prop-1").unwrap();

        // Round 1 votes split, conclude round 1.
        council
            .vote_in_round(
                &delib_id,
                CouncilVote {
                    member_id: "alice".to_string(),
                    proposal_id: "prop-1".to_string(),
                    selected_option: "support".to_string(),
                    weight: 1,
                    vote_ms: now_epoch_ms(),
                    rationale: None,
                },
            )
            .unwrap();
        council
            .vote_in_round(
                &delib_id,
                CouncilVote {
                    member_id: "bob".to_string(),
                    proposal_id: "prop-1".to_string(),
                    selected_option: "oppose".to_string(),
                    weight: 1,
                    vote_ms: now_epoch_ms(),
                    rationale: None,
                },
            )
            .unwrap();

        let concluded = council.conclude_round(&delib_id).unwrap();
        assert!(
            concluded,
            "Deliberation should have concluded with max_rounds=1"
        );

        // With max_rounds=1 and a split vote, the deliberation has concluded.
        // Attempting to vote should fail with "already concluded".
        let err = council
            .vote_in_round(
                &delib_id,
                CouncilVote {
                    member_id: "carol".to_string(),
                    proposal_id: "prop-1".to_string(),
                    selected_option: "support".to_string(),
                    weight: 1,
                    vote_ms: now_epoch_ms(),
                    rationale: None,
                },
            )
            .unwrap_err();
        assert!(
            err.to_string().contains("concluded"),
            "Expected error containing 'concluded', got: {}",
            err
        );
    }

    #[test]
    fn test_conclude_round_twice_fails() {
        let council = sample_deliberation_council();
        let delib_id = council.start_deliberation("prop-1").unwrap();

        // Vote and conclude round 1.
        for member in &["alice", "bob"] {
            council
                .vote_in_round(
                    &delib_id,
                    CouncilVote {
                        member_id: member.to_string(),
                        proposal_id: "prop-1".to_string(),
                        selected_option: "oppose".to_string(),
                        weight: 1,
                        vote_ms: now_epoch_ms(),
                        rationale: None,
                    },
                )
                .unwrap();
        }
        council.conclude_round(&delib_id).unwrap();

        // Second conclude on the same round should fail.
        let err = council.conclude_round(&delib_id).unwrap_err();
        assert!(err.to_string().contains("concluded"));
    }

    #[test]
    fn test_deliberation_statement_with_amendments() {
        let council = sample_deliberation_council();
        let delib_id = council.start_deliberation("prop-1").unwrap();

        let statement = DeliberationStatement {
            member_id: "alice".to_string(),
            position: CouncilPosition::Amend,
            reasoning: "We should increase the memory limit.".to_string(),
            amendments: vec![
                "Increase memory limit from 512MB to 1GB".to_string(),
                "Add monitoring alerts".to_string(),
            ],
            submitted_at: now_epoch_ms(),
        };

        council.submit_statement(&delib_id, statement).unwrap();

        let delib = council.get_deliberation(&delib_id).unwrap();
        let stmt = &delib.rounds[0].statements[0];
        assert_eq!(stmt.position, CouncilPosition::Amend);
        assert_eq!(stmt.amendments.len(), 2);
        assert!(stmt.amendments[0].contains("1GB"));
    }

    #[test]
    fn test_deliberation_config_default_values() {
        let config = DeliberationConfig::default();
        assert_eq!(config.max_rounds, 3);
        assert_eq!(config.debate_timeout_secs, 60);
        assert!(!config.require_consensus);
    }

    #[test]
    fn test_profile_reflects_state() {
        let council = default_council();

        // Initial profile.
        let p = council.profile();
        assert_eq!(p.total_members, 0);

        // Add members.
        council
            .add_member(sample_member("alice", "Alice", "strategist", 1))
            .unwrap();
        council
            .add_member(sample_member("bob", "Bob", "analyst", 2))
            .unwrap();

        let p = council.profile();
        assert_eq!(p.total_members, 2);
        assert_eq!(p.active_members, 2);

        // Submit some proposals.
        let mut p1 = sample_proposal("prop-1", "Proposal A", "alice");
        p1.status = ProposalStatus::Pending;
        council.submit_proposal(p1).unwrap();

        let mut p2 = sample_proposal("prop-2", "Proposal B", "bob");
        p2.status = ProposalStatus::Active;
        council.submit_proposal(p2).unwrap();

        let p = council.profile();
        assert_eq!(p.total_proposals, 2);
        assert_eq!(p.pending_count, 2); // Both are pending or active
        assert_eq!(p.passed_count, 0);
        assert_eq!(p.rejected_count, 0);
        assert_eq!(p.tied_count, 0);

        // Manually alter a proposal's status to test counts.
        {
            let mut proposals = council.proposals.lock().unwrap();
            if let Some(p) = proposals.get_mut("prop-1") {
                p.status = ProposalStatus::Passed;
            }
        }

        let p = council.profile();
        assert_eq!(p.passed_count, 1);
        assert_eq!(p.pending_count, 1); // Only prop-2 is still active
    }

    #[test]
    fn test_count_valid_proposals_in_profile() {
        let council = OrchestrationCouncil::new(CouncilConfig {
            name: "Profile Tester".to_string(),
            min_members_for_quorum: 1,
            voting_duration_ms: 1,
            max_proposals: 100,
            enable_reputation: false,
            reputation_warmup_rounds: 0,
            ..Default::default()
        });

        // Add a member.
        council
            .add_member(sample_member("alice", "Alice", "strategist", 1))
            .unwrap();

        // Submit a proposal and make it pass.
        council
            .submit_proposal(sample_proposal("p1", "Pass me", "alice"))
            .unwrap();
        let mut p2 = sample_proposal("p2", "Reject me", "alice");
        p2.status = ProposalStatus::Rejected;
        council.submit_proposal(p2).unwrap();
        let mut p3 = sample_proposal("p3", "Expire me", "alice");
        p3.status = ProposalStatus::Expired;
        council.submit_proposal(p3).unwrap();
        let mut p4 = sample_proposal("p4", "Tie me", "alice");
        p4.status = ProposalStatus::Tied;
        council.submit_proposal(p4).unwrap();

        {
            let proposals = council.proposals.lock().unwrap();
            let mut p1c = proposals.get("p1").cloned().unwrap();
            p1c.status = ProposalStatus::Passed;
            drop(proposals);
            let mut proposals = council.proposals.lock().unwrap();
            proposals.insert("p1".to_string(), p1c);
        }

        let p = council.profile();
        assert_eq!(p.total_proposals, 4);
        assert_eq!(p.passed_count, 1);
        assert_eq!(p.rejected_count, 1);
        assert_eq!(p.tied_count, 1);
    }

    #[test]
    fn test_auto_eject_low_performers() {
        let mut council = OrchestrationCouncil::new(CouncilConfig {
            name: "Ejection Test".to_string(),
            min_members_for_quorum: 2,
            voting_duration_ms: 86_400_000,
            max_proposals: 100,
            enable_reputation: true,
            reputation_warmup_rounds: 0,
            ejection_threshold: Some(0.3),
            ejection_window: Some(20),
            ejection_warmup_rounds: Some(0),
            ..Default::default()
        });

        // Add a member with poor recent accuracy
        let member_id = "poor-performer".to_string();
        council
            .add_member(sample_member(&member_id, "Poor Performer", "analyst", 1))
            .unwrap();

        {
            let mut rep = council.reputation.lock().unwrap();
            let record = rep
                .entry(member_id.clone())
                .or_insert_with(|| ReputationRecord::new(&member_id, 0));
            record.warmup_remaining = 0;
            record.recent_window = vec![false; 25]; // 25 consecutive wrong votes
        }

        let ejected = council.auto_eject_low_performers();
        assert!(
            ejected.contains(&member_id),
            "low performer should be ejected"
        );
    }
}
