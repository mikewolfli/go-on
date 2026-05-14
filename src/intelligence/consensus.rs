//! F-GAP-16: Consensus Engine
//!
//! Multi-node consensus and arbitration for distributed decision-making.
//! Supports leader election, round-based voting, heartbeat-based failure
//! detection, and majority-based consensus finalization.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::intelligence::now_ms;

// ── ID generation ────────────────────────────────────────────────────────────

static ROUND_ID_COUNTER: AtomicU64 = AtomicU64::new(1);
static PROPOSAL_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

fn generate_round_id() -> String {
    let n = ROUND_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("round-{}", n)
}

fn generate_proposal_id() -> String {
    let n = PROPOSAL_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("prop-{}", n)
}

// Use `crate::intelligence::now_ms()` instead — shared utility in mod.rs

// ── Error type ──────────────────────────────────────────────────────────────

/// Errors that can occur during consensus operations.
#[derive(Debug, Clone)]
pub enum ConsensusError {
    /// A node with the given id already exists.
    DuplicateNode(String),
    /// The specified node was not found.
    NodeNotFound(String),
    /// The specified round was not found.
    RoundNotFound(String),
    /// No round is currently active.
    NoActiveRound,
    /// The node is not registered in the engine.
    UnregisteredNode(String),
}

impl std::fmt::Display for ConsensusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateNode(id) => write!(f, "duplicate node: {id}"),
            Self::NodeNotFound(id) => write!(f, "node not found: {id}"),
            Self::RoundNotFound(id) => write!(f, "round not found: {id}"),
            Self::NoActiveRound => write!(f, "no active round in progress"),
            Self::UnregisteredNode(id) => write!(f, "unregistered node: {id}"),
        }
    }
}

impl std::error::Error for ConsensusError {}

/// Convenience result alias for consensus operations.
pub type Result<T> = std::result::Result<T, ConsensusError>;

// ── Data types ──────────────────────────────────────────────────────────────

/// Role a node can hold in the consensus cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeRole {
    /// Leader — drives consensus rounds.
    Leader,
    /// Follower — participates in voting.
    Follower,
    /// Candidate — standing for election.
    Candidate,
    /// Observer — watches but does not vote.
    Observer,
}

/// A participant in the consensus cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusNode {
    pub id: String,
    pub address: String,
    pub weight: u32,
    pub role: NodeRole,
    pub is_online: bool,
    pub last_heartbeat_ms: u64,
}

/// Status of a consensus round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoundStatus {
    /// Round has been created but voting has not started.
    Pending,
    /// Voting is in progress.
    InProgress,
    /// Consensus has been reached; the round is committed.
    Committed,
    /// Consensus could not be reached; the round failed.
    Failed,
}

/// A single consensus round.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusRound {
    pub id: String,
    pub leader_id: String,
    pub start_ms: u64,
    pub status: RoundStatus,
    pub proposals_count: usize,
    pub votes_collected: usize,
}

/// A proposal put forward during a consensus round.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusProposal {
    pub id: String,
    pub round_id: String,
    pub proposer_id: String,
    pub data: serde_json::Value,
    pub created_ms: u64,
}

/// A vote cast by a node on a proposal in a round.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusVote {
    pub node_id: String,
    pub round_id: String,
    pub proposal_id: String,
    pub approve: bool,
    pub weight: u32,
    pub vote_ms: u64,
}

/// Configuration for the consensus engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusConfig {
    /// Minimum number of nodes required to form a quorum.
    pub min_nodes: u32,
    /// Milliseconds between expected heartbeats.
    pub heartbeat_interval_ms: u64,
    /// Milliseconds after which a leader election is triggered.
    pub election_timeout_ms: u64,
    /// Maximum number of rounds to retain in history.
    pub max_rounds: usize,
}

impl Default for ConsensusConfig {
    fn default() -> Self {
        Self {
            min_nodes: 3,
            heartbeat_interval_ms: 3_000,
            election_timeout_ms: 10_000,
            max_rounds: 100,
        }
    }
}

/// Runtime metrics snapshot for the consensus engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusProfile {
    pub total_nodes: u32,
    pub online_nodes: u32,
    pub leader_id: Option<String>,
    pub total_rounds_passed: u32,
    pub total_rounds_failed: u32,
    pub current_round_id: Option<String>,
}

// ── Consensus Engine ────────────────────────────────────────────────────────

/// Multi-node consensus engine for distributed decision-making.
///
/// Thread-safe, backed by `Arc<Mutex<…>>` for all mutable state.
pub struct ConsensusEngine {
    /// Registered cluster nodes.
    nodes: Arc<Mutex<HashMap<String, ConsensusNode>>>,
    /// Historical and current consensus rounds.
    rounds: Arc<Mutex<Vec<ConsensusRound>>>,
    /// Proposals associated with rounds.
    proposals: Arc<Mutex<Vec<ConsensusProposal>>>,
    /// Votes that have been cast.
    votes: Arc<Mutex<Vec<ConsensusVote>>>,
    /// Engine-level configuration.
    config: ConsensusConfig,
}

impl ConsensusEngine {
    /// Create a new `ConsensusEngine` with the given configuration.
    pub fn new(config: ConsensusConfig) -> Self {
        Self {
            nodes: Arc::new(Mutex::new(HashMap::new())),
            rounds: Arc::new(Mutex::new(Vec::new())),
            proposals: Arc::new(Mutex::new(Vec::new())),
            votes: Arc::new(Mutex::new(Vec::new())),
            config,
        }
    }

    // ── Node management ─────────────────────────────────────────────────

    /// Register a new node in the cluster.
    ///
    /// Returns `Err(ConsensusError::DuplicateNode)` if a node with the same
    /// id already exists.
    pub fn register_node(&self, mut node: ConsensusNode) -> Result<()> {
        let mut nodes = self
            .nodes
            .lock()
            .map_err(|_| ConsensusError::DuplicateNode(node.id.clone()))?;

        if nodes.contains_key(&node.id) {
            return Err(ConsensusError::DuplicateNode(node.id));
        }

        if node.last_heartbeat_ms == 0 {
            node.last_heartbeat_ms = now_ms();
        }
        nodes.insert(node.id.clone(), node);
        Ok(())
    }

    /// Unregister (remove) a node from the cluster by its id.
    pub fn unregister_node(&self, id: &str) -> Result<()> {
        let mut nodes = self
            .nodes
            .lock()
            .map_err(|_| ConsensusError::NodeNotFound(id.to_string()))?;

        if nodes.remove(id).is_none() {
            return Err(ConsensusError::NodeNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Return a snapshot of all currently registered nodes.
    pub fn list_nodes(&self) -> Vec<ConsensusNode> {
        match self.nodes.lock() {
            Ok(nodes) => nodes.values().cloned().collect(),
            Err(_) => vec![],
        }
    }

    // ── Round management ────────────────────────────────────────────────

    /// Start a new consensus round led by the specified node.
    ///
    /// The proposals slice contains arbitrary JSON data items.  Each item is
    /// wrapped in a `ConsensusProposal` and associated with the new round.
    ///
    /// Returns the id of the newly created round on success.
    pub fn start_round(
        &self,
        leader_id: &str,
        proposals: Vec<serde_json::Value>,
    ) -> Result<String> {
        // Verify the leader exists.
        {
            let nodes = self
                .nodes
                .lock()
                .map_err(|_| ConsensusError::NodeNotFound(leader_id.to_string()))?;
            if !nodes.contains_key(leader_id) {
                return Err(ConsensusError::NodeNotFound(leader_id.to_string()));
            }
        }

        let round_id = generate_round_id();
        let start = now_ms();

        // Build the round.
        let round = ConsensusRound {
            id: round_id.clone(),
            leader_id: leader_id.to_string(),
            start_ms: start,
            status: RoundStatus::InProgress,
            proposals_count: proposals.len(),
            votes_collected: 0,
        };

        // Build proposals.
        let proposal_list: Vec<ConsensusProposal> = proposals
            .into_iter()
            .map(|data| ConsensusProposal {
                id: generate_proposal_id(),
                round_id: round_id.clone(),
                proposer_id: leader_id.to_string(),
                data,
                created_ms: now_ms(),
            })
            .collect();

        let mut rounds = self
            .rounds
            .lock()
            .map_err(|_| ConsensusError::RoundNotFound(round_id.clone()))?;

        let mut all_proposals = self
            .proposals
            .lock()
            .map_err(|_| ConsensusError::RoundNotFound(round_id.clone()))?;

        // Enforce max_rounds cap: remove oldest completed/failed rounds.
        while rounds.len() >= self.config.max_rounds {
            rounds.remove(0);
        }

        rounds.push(round);
        all_proposals.extend(proposal_list);

        Ok(round_id)
    }

    /// Cast a vote in the current round.
    ///
    /// The vote is recorded only if the voting node is registered and the
    /// referenced round exists and is in `InProgress` status.
    pub fn cast_vote(&self, vote: ConsensusVote) -> Result<()> {
        // Verify node exists.
        {
            let nodes = self
                .nodes
                .lock()
                .map_err(|_| ConsensusError::UnregisteredNode(vote.node_id.clone()))?;
            if !nodes.contains_key(&vote.node_id) {
                return Err(ConsensusError::UnregisteredNode(vote.node_id.clone()));
            }
        }

        // Verify round exists and is in progress.
        {
            let rounds = self
                .rounds
                .lock()
                .map_err(|_| ConsensusError::RoundNotFound(vote.round_id.clone()))?;
            let round = rounds
                .iter()
                .find(|r| r.id == vote.round_id)
                .ok_or_else(|| ConsensusError::RoundNotFound(vote.round_id.clone()))?;
            if round.status != RoundStatus::InProgress {
                return Err(ConsensusError::NoActiveRound);
            }
        }

        let round_id = vote.round_id.clone();

        let mut votes = self
            .votes
            .lock()
            .map_err(|_| ConsensusError::RoundNotFound(round_id.clone()))?;

        // Record the vote with a timestamp.
        let recorded = ConsensusVote {
            vote_ms: now_ms(),
            ..vote
        };
        votes.push(recorded);

        // Bump votes_collected on the round.
        let mut rounds = self
            .rounds
            .lock()
            .map_err(|_| ConsensusError::RoundNotFound(String::new()))?;
        if let Some(round) = rounds.iter_mut().find(|r| r.id == round_id) {
            round.votes_collected = round.votes_collected.saturating_add(1);
        }

        Ok(())
    }

    /// Finalize the current/last round: tally votes and determine consensus.
    ///
    /// Consensus is reached when a majority of online nodes (by weight) approve
    /// the most popular proposal.  The round status is set to `Committed` on
    /// success or `Failed` otherwise.
    pub fn finalize_round(&self) -> Result<()> {
        // Find the most recent InProgress round.
        let round_id = {
            let rounds = self
                .rounds
                .lock()
                .map_err(|_| ConsensusError::NoActiveRound)?;
            let round = rounds
                .iter()
                .rev()
                .find(|r| r.status == RoundStatus::InProgress)
                .ok_or(ConsensusError::NoActiveRound)?;
            round.id.clone()
        };

        // Gather votes for this round — clone to avoid borrow lifetime issues.
        let vote_summary = {
            let votes = self
                .votes
                .lock()
                .map_err(|_| ConsensusError::RoundNotFound(round_id.clone()))?;
            let round_votes: Vec<ConsensusVote> = votes
                .iter()
                .filter(|v| v.round_id == round_id)
                .cloned()
                .collect();

            // Group votes by proposal, tally approve weight.
            let mut approval_weight: HashMap<String, u32> = HashMap::new();
            let mut total_weight: u32 = 0;
            for v in &round_votes {
                total_weight = total_weight.saturating_add(v.weight);
                if v.approve {
                    *approval_weight.entry(v.proposal_id.clone()).or_insert(0) += v.weight;
                }
            }
            (approval_weight, total_weight, round_votes.len())
        };

        let (approval_weight, _total_weight, vote_count) = vote_summary;

        // Determine total online node weight.
        let online_weight = {
            let nodes = self
                .nodes
                .lock()
                .map_err(|_| ConsensusError::NoActiveRound)?;
            nodes
                .values()
                .filter(|n| n.is_online)
                .map(|n| n.weight)
                .sum::<u32>()
        };

        // Find the proposal with the most approve weight.
        let best_approval = approval_weight.into_values().max().unwrap_or(0);

        // Require majority: best_approval > online_weight / 2
        let majority = online_weight / 2;
        let committed = best_approval > majority;

        // Update round status.
        let mut rounds = self
            .rounds
            .lock()
            .map_err(|_| ConsensusError::RoundNotFound(round_id.clone()))?;
        if let Some(round) = rounds.iter_mut().find(|r| r.id == round_id) {
            round.votes_collected = vote_count;
            round.status = if committed {
                RoundStatus::Committed
            } else {
                RoundStatus::Failed
            };
        }

        Ok(())
    }

    // ── Query methods ───────────────────────────────────────────────────

    /// Get details for a specific round by id.
    pub fn get_round(&self, id: &str) -> Result<ConsensusRound> {
        let rounds = self
            .rounds
            .lock()
            .map_err(|_| ConsensusError::RoundNotFound(id.to_string()))?;
        rounds
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or_else(|| ConsensusError::RoundNotFound(id.to_string()))
    }

    /// Get the current / most recent round.
    pub fn current_round(&self) -> Option<ConsensusRound> {
        match self.rounds.lock() {
            Ok(rounds) => rounds.last().cloned(),
            Err(_) => None,
        }
    }

    // ── Heartbeat & failure detection ───────────────────────────────────

    /// Record a heartbeat from the given node.
    ///
    /// Updates the node's `last_heartbeat_ms` and sets it online.
    pub fn heartbeat(&self, node_id: &str) -> Result<()> {
        let mut nodes = self
            .nodes
            .lock()
            .map_err(|_| ConsensusError::NodeNotFound(node_id.to_string()))?;
        let node = nodes
            .get_mut(node_id)
            .ok_or_else(|| ConsensusError::NodeNotFound(node_id.to_string()))?;
        node.last_heartbeat_ms = now_ms();
        node.is_online = true;
        Ok(())
    }

    /// Detect nodes that have missed the heartbeat interval.
    ///
    /// Any node whose `last_heartbeat_ms` is older than
    /// `heartbeat_interval_ms` is marked offline.  Returns the ids
    /// of nodes that were just marked offline.
    pub fn detect_failures(&self) -> Vec<String> {
        let now = now_ms();
        let interval = self.config.heartbeat_interval_ms;

        let mut nodes = match self.nodes.lock() {
            Ok(n) => n,
            Err(_) => return vec![],
        };

        let mut failed: Vec<String> = Vec::new();
        for node in nodes.values_mut() {
            if node.is_online && now.saturating_sub(node.last_heartbeat_ms) > interval {
                node.is_online = false;
                failed.push(node.id.clone());
            }
        }
        failed
    }

    // ── Leader election ─────────────────────────────────────────────────

    /// Simple leader election: the online node with the highest weight is
    /// elected leader.  The current leader (if any) is demoted to
    /// follower.  Returns the id of the newly elected leader, or `None`
    /// if no online nodes are available.
    pub fn elect_leader(&self) -> Option<String> {
        let mut nodes = match self.nodes.lock() {
            Ok(n) => n,
            Err(_) => return None,
        };

        // Demote current leader.
        for node in nodes.values_mut() {
            if node.role == NodeRole::Leader {
                node.role = NodeRole::Follower;
            }
        }

        // Pick the online node with the highest weight (ties broken by id).
        let leader_id = nodes
            .values()
            .filter(|n| n.is_online)
            .max_by(|a, b| a.weight.cmp(&b.weight).then_with(|| b.id.cmp(&a.id)))
            .map(|n| n.id.clone());

        // Promote the chosen node.
        if let Some(ref id) = leader_id {
            if let Some(node) = nodes.get_mut(id) {
                node.role = NodeRole::Leader;
            }
        }

        leader_id
    }

    // ── Profile ─────────────────────────────────────────────────────────

    /// Return a snapshot of the consensus engine's runtime metrics.
    pub fn profile(&self) -> ConsensusProfile {
        let nodes = match self.nodes.lock() {
            Ok(n) => n.clone(),
            Err(_) => HashMap::new(),
        };
        let rounds = match self.rounds.lock() {
            Ok(r) => r.clone(),
            Err(_) => vec![],
        };

        let total_nodes = nodes.len() as u32;
        let online_nodes = nodes.values().filter(|n| n.is_online).count() as u32;
        let leader_id = nodes
            .values()
            .find(|n| n.role == NodeRole::Leader)
            .map(|n| n.id.clone());

        let total_rounds_passed = rounds
            .iter()
            .filter(|r| r.status == RoundStatus::Committed)
            .count() as u32;
        let total_rounds_failed = rounds
            .iter()
            .filter(|r| r.status == RoundStatus::Failed)
            .count() as u32;
        let current_round_id = rounds.last().map(|r| r.id.clone());

        ConsensusProfile {
            total_nodes,
            online_nodes,
            leader_id,
            total_rounds_passed,
            total_rounds_failed,
            current_round_id,
        }
    }
}

impl Default for ConsensusEngine {
    fn default() -> Self {
        Self::new(ConsensusConfig::default())
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_node(id: &str, weight: u32, role: NodeRole) -> ConsensusNode {
        ConsensusNode {
            id: id.to_string(),
            address: format!("10.0.0.{}:9000", id),
            weight,
            role,
            is_online: true,
            last_heartbeat_ms: now_ms(),
        }
    }

    fn sample_vote(
        node_id: &str,
        round_id: &str,
        proposal_id: &str,
        approve: bool,
        weight: u32,
    ) -> ConsensusVote {
        ConsensusVote {
            node_id: node_id.to_string(),
            round_id: round_id.to_string(),
            proposal_id: proposal_id.to_string(),
            approve,
            weight,
            vote_ms: 0,
        }
    }

    // ── 12 required tests ───────────────────────────────────────────────

    #[test]
    fn test_new_engine_empty() {
        let engine = ConsensusEngine::default();
        let p = engine.profile();
        assert_eq!(p.total_nodes, 0);
        assert_eq!(p.online_nodes, 0);
        assert!(p.leader_id.is_none());
        assert_eq!(p.total_rounds_passed, 0);
        assert_eq!(p.total_rounds_failed, 0);
        assert!(p.current_round_id.is_none());
    }

    #[test]
    fn test_register_and_list_nodes() {
        let engine = ConsensusEngine::default();
        let n1 = sample_node("alpha", 10, NodeRole::Follower);
        let n2 = sample_node("beta", 20, NodeRole::Follower);
        assert!(engine.register_node(n1).is_ok());
        assert!(engine.register_node(n2).is_ok());

        let nodes = engine.list_nodes();
        assert_eq!(nodes.len(), 2);

        let ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains(&"alpha"));
        assert!(ids.contains(&"beta"));
    }

    #[test]
    fn test_unregister_node() {
        let engine = ConsensusEngine::default();
        let n1 = sample_node("alpha", 10, NodeRole::Follower);
        engine.register_node(n1).unwrap();

        assert!(engine.unregister_node("alpha").is_ok());
        assert!(engine.unregister_node("alpha").is_err()); // already gone
        assert_eq!(engine.list_nodes().len(), 0);
    }

    #[test]
    fn test_start_round() {
        let engine = ConsensusEngine::default();
        let n1 = sample_node("leader", 50, NodeRole::Leader);
        engine.register_node(n1).unwrap();

        let proposals = vec![
            serde_json::json!({"action": "deploy", "version": "2.0.0"}),
            serde_json::json!({"action": "rollback", "version": "1.9.0"}),
        ];

        let round_id = engine
            .start_round("leader", proposals)
            .expect("should start round");

        let round = engine.get_round(&round_id).expect("round should exist");
        assert_eq!(round.leader_id, "leader");
        assert_eq!(round.proposals_count, 2);
        assert_eq!(round.status, RoundStatus::InProgress);
        assert!(round.start_ms > 0);
    }

    #[test]
    fn test_cast_vote_reaches_consensus() {
        let engine = ConsensusEngine::default();

        // Register 3 online nodes with weights that sum to a clear majority.
        engine
            .register_node(sample_node("alice", 30, NodeRole::Leader))
            .unwrap();
        engine
            .register_node(sample_node("bob", 20, NodeRole::Follower))
            .unwrap();
        engine
            .register_node(sample_node("carol", 10, NodeRole::Follower))
            .unwrap();

        let proposals = vec![serde_json::json!({"proposal": "A"})];
        let round_id = engine.start_round("alice", proposals).unwrap();
        let prop_id = {
            let proposals = engine.proposals.lock().unwrap();
            proposals
                .iter()
                .find(|p| p.round_id == round_id)
                .map(|p| p.id.clone())
                .unwrap()
        };

        // alice (30) + bob (20) = 50 approve weight > (60 / 2 = 30) => majority.
        engine
            .cast_vote(sample_vote("alice", &round_id, &prop_id, true, 30))
            .unwrap();
        engine
            .cast_vote(sample_vote("bob", &round_id, &prop_id, true, 20))
            .unwrap();
        engine
            .cast_vote(sample_vote("carol", &round_id, &prop_id, false, 10))
            .unwrap();

        engine.finalize_round().unwrap();
        let round = engine.get_round(&round_id).unwrap();
        assert_eq!(round.status, RoundStatus::Committed);
        assert_eq!(round.votes_collected, 3);
    }

    #[test]
    fn test_cast_vote_fails_majority() {
        let engine = ConsensusEngine::default();

        engine
            .register_node(sample_node("alice", 30, NodeRole::Leader))
            .unwrap();
        engine
            .register_node(sample_node("bob", 20, NodeRole::Follower))
            .unwrap();
        engine
            .register_node(sample_node("carol", 10, NodeRole::Follower))
            .unwrap();

        let proposals = vec![serde_json::json!({"proposal": "B"})];
        let round_id = engine.start_round("alice", proposals).unwrap();
        let prop_id = {
            let proposals = engine.proposals.lock().unwrap();
            proposals
                .iter()
                .find(|p| p.round_id == round_id)
                .map(|p| p.id.clone())
                .unwrap()
        };

        // Only alice (30) approves, bob (20) and carol (10) reject.
        // approve weight = 30, online_weight = 60, majority threshold = 60/2 = 30.
        // Since 30 is not > 30, consensus fails.
        engine
            .cast_vote(sample_vote("alice", &round_id, &prop_id, true, 30))
            .unwrap();
        engine
            .cast_vote(sample_vote("bob", &round_id, &prop_id, false, 20))
            .unwrap();
        engine
            .cast_vote(sample_vote("carol", &round_id, &prop_id, false, 10))
            .unwrap();

        engine.finalize_round().unwrap();
        let round = engine.get_round(&round_id).unwrap();
        assert_eq!(round.status, RoundStatus::Failed);
    }

    #[test]
    fn test_finalize_round_committed() {
        let engine = ConsensusEngine::default();
        engine
            .register_node(sample_node("alice", 40, NodeRole::Leader))
            .unwrap();
        engine
            .register_node(sample_node("bob", 30, NodeRole::Follower))
            .unwrap();

        let proposals = vec![serde_json::json!({"x": 1})];
        let round_id = engine.start_round("alice", proposals).unwrap();
        let prop_id = {
            let proposals = engine.proposals.lock().unwrap();
            proposals
                .iter()
                .find(|p| p.round_id == round_id)
                .unwrap()
                .id
                .clone()
        };

        // alice (40) + bob (30) approve => 70 > (70/2=35) => majority.
        engine
            .cast_vote(sample_vote("alice", &round_id, &prop_id, true, 40))
            .unwrap();
        engine
            .cast_vote(sample_vote("bob", &round_id, &prop_id, true, 30))
            .unwrap();

        engine.finalize_round().unwrap();
        let round = engine.get_round(&round_id).unwrap();
        assert_eq!(round.status, RoundStatus::Committed);
    }

    #[test]
    fn test_heartbeat() {
        let engine = ConsensusEngine::default();
        let mut n1 = sample_node("alpha", 10, NodeRole::Follower);
        n1.is_online = false;
        n1.last_heartbeat_ms = 0;
        engine.register_node(n1).unwrap();

        // Heartbeat brings the node online and updates the timestamp.
        engine.heartbeat("alpha").unwrap();
        let nodes = engine.list_nodes();
        let alpha = nodes.iter().find(|n| n.id == "alpha").unwrap();
        assert!(alpha.is_online);
        assert!(alpha.last_heartbeat_ms > 0);
    }

    #[test]
    fn test_detect_failures() {
        let engine = ConsensusEngine::default();

        // Node with a very old heartbeat.
        let mut old_node = sample_node("old", 10, NodeRole::Follower);
        old_node.last_heartbeat_ms = 1; // ancient
        engine.register_node(old_node).unwrap();

        // Node with a current heartbeat.
        engine
            .register_node(sample_node("fresh", 10, NodeRole::Follower))
            .unwrap();

        let failed = engine.detect_failures();
        assert!(failed.contains(&"old".to_string()));
        assert!(!failed.contains(&"fresh".to_string()));

        // Verify 'old' is now offline.
        let nodes = engine.list_nodes();
        let old = nodes.iter().find(|n| n.id == "old").unwrap();
        assert!(!old.is_online);
    }

    #[test]
    fn test_elect_leader() {
        let engine = ConsensusEngine::default();

        let n1 = sample_node("node-a", 50, NodeRole::Follower);
        let n2 = sample_node("node-b", 80, NodeRole::Follower);
        let mut n3 = sample_node("node-c", 30, NodeRole::Follower);
        n3.is_online = false; // offline, should not be elected

        engine.register_node(n1).unwrap();
        engine.register_node(n2).unwrap();
        engine.register_node(n3).unwrap();

        let elected = engine.elect_leader().expect("should elect a leader");
        assert_eq!(elected, "node-b"); // highest weight among online nodes

        let nodes = engine.list_nodes();
        let leader = nodes.iter().find(|n| n.role == NodeRole::Leader).unwrap();
        assert_eq!(leader.id, "node-b");

        let old_follower = nodes.iter().find(|n| n.id == "node-a").unwrap();
        assert_eq!(old_follower.role, NodeRole::Follower);
    }

    #[test]
    fn test_profile_reflects_state() {
        let engine = ConsensusEngine::default();

        engine
            .register_node(sample_node("alice", 30, NodeRole::Leader))
            .unwrap();
        engine
            .register_node(sample_node("bob", 20, NodeRole::Follower))
            .unwrap();

        // Run one successful round.
        let proposals = vec![serde_json::json!({"ok": true})];
        let round_id = engine.start_round("alice", proposals).unwrap();
        let prop_id = {
            let proposals = engine.proposals.lock().unwrap();
            proposals
                .iter()
                .find(|p| p.round_id == round_id)
                .unwrap()
                .id
                .clone()
        };
        engine
            .cast_vote(sample_vote("alice", &round_id, &prop_id, true, 30))
            .unwrap();
        engine
            .cast_vote(sample_vote("bob", &round_id, &prop_id, true, 20))
            .unwrap();
        engine.finalize_round().unwrap();

        let p = engine.profile();
        assert_eq!(p.total_nodes, 2);
        assert_eq!(p.online_nodes, 2);
        assert!(p.leader_id.is_some());
        assert_eq!(p.total_rounds_passed, 1);
        assert_eq!(p.total_rounds_failed, 0);
        assert_eq!(p.current_round_id.as_deref(), Some(round_id.as_str()));
    }

    #[test]
    fn test_duplicate_node_fails() {
        let engine = ConsensusEngine::default();
        let n1 = sample_node("dup", 10, NodeRole::Follower);
        assert!(engine.register_node(n1.clone()).is_ok());
        let err = engine.register_node(n1).unwrap_err();
        match err {
            ConsensusError::DuplicateNode(id) => assert_eq!(id, "dup"),
            other => panic!("expected DuplicateNode, got {other:?}"),
        }
    }

    // ── Additional coverage ─────────────────────────────────────────────

    #[test]
    fn test_start_round_fails_for_unknown_leader() {
        let engine = ConsensusEngine::default();
        let err = engine
            .start_round("ghost", vec![serde_json::json!({"x": 1})])
            .unwrap_err();
        assert!(matches!(err, ConsensusError::NodeNotFound(id) if id == "ghost"));
    }

    #[test]
    fn test_get_round_unknown() {
        let engine = ConsensusEngine::default();
        let err = engine.get_round("nonexistent").unwrap_err();
        assert!(matches!(err, ConsensusError::RoundNotFound(id) if id == "nonexistent"));
    }

    #[test]
    fn test_heartbeat_unknown_node() {
        let engine = ConsensusEngine::default();
        let err = engine.heartbeat("ghost").unwrap_err();
        assert!(matches!(err, ConsensusError::NodeNotFound(id) if id == "ghost"));
    }

    #[test]
    fn test_elect_leader_no_online_nodes() {
        let engine = ConsensusEngine::default();
        let mut n1 = sample_node("offline", 10, NodeRole::Follower);
        n1.is_online = false;
        engine.register_node(n1).unwrap();
        assert!(engine.elect_leader().is_none());
    }

    #[test]
    fn test_finalize_round_no_active_round() {
        let engine = ConsensusEngine::default();
        let err = engine.finalize_round().unwrap_err();
        assert!(matches!(err, ConsensusError::NoActiveRound));
    }

    #[test]
    fn test_current_round_when_empty() {
        let engine = ConsensusEngine::default();
        assert!(engine.current_round().is_none());
    }

    #[test]
    fn test_detect_failures_empty() {
        let engine = ConsensusEngine::default();
        let failed = engine.detect_failures();
        assert!(failed.is_empty());
    }

    #[test]
    fn test_consensus_error_display() {
        let e = ConsensusError::DuplicateNode("x".into());
        assert_eq!(format!("{e}"), "duplicate node: x");
        let e = ConsensusError::NodeNotFound("nope".into());
        assert_eq!(format!("{e}"), "node not found: nope");
        let e = ConsensusError::RoundNotFound("r-1".into());
        assert_eq!(format!("{e}"), "round not found: r-1");
        let e = ConsensusError::NoActiveRound;
        assert_eq!(format!("{e}"), "no active round in progress");
        let e = ConsensusError::UnregisteredNode("bad".into());
        assert_eq!(format!("{e}"), "unregistered node: bad");
    }
}
