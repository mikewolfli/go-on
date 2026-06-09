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

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::council::test_support::*;
    use super::super::council::OrchestrationCouncil;
    use super::super::types::*;

    #[test]
    fn test_cast_vote() {
        let council = default_council();
        council
            .add_member(sample_member("alice", "Alice", "strategist", 1))
            .unwrap();
        council
            .add_member(sample_member("bob", "Bob", "analyst", 2))
            .unwrap();
        council
            .submit_proposal(sample_proposal("prop-1", "Test proposal", "alice"))
            .unwrap();

        // Cast vote from Alice.
        council
            .cast_vote(CouncilVote {
                member_id: "alice".to_string(),
                proposal_id: "prop-1".to_string(),
                selected_option: "approve".to_string(),
                weight: 1,
                vote_ms: now_epoch_ms(),
                rationale: Some("Good idea".to_string()),
            })
            .unwrap();

        // Cast vote from Bob.
        council
            .cast_vote(CouncilVote {
                member_id: "bob".to_string(),
                proposal_id: "prop-1".to_string(),
                selected_option: "approve".to_string(),
                weight: 2,
                vote_ms: now_epoch_ms(),
                rationale: None,
            })
            .unwrap();

        // Duplicate vote should fail.
        let err = council
            .cast_vote(CouncilVote {
                member_id: "alice".to_string(),
                proposal_id: "prop-1".to_string(),
                selected_option: "reject".to_string(),
                weight: 1,
                vote_ms: now_epoch_ms(),
                rationale: None,
            })
            .unwrap_err();
        assert!(
            err.to_string().contains("already voted") || err.to_string().contains("error.council."),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_tally_votes_simple_majority() {
        let council = default_council();
        council
            .add_member(sample_member("alice", "Alice", "strategist", 1))
            .unwrap();
        council
            .add_member(sample_member("bob", "Bob", "analyst", 2))
            .unwrap();
        council
            .add_member(sample_member("carol", "Carol", "overseer", 1))
            .unwrap();
        council
            .submit_proposal(sample_proposal("prop-1", "Increase memory", "alice"))
            .unwrap();

        // Alice approves (weight 1), Bob approves (weight 2), Carol rejects (weight 1).
        council
            .cast_vote(CouncilVote {
                member_id: "alice".to_string(),
                proposal_id: "prop-1".to_string(),
                selected_option: "approve".to_string(),
                weight: 1,
                vote_ms: now_epoch_ms(),
                rationale: None,
            })
            .unwrap();
        council
            .cast_vote(CouncilVote {
                member_id: "bob".to_string(),
                proposal_id: "prop-1".to_string(),
                selected_option: "approve".to_string(),
                weight: 2,
                vote_ms: now_epoch_ms(),
                rationale: None,
            })
            .unwrap();
        council
            .cast_vote(CouncilVote {
                member_id: "carol".to_string(),
                proposal_id: "prop-1".to_string(),
                selected_option: "reject".to_string(),
                weight: 1,
                vote_ms: now_epoch_ms(),
                rationale: None,
            })
            .unwrap();

        let result = council.tally_votes("prop-1").unwrap();
        assert!(result.passed);
        assert_eq!(result.winning_option.as_deref(), Some("approve"));
        assert!(!result.tie);
        assert_eq!(result.total_votes, 4);
        assert_eq!(*result.option_tallies.get("approve").unwrap(), 3);
        assert_eq!(*result.option_tallies.get("reject").unwrap(), 1);

        let proposal = council.get_proposal("prop-1").unwrap();
        assert_eq!(proposal.status, ProposalStatus::Passed);
    }

    #[test]
    fn test_tally_votes_tie() {
        let council = default_council();
        council
            .add_member(sample_member("alice", "Alice", "strategist", 1))
            .unwrap();
        council
            .add_member(sample_member("bob", "Bob", "analyst", 1))
            .unwrap();
        council
            .submit_proposal(sample_proposal("prop-1", "Tie test", "alice"))
            .unwrap();

        // Alice approves (weight 1), Bob rejects (weight 1) = tie.
        council
            .cast_vote(CouncilVote {
                member_id: "alice".to_string(),
                proposal_id: "prop-1".to_string(),
                selected_option: "approve".to_string(),
                weight: 1,
                vote_ms: now_epoch_ms(),
                rationale: None,
            })
            .unwrap();
        council
            .cast_vote(CouncilVote {
                member_id: "bob".to_string(),
                proposal_id: "prop-1".to_string(),
                selected_option: "reject".to_string(),
                weight: 1,
                vote_ms: now_epoch_ms(),
                rationale: None,
            })
            .unwrap();

        let result = council.tally_votes("prop-1").unwrap();
        assert!(!result.passed);
        assert!(result.tie);
        assert!(result.winning_option.is_none());
        assert_eq!(result.total_votes, 2);

        let proposal = council.get_proposal("prop-1").unwrap();
        assert_eq!(proposal.status, ProposalStatus::Tied);
    }

    #[test]
    fn test_quorum_not_met() {
        let council = OrchestrationCouncil::new(CouncilConfig {
            name: "Test Council".to_string(),
            min_members_for_quorum: 5,
            voting_duration_ms: 86_400_000,
            max_proposals: 100,
            enable_reputation: false,
            reputation_warmup_rounds: 0,
            ..Default::default()
        });

        council
            .add_member(sample_member("alice", "Alice", "strategist", 1))
            .unwrap();
        council
            .add_member(sample_member("bob", "Bob", "analyst", 1))
            .unwrap();
        council
            .submit_proposal(sample_proposal("prop-1", "Quorum test", "alice"))
            .unwrap();

        council
            .cast_vote(CouncilVote {
                member_id: "alice".to_string(),
                proposal_id: "prop-1".to_string(),
                selected_option: "approve".to_string(),
                weight: 1,
                vote_ms: now_epoch_ms(),
                rationale: None,
            })
            .unwrap();
        council
            .cast_vote(CouncilVote {
                member_id: "bob".to_string(),
                proposal_id: "prop-1".to_string(),
                selected_option: "approve".to_string(),
                weight: 1,
                vote_ms: now_epoch_ms(),
                rationale: None,
            })
            .unwrap();

        let result = council.tally_votes("prop-1").unwrap();
        assert!(!result.passed);
        assert!(!result.tie);
        assert!(result.winning_option.is_none());
        // Even though all votes are "approve", quorum requires 5 members to vote.
        assert_eq!(result.total_votes, 2);

        let proposal = council.get_proposal("prop-1").unwrap();
        assert_eq!(proposal.status, ProposalStatus::Rejected);
    }

    #[test]
    fn test_inactive_member_cannot_vote() {
        let council = default_council();
        let mut member = sample_member("alice", "Alice", "strategist", 1);
        member.is_active = false;
        council.add_member(member).unwrap();
        council
            .submit_proposal(sample_proposal("prop-1", "Test", "alice"))
            .unwrap();

        let err = council
            .cast_vote(CouncilVote {
                member_id: "alice".to_string(),
                proposal_id: "prop-1".to_string(),
                selected_option: "approve".to_string(),
                weight: 1,
                vote_ms: now_epoch_ms(),
                rationale: None,
            })
            .unwrap_err();
        assert!(
            err.to_string().contains("inactive") || err.to_string().contains("error.council."),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_invalid_vote_option_rejected() {
        let council = default_council();
        council
            .add_member(sample_member("alice", "Alice", "strategist", 1))
            .unwrap();
        council
            .submit_proposal(sample_proposal("prop-1", "Test", "alice"))
            .unwrap();

        let err = council
            .cast_vote(CouncilVote {
                member_id: "alice".to_string(),
                proposal_id: "prop-1".to_string(),
                selected_option: "invalid_option".to_string(),
                weight: 1,
                vote_ms: now_epoch_ms(),
                rationale: None,
            })
            .unwrap_err();
        assert!(
            err.to_string().contains("not a valid option")
                || err.to_string().contains("error.council."),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_tally_non_active_proposal_fails() {
        let council = default_council();
        let mut proposal = sample_proposal("prop-1", "Test", "alice");
        proposal.status = ProposalStatus::Pending;
        council.submit_proposal(proposal).unwrap();

        let err = council.tally_votes("prop-1").unwrap_err();
        assert!(
            err.to_string().contains("not active") || err.to_string().contains("error.council."),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_cast_vote_nonexistent_member_fails() {
        let council = default_council();
        council
            .submit_proposal(sample_proposal("prop-1", "Test", "alice"))
            .unwrap();

        let err = council
            .cast_vote(CouncilVote {
                member_id: "ghost".to_string(),
                proposal_id: "prop-1".to_string(),
                selected_option: "approve".to_string(),
                weight: 1,
                vote_ms: now_epoch_ms(),
                rationale: None,
            })
            .unwrap_err();
        assert!(
            err.to_string().contains("not found") || err.to_string().contains("error.council."),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_cast_vote_nonexistent_proposal_fails() {
        let council = default_council();
        council
            .add_member(sample_member("alice", "Alice", "strategist", 1))
            .unwrap();

        let err = council
            .cast_vote(CouncilVote {
                member_id: "alice".to_string(),
                proposal_id: "no-such-proposal".to_string(),
                selected_option: "approve".to_string(),
                weight: 1,
                vote_ms: now_epoch_ms(),
                rationale: None,
            })
            .unwrap_err();
        assert!(
            err.to_string().contains("not found") || err.to_string().contains("error.council."),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_reputation_record_accuracy_updates() {
        let mut record = ReputationRecord::new("member-1", 0);
        assert_eq!(record.accuracy, 0.5);
        assert_eq!(record.influence_multiplier, 1.0);

        // 3 accurate votes in a row → accuracy increases
        record.record_outcome(true);
        let m1 = record.influence_multiplier;
        record.record_outcome(true);
        record.record_outcome(true);
        let m2 = record.influence_multiplier;
        assert!(m2 > m1, "accuracy should improve multiplier");
    }

    #[test]
    fn test_reputation_penalizes_inaccurate_voting() {
        let mut record = ReputationRecord::new("member-1", 0);
        let initial = record.influence_multiplier;

        // 3 inaccurate votes → accuracy drops
        record.record_outcome(false);
        record.record_outcome(false);
        record.record_outcome(false);
        assert!(
            record.influence_multiplier < initial,
            "inaccurate voting should reduce multiplier"
        );
        assert!(record.influence_multiplier >= 0.5); // Minimum floor
    }

    #[test]
    fn test_reputation_warmup_protects_new_members() {
        let mut record = ReputationRecord::new("new-member", 3);
        assert_eq!(record.warmup_remaining, 3);

        // During warmup, multiplier stays at 1.0 regardless of outcomes
        record.record_outcome(false);
        assert_eq!(record.warmup_remaining, 2);
        assert_eq!(record.influence_multiplier, 1.0);

        record.record_outcome(false);
        assert_eq!(record.warmup_remaining, 1);
        assert_eq!(record.influence_multiplier, 1.0);

        record.record_outcome(false);
        assert_eq!(record.warmup_remaining, 0);
        assert_eq!(record.influence_multiplier, 1.0); // Still 1.0 (warmup covered this call)

        // After warmup exhausts, reputation takes effect
        record.record_outcome(false);
        assert_eq!(record.warmup_remaining, 0);
        assert!(record.influence_multiplier < 1.0);
    }

    #[test]
    fn test_council_tally_with_reputation() {
        let council = default_council();
        council
            .add_member(sample_member("high-acc", "High Accuracy", "expert", 10))
            .unwrap();
        council
            .add_member(sample_member("low-acc", "Low Accuracy", "novice", 10))
            .unwrap();

        // Simulate past votes: high-acc is accurate, low-acc is inaccurate
        for _ in 0..5 {
            council.ensure_reputation("high-acc");
            let mut rep = council.reputation.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned, recovering");
                poisoned.into_inner()
            });
            if let Some(r) = rep.get_mut("high-acc") {
                r.record_outcome(true);
            }
            drop(rep);
            council.ensure_reputation("low-acc");
            let mut rep = council.reputation.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned, recovering");
                poisoned.into_inner()
            });
            if let Some(r) = rep.get_mut("low-acc") {
                r.record_outcome(false);
            }
            drop(rep);
        }

        // Submit proposal
        let proposal = sample_proposal("prop-1", "Test", "high-acc");
        council.submit_proposal(proposal).unwrap();

        // Both vote approve
        council
            .cast_vote(CouncilVote {
                member_id: "high-acc".to_string(),
                proposal_id: "prop-1".to_string(),
                selected_option: "approve".to_string(),
                weight: 10,
                vote_ms: now_epoch_ms(),
                rationale: None,
            })
            .unwrap();
        council
            .cast_vote(CouncilVote {
                member_id: "low-acc".to_string(),
                proposal_id: "prop-1".to_string(),
                selected_option: "approve".to_string(),
                weight: 10,
                vote_ms: now_epoch_ms(),
                rationale: None,
            })
            .unwrap();

        let result = council.tally_votes("prop-1").unwrap();
        assert!(result.passed);

        // High-accuracy member's vote should have more weight now
        let high_power = council.effective_voting_power("high-acc", 10);
        let low_power = council.effective_voting_power("low-acc", 10);
        assert!(
            high_power > low_power,
            "high-accuracy member should have more voting power, got {} vs {}",
            high_power,
            low_power
        );
    }

    #[test]
    fn test_record_vote_accuracy_updates_reputation() {
        let council = default_council();
        council
            .add_member(sample_member("voter-1", "Voter One", "analyst", 1))
            .unwrap();
        council
            .add_member(sample_member("voter-2", "Voter Two", "analyst", 1))
            .unwrap();

        let proposal = sample_proposal("prop-rep", "Rep Test", "voter-1");
        council.submit_proposal(proposal).unwrap();

        // Both vote approve
        council
            .cast_vote(CouncilVote {
                member_id: "voter-1".to_string(),
                proposal_id: "prop-rep".to_string(),
                selected_option: "approve".to_string(),
                weight: 1,
                vote_ms: now_epoch_ms(),
                rationale: None,
            })
            .unwrap();
        council
            .cast_vote(CouncilVote {
                member_id: "voter-2".to_string(),
                proposal_id: "prop-rep".to_string(),
                selected_option: "approve".to_string(),
                weight: 1,
                vote_ms: now_epoch_ms(),
                rationale: None,
            })
            .unwrap();

        let result = council.tally_votes("prop-rep").unwrap();
        assert!(result.passed);

        // Record accuracy - both should be marked accurate since they voted with the winner
        council
            .record_vote_accuracy("prop-rep", &result.winning_option)
            .unwrap();

        let rep1 = council.get_reputation("voter-1").unwrap();
        assert_eq!(rep1.accurate_votes, 1);
        assert_eq!(rep1.total_votes, 1);

        let rep2 = council.get_reputation("voter-2").unwrap();
        assert_eq!(rep2.accurate_votes, 1);
        assert_eq!(rep2.total_votes, 1);
    }
}
