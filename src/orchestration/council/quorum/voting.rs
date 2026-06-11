//! Voting logic for `OrchestrationCouncil` deliberation rounds.
//!
//! Handles submitting statements and casting votes within a deliberation round.

use super::super::council::OrchestrationCouncil;
use super::super::types::*;
use anyhow::{anyhow, Result};

impl OrchestrationCouncil {
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
}
