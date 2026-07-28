//! Type definitions for the Orchestration Council.
//!
//! Includes all structs, enums, default implementations, helper functions,
//! and the `ReputationRecord` implementation.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The status of a council proposal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ProposalStatus {
    /// Proposal has been submitted but voting has not yet started.
    Pending,
    /// Proposal is open for voting.
    Active,
    /// Proposal has passed (quorum met and majority reached).
    Passed,
    /// Proposal has been rejected.
    Rejected,
    /// Voting resulted in a tie (no single winner).
    Tied,
    /// Proposal expired before reaching a conclusion.
    Expired,
}

/// A member agent in the council.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CouncilMember {
    /// Unique member identifier.
    pub id: String,
    /// Display name of the member.
    pub name: String,
    /// Role or function the member serves.
    pub role: String,
    /// Weight of this member's vote (higher = more influence).
    pub voting_power: u32,
    /// Areas of expertise or specialization.
    pub specializations: Vec<String>,
    /// Whether the member is currently active in the council.
    pub is_active: bool,
    /// Unix timestamp (milliseconds) when the member joined.
    pub joined_ms: u64,
}

/// A proposal submitted for the council to vote on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CouncilProposal {
    /// Unique proposal identifier.
    pub id: String,
    /// Short human-readable title.
    pub title: String,
    /// Detailed description of the proposal.
    pub description: String,
    /// Member ID of the submitter.
    pub submitted_by: String,
    /// Voting options (e.g. ["approve", "reject", "abstain"]).
    pub options: Vec<String>,
    /// Current status of the proposal.
    pub status: ProposalStatus,
    /// Unix timestamp (milliseconds) when the proposal was created.
    pub created_ms: u64,
}

/// A vote cast by a council member on a proposal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CouncilVote {
    /// ID of the member casting the vote.
    pub member_id: String,
    /// ID of the proposal being voted on.
    pub proposal_id: String,
    /// Which option the member selected.
    pub selected_option: String,
    /// Voting weight applied (typically matches member's voting_power).
    pub weight: u32,
    /// Unix timestamp (milliseconds) when the vote was cast.
    pub vote_ms: u64,
    /// Optional justification or rationale for the vote.
    pub rationale: Option<String>,
}

/// Outcome of tallying votes for a proposal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoteResult {
    /// ID of the proposal that was tallied.
    pub proposal_id: String,
    /// Per-option vote tallies (option → total weighted votes).
    pub option_tallies: HashMap<String, u32>,
    /// Total weighted votes cast.
    pub total_votes: u32,
    /// Whether the proposal passed.
    pub passed: bool,
    /// The winning option, if any.
    pub winning_option: Option<String>,
    /// Whether the vote resulted in a tie.
    pub tie: bool,
}

/// Configuration for the orchestration council.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CouncilConfig {
    /// Human-readable name for this council.
    pub name: String,
    /// Minimum number of active members required for a quorum.
    pub min_members_for_quorum: u32,
    /// Duration (in milliseconds) that voting remains open.
    pub voting_duration_ms: u64,
    /// Maximum number of proposals tracked at once.
    pub max_proposals: usize,
    /// Enable reputation-based voting power adjustment.
    /// When enabled, members who vote accurately gain influence over time.
    #[serde(default = "default_enable_reputation")]
    pub enable_reputation: bool,
    /// Number of voting rounds before reputation affects voting power.
    #[serde(default = "default_reputation_warmup_rounds")]
    pub reputation_warmup_rounds: u32,
    /// Threshold below which a member is auto-ejected (default: 0.3)
    pub ejection_threshold: Option<f64>,
    /// Number of consecutive rounds of low accuracy before ejection (default: 20)
    pub ejection_window: Option<usize>,
    /// Warmup rounds before a new member can be ejected (default: 10)
    pub ejection_warmup_rounds: Option<usize>,
    /// Interval in seconds between auto-ejection background checks (default: 300).
    /// Set to 0 or None to use the default of 300 seconds (5 minutes).
    pub ejection_check_interval_s: Option<u64>,

    /// Minimum number of active members to trigger multi-round deliberation.
    /// When active members >= this threshold, proposals automatically use
    /// multi-round deliberation instead of single-round voting.
    /// Set to 0 to disable multi-round deliberation entirely.
    #[serde(default = "default_deliberation_member_threshold")]
    pub deliberation_member_threshold: usize,
}

fn default_enable_reputation() -> bool {
    true
}

fn default_reputation_warmup_rounds() -> u32 {
    5
}

fn default_deliberation_member_threshold() -> usize {
    0
}

impl Default for CouncilConfig {
    fn default() -> Self {
        Self {
            name: "Orchestration Council".to_string(),
            min_members_for_quorum: 3,
            voting_duration_ms: 86_400_000, // 24 hours
            max_proposals: 100,
            enable_reputation: default_enable_reputation(),
            reputation_warmup_rounds: default_reputation_warmup_rounds(),
            ejection_threshold: None,
            ejection_window: None,
            ejection_warmup_rounds: None,
            ejection_check_interval_s: None,
            deliberation_member_threshold: default_deliberation_member_threshold(),
        }
    }
}

/// Wrapper for deliberation identifiers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct DeliberationId(pub String);

impl std::fmt::Display for DeliberationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Position a council member can take during deliberation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum CouncilPosition {
    Support,
    Oppose,
    Amend,
    Abstain,
}

/// A statement made by a council member during a deliberation round.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliberationStatement {
    /// ID of the member making the statement.
    pub member_id: String,
    /// The member's position on the proposal.
    pub position: CouncilPosition,
    /// Detailed reasoning for the position.
    pub reasoning: String,
    /// Proposed amendments to the proposal (if position is Amend).
    pub amendments: Vec<String>,
    /// Unix timestamp (milliseconds) when the statement was submitted.
    pub submitted_at: u64,
}

/// A single round of deliberation within a multi-round debate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliberationRound {
    /// Round number (1-based).
    pub round_number: usize,
    /// Statements submitted by members in this round.
    pub statements: Vec<DeliberationStatement>,
    /// Votes cast by members in this round.
    pub votes: Vec<CouncilVote>,
    /// Whether this round has been concluded.
    pub concluded: bool,
}

/// A multi-round deliberation process for a proposal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deliberation {
    /// Unique deliberation identifier.
    pub id: DeliberationId,
    /// ID of the proposal under deliberation.
    pub proposal_id: String,
    /// Rounds of debate that have occurred.
    pub rounds: Vec<DeliberationRound>,
    /// Maximum number of rounds allowed.
    pub max_rounds: usize,
    /// Whether consensus has been reached.
    pub consensus_reached: bool,
    /// Final decision, if the deliberation has concluded.
    pub final_decision: Option<CouncilDecision>,
    /// Unix timestamp (milliseconds) when deliberation started.
    pub started_at: u64,
}

/// Decision reached by the council after deliberation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CouncilDecision {
    /// The final position adopted.
    pub position: CouncilPosition,
    /// Amended proposal text, if any amendments were adopted.
    pub amended_text: Option<String>,
    /// Round number at which the decision was reached.
    pub decided_at_round: usize,
}

/// Configuration for deliberation processes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliberationConfig {
    /// Maximum number of rounds (default: 3).
    #[serde(default = "default_max_rounds")]
    pub max_rounds: usize,
    /// Whether consensus is required for a decision (default: false).
    #[serde(default)]
    pub require_consensus: bool,
    /// Timeout in seconds for debate within a round (default: 60).
    #[serde(default = "default_debate_timeout_secs")]
    pub debate_timeout_secs: u64,
}

fn default_max_rounds() -> usize {
    3
}

fn default_debate_timeout_secs() -> u64 {
    60
}

impl Default for DeliberationConfig {
    fn default() -> Self {
        Self {
            max_rounds: 3,
            require_consensus: false,
            debate_timeout_secs: 60,
        }
    }
}

/// Runtime profile snapshot for the orchestration council.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CouncilProfile {
    /// Total number of registered members.
    pub total_members: u32,
    /// Number of currently active members.
    pub active_members: u32,
    /// Total proposals ever submitted.
    pub total_proposals: u32,
    /// Number of proposals that passed.
    pub passed_count: u32,
    /// Number of proposals that were rejected.
    pub rejected_count: u32,
    /// Number of proposals still pending or active.
    pub pending_count: u32,
    /// Number of proposals that ended in a tie.
    pub tied_count: u32,
    /// Number of members with adjusted voting power due to reputation.
    pub reputation_adjusted_members: u32,
}

/// Reputation tracking record for a council member.
///
/// Tracks voting accuracy over time to enable adaptive voting power.
/// Members who consistently vote with the council's final outcome gain
/// influence; those who vote against consensus lose influence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReputationRecord {
    /// Member ID this record belongs to.
    pub member_id: String,
    /// Total votes cast by this member.
    pub total_votes: u64,
    /// Number of times this member voted with the majority outcome.
    pub accurate_votes: u64,
    /// Current accuracy ratio (0.0–1.0).
    pub accuracy: f64,
    /// Rolling window of recent vote outcomes (true=accurate, false=inaccurate).
    /// Limited to the last 50 votes.
    pub recent_window: Vec<bool>,
    /// Current effective voting power multiplier (0.5–2.0).
    /// Starts at 1.0 and adjusts based on accuracy.
    pub influence_multiplier: f64,
    /// Number of warmup rounds remaining before reputation takes effect.
    pub warmup_remaining: u32,
}

impl ReputationRecord {
    /// Create a new reputation record for a member.
    pub fn new(member_id: &str, warmup_rounds: u32) -> Self {
        Self {
            member_id: member_id.to_string(),
            total_votes: 0,
            accurate_votes: 0,
            accuracy: 0.5,
            recent_window: Vec::with_capacity(50),
            influence_multiplier: 1.0,
            warmup_remaining: warmup_rounds,
        }
    }

    /// Record whether this member's vote was accurate (matched outcome).
    /// Returns the updated influence multiplier.
    pub fn record_outcome(&mut self, was_accurate: bool) -> f64 {
        self.total_votes += 1;
        if was_accurate {
            self.accurate_votes += 1;
        }
        self.recent_window.push(was_accurate);
        if self.recent_window.len() > 50 {
            self.recent_window.remove(0);
        }

        // Compute accuracy from recent window (exponential focus on recent)
        let recent_accuracy = if self.recent_window.is_empty() {
            0.5
        } else {
            // Weight recent votes more heavily
            let weighted_sum: f64 = self
                .recent_window
                .iter()
                .enumerate()
                .map(|(i, &acc)| {
                    let weight = 1.0 + (i as f64 / self.recent_window.len() as f64) * 2.0;
                    if acc {
                        weight
                    } else {
                        0.0
                    }
                })
                .sum();
            let total_weight: f64 = self
                .recent_window
                .iter()
                .enumerate()
                .map(|(i, _)| 1.0 + (i as f64 / self.recent_window.len() as f64) * 2.0)
                .sum();
            weighted_sum / total_weight.max(1.0)
        };

        self.accuracy = (self.accuracy * 0.3 + recent_accuracy * 0.7).clamp(0.0, 1.0);

        // Decrease warmup
        if self.warmup_remaining > 0 {
            self.warmup_remaining -= 1;
            self.influence_multiplier = 1.0;
        } else {
            // Adjust influence multiplier based on accuracy
            // accuracy 0.5 → multiplier 1.0, accuracy 0.9 → 1.8, accuracy 0.2 → 0.5
            self.influence_multiplier = (0.5 + self.accuracy).clamp(0.5, 2.0);
        }

        self.influence_multiplier
    }
}

/// Returns the current Unix timestamp in milliseconds.
pub fn now_epoch_ms() -> u64 {
    crate::shared::timestamps::now_ts_ms() as u64
}
