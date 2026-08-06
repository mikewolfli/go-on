//! Orchestration Council — F-GAP-15 (FUTURE5.M1 / BLUE38 §6.6)
//!
//! A multi-agent council that coordinates decision-making among multiple agents.
//! Members submit proposals, cast weighted votes, and the council tallies results
//! to determine outcomes. Supports quorum checks, time-based expiration, and
//! runtime profile snapshots.

use super::types::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::time::{self, Duration};

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
        }
    }

    /// Start a background task that periodically auto-ejects low-performer
    /// members based on `ejection_threshold` / `ejection_window` config values.
    ///
    /// This should be called during initialization whenever the council is
    /// wrapped in `Arc<Mutex<>>` (e.g. from `CapabilityBus::new`).
    ///
    /// The check runs every 300 seconds (5 minutes) by default.
    pub fn start_auto_ejection(council: Arc<Mutex<Self>>) {
        let interval_secs = {
            let guard = council.lock().unwrap_or_else(|e| e.into_inner());
            let cfg = &guard.config;
            // Use the ejection check interval if configured, otherwise 300s
            cfg.ejection_check_interval_s.unwrap_or(300)
        };
        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(interval_secs));
            // Skip the first tick to give the system time to stabilize
            interval.tick().await;
            loop {
                interval.tick().await;
                if let Ok(mut council) = council.lock() {
                    let ejected = council.auto_eject_low_performers();
                    if !ejected.is_empty() {
                        tracing::info!(?ejected, "Council auto-ejected low-performer members");
                    }
                }
            }
        });
    }
}

// ─── Shared Test Helpers ─────────────────────────────────────────────────────
// These are used by tests in sub-modules (member.rs, proposal.rs, voting.rs, quorum.rs).

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub fn sample_member(id: &str, name: &str, role: &str, voting_power: u32) -> CouncilMember {
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

    pub fn sample_proposal(id: &str, title: &str, submitter: &str) -> CouncilProposal {
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

    pub fn default_council() -> OrchestrationCouncil {
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
}
