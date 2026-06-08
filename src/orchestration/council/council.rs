//! Orchestration Council — F-GAP-15 (FUTURE5.M1 / BLUE38 §6.6)
//!
//! A multi-agent council that coordinates decision-making among multiple agents.
//! Members submit proposals, cast weighted votes, and the council tallies results
//! to determine outcomes. Supports quorum checks, time-based expiration, and
//! runtime profile snapshots.

use super::types::*;
use crate::i18n::runtime::tf;
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Multi-agent orchestration council — F-GAP-15.
///
/// Thread-safe: all mutable state is protected behind `Arc<Mutex<>>`.
/// Members vote on proposals with weighted voting power, and outcomes
/// are determined by weighted majority with reputation-based adjustments.
///
/// Reputation learning: members who consistently vote with the council's
/// final outcome gain influence (up to 2.0x); those who vote against
/// consensus lose influence (down to 0.5x). This enables the council to
/// become smarter over time by amplifying the voice of accurate members.
///
/// Deliberation (GAP-B50-08): optional multi-round debate mechanism.
/// When deliberation is active, members submit statements and vote in
/// successive rounds, with the option to change positions between rounds.
#[derive(Debug)]
pub struct OrchestrationCouncil {
    /// Council configuration.
    pub(crate) config: CouncilConfig,
    /// Registered council members keyed by ID.
    pub(crate) members: Arc<Mutex<HashMap<String, CouncilMember>>>,
    /// Proposals keyed by ID.
    pub(crate) proposals: Arc<Mutex<HashMap<String, CouncilProposal>>>,
    /// Votes keyed by (member_id, proposal_id).
    pub(crate) votes: Arc<Mutex<HashMap<(String, String), CouncilVote>>>,
    /// Reputation records keyed by member ID.
    pub(crate) reputation: Arc<Mutex<HashMap<String, ReputationRecord>>>,
    /// Deliberation processes keyed by deliberation ID.
    pub(crate) deliberations: Arc<Mutex<HashMap<DeliberationId, Deliberation>>>,
    /// Configuration for deliberation processes.
    pub(crate) deliberation_config: DeliberationConfig,
}

impl OrchestrationCouncil {
    /// Create a new `OrchestrationCouncil` with the given configuration.
    pub fn new(config: CouncilConfig) -> Self {
        Self {
            config,
            members: Arc::new(Mutex::new(HashMap::new())),
            proposals: Arc::new(Mutex::new(HashMap::new())),
            votes: Arc::new(Mutex::new(HashMap::new())),
            reputation: Arc::new(Mutex::new(HashMap::new())),
            deliberations: Arc::new(Mutex::new(HashMap::new())),
            deliberation_config: DeliberationConfig::default(),
        }
    }

    /// Create a new `OrchestrationCouncil` with deliberation configuration.
    pub fn new_with_deliberation_config(
        config: CouncilConfig,
        deliberation_config: DeliberationConfig,
    ) -> Self {
        Self {
            config,
            members: Arc::new(Mutex::new(HashMap::new())),
            proposals: Arc::new(Mutex::new(HashMap::new())),
            votes: Arc::new(Mutex::new(HashMap::new())),
            reputation: Arc::new(Mutex::new(HashMap::new())),
            deliberations: Arc::new(Mutex::new(HashMap::new())),
            deliberation_config,
        }
    }

    /// Add a new member to the council.
    ///
    /// Returns an error if a member with the same ID already exists.
    pub fn add_member(&self, member: CouncilMember) -> Result<()> {
        let mut members = self
            .members
            .lock()
            .map_err(|e| anyhow!("Failed to acquire lock on members: {}", e))?;

        if members.contains_key(&member.id) {
            return Err(anyhow!(tf(
                "error.council.member_already_exists",
                &[("member_id", &member.id)]
            )));
        }

        members.insert(member.id.clone(), member);
        Ok(())
    }

    /// Remove a member from the council by ID.
    ///
    /// Returns `Ok(true)` if the member was removed, `Ok(false)` if the
    /// member did not exist (no-op).
    pub fn remove_member(&self, id: &str) -> Result<bool> {
        let mut members = self
            .members
            .lock()
            .map_err(|e| anyhow!("Failed to acquire lock on members: {}", e))?;

        Ok(members.remove(id).is_some())
    }

    /// Get a member's details by ID.
    pub fn get_member(&self, id: &str) -> Result<CouncilMember> {
        let members = self
            .members
            .lock()
            .map_err(|e| anyhow!("Failed to acquire lock on members: {}", e))?;

        members
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!(tf("error.council.member_not_found", &[("member_id", id)])))
    }

    /// List all registered council members.
    pub fn list_members(&self) -> Result<Vec<CouncilMember>> {
        let members = self
            .members
            .lock()
            .map_err(|e| anyhow!("Failed to acquire lock on members: {}", e))?;

        let mut list: Vec<CouncilMember> = members.values().cloned().collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(list)
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn sample_member(id: &str, name: &str, role: &str, voting_power: u32) -> CouncilMember {
        CouncilMember {
            id: id.to_string(),
            name: name.to_string(),
            role: role.to_string(),
            voting_power,
            specializations: vec![],
            is_active: true,
            joined_ms: now_epoch_ms(),
        }
    }

    fn sample_proposal(id: &str, title: &str, submitter: &str) -> CouncilProposal {
        CouncilProposal {
            id: id.to_string(),
            title: title.to_string(),
            description: format!("Description for {}", title),
            submitted_by: submitter.to_string(),
            options: vec!["approve".to_string(), "reject".to_string()],
            status: ProposalStatus::Active,
            created_ms: now_epoch_ms(),
        }
    }

    fn default_council() -> OrchestrationCouncil {
        OrchestrationCouncil::new(CouncilConfig {
            name: "Test Council".to_string(),
            min_members_for_quorum: 2,
            voting_duration_ms: 86_400_000,
            max_proposals: 100,
            enable_reputation: true,
            reputation_warmup_rounds: 0, // Zero warmup for tests
            ..Default::default()
        })
    }

    #[test]
    fn test_new_council_empty() {
        let council = default_council();
        let p = council.profile();
        assert_eq!(p.total_members, 0);
        assert_eq!(p.active_members, 0);
        assert_eq!(p.total_proposals, 0);
        assert_eq!(p.passed_count, 0);
        assert_eq!(p.rejected_count, 0);
        assert_eq!(p.pending_count, 0);
    }

    #[test]
    fn test_add_and_list_members() {
        let council = default_council();
        council
            .add_member(sample_member("alice", "Alice", "strategist", 1))
            .unwrap();
        council
            .add_member(sample_member("bob", "Bob", "analyst", 2))
            .unwrap();

        let members = council.list_members().unwrap();
        assert_eq!(members.len(), 2);

        let alice = council.get_member("alice").unwrap();
        assert_eq!(alice.name, "Alice");
        assert_eq!(alice.voting_power, 1);

        let bob = council.get_member("bob").unwrap();
        assert_eq!(bob.name, "Bob");
        assert_eq!(bob.voting_power, 2);
    }

    #[test]
    fn test_remove_member() {
        let council = default_council();
        council
            .add_member(sample_member("alice", "Alice", "strategist", 1))
            .unwrap();

        let removed = council.remove_member("alice").unwrap();
        assert!(removed);
        assert!(council.get_member("alice").is_err());
        assert_eq!(council.list_members().unwrap().len(), 0);
    }

    #[test]
    fn test_remove_nonexistent_member_noop() {
        let council = default_council();
        let removed = council.remove_member("nonexistent").unwrap();
        assert!(!removed);
    }

    #[test]
    fn test_submit_proposal() {
        let council = default_council();
        let proposal = sample_proposal("prop-1", "Increase memory limit", "alice");
        council.submit_proposal(proposal).unwrap();

        let retrieved = council.get_proposal("prop-1").unwrap();
        assert_eq!(retrieved.title, "Increase memory limit");
        assert_eq!(retrieved.submitted_by, "alice");

        // Duplicate should fail.
        let dup = sample_proposal("prop-1", "Duplicate", "bob");
        let err = council.submit_proposal(dup).unwrap_err();
        assert!(
            err.to_string().contains("already exists")
                || err.to_string().contains("error.council."),
            "unexpected error: {}",
            err
        );
    }

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
    fn test_expire_old_proposals() {
        let council = OrchestrationCouncil::new(CouncilConfig {
            name: "Test Council".to_string(),
            min_members_for_quorum: 1,
            voting_duration_ms: 1, // 1 ms → everything expires immediately
            max_proposals: 100,
            enable_reputation: false,
            reputation_warmup_rounds: 0,
            ..Default::default()
        });

        council
            .add_member(sample_member("alice", "Alice", "strategist", 1))
            .unwrap();
        council
            .submit_proposal(sample_proposal("prop-1", "Expire me", "alice"))
            .unwrap();

        // Sleep briefly to ensure time passes.
        std::thread::sleep(Duration::from_millis(5));

        let expired = council.expire_old_proposals().unwrap();
        assert_eq!(expired, 1);

        let proposal = council.get_proposal("prop-1").unwrap();
        assert_eq!(proposal.status, ProposalStatus::Expired);
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
    fn test_get_proposal() {
        let council = default_council();
        let proposal = sample_proposal("prop-42", "The Answer", "deep-thought");
        council.submit_proposal(proposal).unwrap();

        let retrieved = council.get_proposal("prop-42").unwrap();
        assert_eq!(retrieved.title, "The Answer");
        assert_eq!(retrieved.submitted_by, "deep-thought");
        assert_eq!(retrieved.options, vec!["approve", "reject"]);

        // Non-existent should error.
        let err = council.get_proposal("does-not-exist").unwrap_err();
        assert!(
            err.to_string().contains("not found") || err.to_string().contains("error.council."),
            "unexpected error: {}",
            err
        );
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
    fn test_list_proposals_by_status() {
        let council = default_council();
        let mut p1 = sample_proposal("prop-1", "Alpha", "alice");
        p1.status = ProposalStatus::Pending;
        council.submit_proposal(p1).unwrap();

        let mut p2 = sample_proposal("prop-2", "Beta", "bob");
        p2.status = ProposalStatus::Passed;
        council.submit_proposal(p2).unwrap();

        let mut p3 = sample_proposal("prop-3", "Gamma", "carol");
        p3.status = ProposalStatus::Rejected;
        council.submit_proposal(p3).unwrap();

        // List all proposals.
        let all = council.list_proposals(None).unwrap();
        assert_eq!(all.len(), 3);

        // Filter by status.
        let pending = council
            .list_proposals(Some(ProposalStatus::Pending))
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].title, "Alpha");

        let passed = council
            .list_proposals(Some(ProposalStatus::Passed))
            .unwrap();
        assert_eq!(passed.len(), 1);
        assert_eq!(passed[0].title, "Beta");

        let rejected = council
            .list_proposals(Some(ProposalStatus::Rejected))
            .unwrap();
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].title, "Gamma");

        // No active proposals.
        let active = council
            .list_proposals(Some(ProposalStatus::Active))
            .unwrap();
        assert_eq!(active.len(), 0);
    }

    #[test]
    fn test_max_proposals_limit() {
        let council = OrchestrationCouncil::new(CouncilConfig {
            name: "Limited Council".to_string(),
            min_members_for_quorum: 1,
            voting_duration_ms: 86_400_000,
            max_proposals: 2,
            enable_reputation: false,
            reputation_warmup_rounds: 0,
            ..Default::default()
        });

        council
            .submit_proposal(sample_proposal("prop-1", "One", "alice"))
            .unwrap();
        council
            .submit_proposal(sample_proposal("prop-2", "Two", "bob"))
            .unwrap();

        // Since prop-1 and prop-2 were inserted, max is 2, so prop-3 should fail.
        match council.submit_proposal(sample_proposal("prop-3", "Three", "carol")) {
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("Maximum number")
                        || msg.contains("maximum")
                        || msg.contains("error.council."),
                    "Expected limit error, got: {msg}"
                );
            }
            Ok(_) => panic!("expected Err when exceeding max_proposals=2, but got Ok"),
        }
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
    fn test_add_duplicate_member_fails() {
        let council = default_council();
        council
            .add_member(sample_member("alice", "Alice", "strategist", 1))
            .unwrap();

        let err = council
            .add_member(sample_member("alice", "Alice Again", "analyst", 2))
            .unwrap_err();
        assert!(
            err.to_string().contains("already exists")
                || err.to_string().contains("error.council."),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_default_config_values() {
        let config = CouncilConfig::default();
        assert_eq!(config.name, "Orchestration Council");
        assert_eq!(config.min_members_for_quorum, 3);
        assert_eq!(config.voting_duration_ms, 86_400_000);
        assert_eq!(config.max_proposals, 100);
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

    // ─── Deliberation Tests (GAP-B50-08) ──────────────────────────────────

    fn sample_deliberation_council() -> OrchestrationCouncil {
        let council = OrchestrationCouncil::new_with_deliberation_config(
            CouncilConfig {
                name: "Deliberation Test Council".to_string(),
                min_members_for_quorum: 2,
                voting_duration_ms: 86_400_000,
                max_proposals: 100,
                enable_reputation: false,
                reputation_warmup_rounds: 0,
                ..Default::default()
            },
            DeliberationConfig {
                max_rounds: 3,
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
        council
    }

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
