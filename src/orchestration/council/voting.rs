//! Voting-related methods for `OrchestrationCouncil`.
//!
//! Handles casting votes, tallying results, recording vote accuracy,
//! reputation tracking, and effective voting power calculation.

use super::council::OrchestrationCouncil;
use super::types::*;
use crate::i18n::runtime::tf;
use anyhow::{anyhow, Result};
use std::collections::HashMap;

impl OrchestrationCouncil {
    /// Initialize reputation tracking for a member (called automatically on first vote).
    #[allow(dead_code)] // F-GAP-15 — kept for external API consistency; used indirectly via record_vote_accuracy
    pub(crate) fn ensure_reputation(&self, member_id: &str) {
        let mut rep = self.reputation.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        if !rep.contains_key(member_id) {
            rep.insert(
                member_id.to_string(),
                ReputationRecord::new(member_id, self.config.reputation_warmup_rounds),
            );
        }
    }

    /// Get the effective voting power for a member, accounting for reputation.
    /// When reputation is disabled or member is in warmup, returns nominal voting_power.
    pub(crate) fn effective_voting_power(&self, member_id: &str, nominal_power: u32) -> u32 {
        if !self.config.enable_reputation {
            return nominal_power;
        }
        let rep = self.reputation.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        if let Some(record) = rep.get(member_id) {
            if record.warmup_remaining > 0 {
                return nominal_power;
            }
            let adjusted = (nominal_power as f64 * record.influence_multiplier).round() as u32;
            return adjusted.max(1); // Minimum voting power of 1
        }
        nominal_power
    }

    /// Record the accuracy of a member's vote after the final outcome is known.
    /// Call this after `tally_votes()` to enable the council to learn.
    pub fn record_vote_accuracy(
        &self,
        proposal_id: &str,
        winning_option: &Option<String>,
    ) -> Result<()> {
        let votes = self
            .votes
            .lock()
            .map_err(|e| anyhow!("Failed to acquire lock on votes: {e}"))?;
        let mut reputation = self
            .reputation
            .lock()
            .map_err(|e| anyhow!("Failed to acquire lock on reputation: {e}"))?;

        for vote in votes.values().filter(|v| v.proposal_id == proposal_id) {
            // Initialize reputation if not present (called without holding reputation lock, which
            // is already held in the outer scope, so we access the map directly)
            if !reputation.contains_key(&vote.member_id) {
                reputation.insert(
                    vote.member_id.to_string(),
                    ReputationRecord::new(&vote.member_id, self.config.reputation_warmup_rounds),
                );
            }
            if let Some(record) = reputation.get_mut(&vote.member_id) {
                let was_accurate = match winning_option {
                    Some(winner) => vote.selected_option == *winner,
                    None => false, // No winner (e.g. tie), no one was accurate
                };
                record.record_outcome(was_accurate);
            }
        }
        Ok(())
    }

    /// Get the reputation record for a member, if available.
    pub fn get_reputation(&self, member_id: &str) -> Option<ReputationRecord> {
        let guard = self.reputation.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        guard.get(member_id).cloned()
    }

    /// Cast a vote on a proposal.
    ///
    /// Returns an error if:
    /// - The member does not exist or is inactive.
    /// - The proposal does not exist or is not in `Active` status.
    /// - The selected option is not a valid option for the proposal.
    /// - The member has already voted on this proposal.
    pub fn cast_vote(&self, vote: CouncilVote) -> Result<()> {
        // Validate member exists and is active.
        {
            let members = self
                .members
                .lock()
                .map_err(|e| anyhow!("Failed to acquire lock on members: {}", e))?;

            let member = members
                .get(&vote.member_id)
                .ok_or_else(|| anyhow!("Member '{}' not found", vote.member_id))?;

            if !member.is_active {
                return Err(anyhow!(tf(
                    "error.council.member_inactive",
                    &[("member_id", &vote.member_id)]
                )));
            }
        }

        // Validate proposal exists and is active.
        {
            let proposals = self
                .proposals
                .lock()
                .map_err(|e| anyhow!("Failed to acquire lock on proposals: {}", e))?;

            let proposal = proposals.get(&vote.proposal_id).ok_or_else(|| {
                anyhow!(tf(
                    "error.council.proposal_not_found",
                    &[("proposal_id", &vote.proposal_id)]
                ))
            })?;

            if proposal.status != ProposalStatus::Active {
                return Err(anyhow!(tf(
                    "error.council.proposal_not_active",
                    &[
                        ("proposal_id", &vote.proposal_id),
                        ("status", &format!("{:?}", proposal.status))
                    ]
                )));
            }

            if !proposal.options.contains(&vote.selected_option) {
                return Err(anyhow!(tf(
                    "error.council.invalid_option",
                    &[
                        ("option", &vote.selected_option),
                        ("proposal_id", &vote.proposal_id),
                        ("valid_options", &format!("{:?}", proposal.options))
                    ]
                )));
            }
        }

        // Record the vote (prevent duplicate).
        let mut votes = self
            .votes
            .lock()
            .map_err(|e| anyhow!("Failed to acquire lock on votes: {}", e))?;

        let key = (vote.member_id.clone(), vote.proposal_id.clone());
        if votes.contains_key(&key) {
            return Err(anyhow!(tf(
                "error.council.duplicate_vote",
                &[
                    ("member_id", &vote.member_id),
                    ("proposal_id", &vote.proposal_id)
                ]
            )));
        }

        votes.insert(key, vote);
        Ok(())
    }

    /// Tally votes for a given proposal and determine the outcome.
    ///
    /// After tallying, the proposal's status is updated to `Passed`,
    /// `Rejected`, or `Tied`. If quorum is not met, the proposal is
    /// marked as `Rejected`.
    pub fn tally_votes(&self, proposal_id: &str) -> Result<VoteResult> {
        let mut proposals = self
            .proposals
            .lock()
            .map_err(|e| anyhow!("Failed to acquire lock on proposals: {}", e))?;

        let proposal = proposals.get(proposal_id).ok_or_else(|| {
            anyhow!(tf(
                "error.council.proposal_not_found",
                &[("proposal_id", proposal_id)]
            ))
        })?;

        if proposal.status != ProposalStatus::Active {
            return Err(anyhow!(tf(
                "error.council.proposal_not_active",
                &[
                    ("proposal_id", proposal_id),
                    ("status", &format!("{:?}", proposal.status))
                ]
            )));
        }

        let votes = self
            .votes
            .lock()
            .map_err(|e| anyhow!("Failed to acquire lock on votes: {}", e))?;

        // Gather votes for this proposal.
        let proposal_votes: Vec<&CouncilVote> = votes
            .values()
            .filter(|v| v.proposal_id == proposal_id)
            .collect();

        // Count active members for quorum check.
        let active_members = {
            let members = self
                .members
                .lock()
                .map_err(|e| anyhow!("Failed to acquire lock on members: {}", e))?;
            members.values().filter(|m| m.is_active).count() as u32
        };

        // Check quorum: at least min_members_for_quorum must vote.
        let quorum_met = proposal_votes.len() as u32 >= self.config.min_members_for_quorum
            && proposal_votes.len() as u32 <= active_members;

        if !quorum_met {
            // Mark as rejected when quorum is not met.
            if let Some(p) = proposals.get_mut(proposal_id) {
                p.status = ProposalStatus::Rejected;
            }

            return Ok(VoteResult {
                proposal_id: proposal_id.to_string(),
                option_tallies: HashMap::new(),
                total_votes: proposal_votes.len() as u32,
                passed: false,
                winning_option: None,
                tie: false,
            });
        }

        // Tally weighted votes per option, using reputation-adjusted voting power.
        let mut option_tallies: HashMap<String, u32> = HashMap::new();
        for option in &proposal.options {
            option_tallies.insert(option.clone(), 0);
        }

        let mut total_votes: u32 = 0;
        for vote in &proposal_votes {
            let effective_weight = self.effective_voting_power(&vote.member_id, vote.weight);
            let tally = option_tallies
                .entry(vote.selected_option.clone())
                .or_insert(0);
            *tally += effective_weight;
            total_votes += effective_weight;
        }

        // Determine the winner(s).
        let max_tally = option_tallies.values().max().copied().unwrap_or(0);
        let winners: Vec<&String> = option_tallies
            .iter()
            .filter(|(_, &count)| count == max_tally && count > 0)
            .map(|(option, _)| option)
            .collect();

        let tie = winners.len() > 1;
        let passed = !tie && !proposal.options.is_empty() && max_tally > 0;

        let winning_option = if !tie && passed {
            Some(winners[0].clone())
        } else {
            None
        };

        // Update proposal status.
        let new_status = if tie {
            ProposalStatus::Tied
        } else if passed {
            ProposalStatus::Passed
        } else {
            ProposalStatus::Rejected
        };

        if let Some(p) = proposals.get_mut(proposal_id) {
            p.status = new_status;
        }

        Ok(VoteResult {
            proposal_id: proposal_id.to_string(),
            option_tallies,
            total_votes,
            passed,
            winning_option,
            tie,
        })
    }
}
