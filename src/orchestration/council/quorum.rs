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
