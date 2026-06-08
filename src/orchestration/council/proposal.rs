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
