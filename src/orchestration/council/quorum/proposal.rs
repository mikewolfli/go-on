//! Proposal management for `OrchestrationCouncil`.
//!
//! Handles starting deliberations and routing proposals through the voting path.

use super::super::council::OrchestrationCouncil;
use super::super::types::*;
use anyhow::{anyhow, Result};

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
}
