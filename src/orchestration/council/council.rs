//! Orchestration Council — F-GAP-15 (FUTURE5.M1 / BLUE38 §6.6)
//!
//! A multi-agent council that coordinates decision-making among multiple agents.
//! Members submit proposals, cast weighted votes, and the council tallies results
//! to determine outcomes. Supports quorum checks, time-based expiration, and
//! runtime profile snapshots.

use super::types::*;
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

    pub fn sample_deliberation_council() -> OrchestrationCouncil {
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
}
