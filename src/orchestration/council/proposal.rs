//! Proposal-related methods for `OrchestrationCouncil`.
//!
//! Handles submitting, retrieving, listing, and expiring proposals.

use super::council::OrchestrationCouncil;
use super::types::*;
use crate::i18n::runtime::tf;
use anyhow::{anyhow, Result};

impl OrchestrationCouncil {
    /// Submit a new proposal to the council.
    ///
    /// The proposal's `id` field is used as-is; if you need auto-generation
    /// you must provide it beforehand. Returns an error if a proposal with
    /// the same ID already exists or if the max proposals limit is reached.
    pub fn submit_proposal(&self, proposal: CouncilProposal) -> Result<()> {
        let mut proposals = self
            .proposals
            .lock()
            .map_err(|e| anyhow!("Failed to acquire lock on proposals: {}", e))?;

        if proposals.contains_key(&proposal.id) {
            return Err(anyhow!(tf(
                "error.council.proposal_already_exists",
                &[("proposal_id", &proposal.id)]
            )));
        }

        if proposals.len() >= self.config.max_proposals {
            return Err(anyhow!(tf(
                "error.council.max_proposals_reached",
                &[("max", &self.config.max_proposals.to_string())]
            )));
        }

        proposals.insert(proposal.id.clone(), proposal);
        Ok(())
    }

    /// Get a proposal's details by ID.
    pub fn get_proposal(&self, id: &str) -> Result<CouncilProposal> {
        let proposals = self
            .proposals
            .lock()
            .map_err(|e| anyhow!("Failed to acquire lock on proposals: {}", e))?;

        proposals.get(id).cloned().ok_or_else(|| {
            anyhow!(tf(
                "error.council.proposal_not_found",
                &[("proposal_id", id)]
            ))
        })
    }

    /// List proposals, optionally filtered by status.
    ///
    /// If `status_filter` is `None`, all proposals are returned.
    pub fn list_proposals(
        &self,
        status_filter: Option<ProposalStatus>,
    ) -> Result<Vec<CouncilProposal>> {
        let proposals = self
            .proposals
            .lock()
            .map_err(|e| anyhow!("Failed to acquire lock on proposals: {}", e))?;

        let mut list: Vec<CouncilProposal> = match status_filter {
            Some(ref status) => proposals
                .values()
                .filter(|p| p.status == *status)
                .cloned()
                .collect(),
            None => proposals.values().cloned().collect(),
        };

        list.sort_by_key(|a| a.created_ms);
        Ok(list)
    }

    /// Expire proposals whose voting window has elapsed.
    ///
    /// Proposals with status `Active` that were created more than
    /// `voting_duration_ms` ago are marked as `Expired`.
    pub fn expire_old_proposals(&self) -> Result<u32> {
        let now_ms = now_epoch_ms();
        let mut proposals = self
            .proposals
            .lock()
            .map_err(|e| anyhow!("Failed to acquire lock on proposals: {}", e))?;

        let mut expired_count = 0u32;
        for proposal in proposals.values_mut() {
            if proposal.status == ProposalStatus::Active
                && now_ms > proposal.created_ms + self.config.voting_duration_ms
            {
                proposal.status = ProposalStatus::Expired;
                expired_count += 1;
            }
        }

        Ok(expired_count)
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::council::test_support::*;
    use super::super::council::OrchestrationCouncil;
    use super::super::types::*;

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
        std::thread::sleep(std::time::Duration::from_millis(5));

        let expired = council.expire_old_proposals().unwrap();
        assert_eq!(expired, 1);

        let proposal = council.get_proposal("prop-1").unwrap();
        assert_eq!(proposal.status, ProposalStatus::Expired);
    }

    #[test]
    fn test_default_config_values() {
        let config = CouncilConfig::default();
        assert_eq!(config.name, "Orchestration Council");
        assert_eq!(config.min_members_for_quorum, 3);
        assert_eq!(config.voting_duration_ms, 86_400_000);
        assert_eq!(config.max_proposals, 100);
    }
}
