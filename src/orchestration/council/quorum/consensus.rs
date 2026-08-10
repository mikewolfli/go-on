//! Consensus logic for `OrchestrationCouncil`.
//!
//! Handles auto-ejection of low performers and council profile snapshots.
//!
//! The multi-round deliberation orchestration (`conclude_round`,
//! `run_multi_round_deliberation`, round tallying) was removed as unwired
//! dead code — the wired council path uses `council/voting.rs`.

use super::super::council::OrchestrationCouncil;
use super::super::types::*;

impl OrchestrationCouncil {
    /// Auto-eject members whose accuracy has been persistently low.
    ///
    /// GAP-B49-09: Members with accuracy < ejection_threshold for ejection_window
    /// consecutive rounds are marked as `inactive`. New members get a protection
    /// period of ejection_warmup_rounds before being eligible for ejection.
    pub fn auto_eject_low_performers(&mut self) -> Vec<String> {
        let eject_threshold = self.config.ejection_threshold.unwrap_or(0.3);
        let eject_window = self.config.ejection_window.unwrap_or(20);
        // New members are protected from ejection until they have participated
        // in at least `ejection_warmup_rounds` voting rounds (recorded
        // outcomes). Reputation records are seeded by `cast_vote`;
        // `total_votes` only advances when outcomes are recorded.
        let eject_warmup = self.config.ejection_warmup_rounds.unwrap_or(10);
        let mut ejected = Vec::new();

        let rep = self.reputation.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("council reputation lock poisoned, recovering");
            poisoned.into_inner()
        });

        for (member_id, record) in rep.iter() {
            // Skip members still in the ejection warmup period.
            if (record.total_votes as usize) < eject_warmup {
                continue;
            }
            // Skip members in reputation warmup.
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
