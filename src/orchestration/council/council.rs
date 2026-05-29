//! Orchestration Council — F-GAP-15 (FUTURE5.M1 / BLUE38 §6.6)
//!
//! A multi-agent council that coordinates decision-making among multiple agents.
//! Members submit proposals, cast weighted votes, and the council tallies results
//! to determine outcomes. Supports quorum checks, time-based expiration, and
//! runtime profile snapshots.

use crate::i18n::runtime::tf;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

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
}

fn default_enable_reputation() -> bool {
    true
}

fn default_reputation_warmup_rounds() -> u32 {
    5
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
    config: CouncilConfig,
    /// Registered council members keyed by ID.
    members: Arc<Mutex<HashMap<String, CouncilMember>>>,
    /// Proposals keyed by ID.
    proposals: Arc<Mutex<HashMap<String, CouncilProposal>>>,
    /// Votes keyed by (member_id, proposal_id).
    votes: Arc<Mutex<HashMap<(String, String), CouncilVote>>>,
    /// Reputation records keyed by member ID.
    reputation: Arc<Mutex<HashMap<String, ReputationRecord>>>,
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

    /// Initialize reputation tracking for a member (called automatically on first vote).
    #[allow(dead_code)] // F-GAP-15 — kept for external API consistency; used indirectly via record_vote_accuracy
    fn ensure_reputation(&self, member_id: &str) {
        if let Ok(mut rep) = self.reputation.lock() {
            if !rep.contains_key(member_id) {
                rep.insert(
                    member_id.to_string(),
                    ReputationRecord::new(member_id, self.config.reputation_warmup_rounds),
                );
            }
        }
    }

    /// Get the effective voting power for a member, accounting for reputation.
    /// When reputation is disabled or member is in warmup, returns nominal voting_power.
    fn effective_voting_power(&self, member_id: &str, nominal_power: u32) -> u32 {
        if !self.config.enable_reputation {
            return nominal_power;
        }
        if let Ok(rep) = self.reputation.lock() {
            if let Some(record) = rep.get(member_id) {
                if record.warmup_remaining > 0 {
                    return nominal_power;
                }
                let adjusted = (nominal_power as f64 * record.influence_multiplier).round() as u32;
                return adjusted.max(1); // Minimum voting power of 1
            }
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
        self.reputation.lock().ok()?.get(member_id).cloned()
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

    /// Return a `CouncilProfile` snapshot reflecting the current state.
    pub fn profile(&self) -> CouncilProfile {
        let members = self.members.lock().ok();
        let proposals = self.proposals.lock().ok();

        let total_members = members.as_ref().map(|m| m.len() as u32).unwrap_or(0);
        let active_members = members
            .as_ref()
            .map(|m| m.values().filter(|m| m.is_active).count() as u32)
            .unwrap_or(0);
        let total_proposals = proposals.as_ref().map(|p| p.len() as u32).unwrap_or(0);

        let passed_count = proposals
            .as_ref()
            .map(|p| {
                p.values()
                    .filter(|pr| pr.status == ProposalStatus::Passed)
                    .count() as u32
            })
            .unwrap_or(0);

        let rejected_count = proposals
            .as_ref()
            .map(|p| {
                p.values()
                    .filter(|pr| pr.status == ProposalStatus::Rejected)
                    .count() as u32
            })
            .unwrap_or(0);

        let pending_count = proposals
            .as_ref()
            .map(|p| {
                p.values()
                    .filter(|pr| {
                        pr.status == ProposalStatus::Pending || pr.status == ProposalStatus::Active
                    })
                    .count() as u32
            })
            .unwrap_or(0);

        let tied_count = proposals
            .as_ref()
            .map(|p| {
                p.values()
                    .filter(|pr| pr.status == ProposalStatus::Tied)
                    .count() as u32
            })
            .unwrap_or(0);

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

/// Returns the current Unix timestamp in milliseconds.
fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
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
            if let Ok(mut rep) = council.reputation.lock() {
                if let Some(r) = rep.get_mut("high-acc") {
                    r.record_outcome(true);
                }
            }
            council.ensure_reputation("low-acc");
            if let Ok(mut rep) = council.reputation.lock() {
                if let Some(r) = rep.get_mut("low-acc") {
                    r.record_outcome(false);
                }
            }
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
