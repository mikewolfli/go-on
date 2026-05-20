//! F-GAP-19: Federated Reinforcement Learning (BLUE38)
//!
//! Cross-node policy distillation for distributed reinforcement learning.
//! Nodes share local policy snapshots (reward + sample count), which are
//! merged via reward-weighted averaging during distillation rounds.
//!
//! All mutable state is guarded behind `Arc<Mutex<…>>` for thread-safe
//! access across asynchronous boundaries.

use crate::intelligence::lock_guard;
use crate::intelligence::now_ms;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

// ── ID generation ───────────────────────────────────────────────────────────

static POLICY_ID_COUNTER: AtomicU64 = AtomicU64::new(1);
static ROUND_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

fn generate_policy_id() -> String {
    let n = POLICY_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("policy-{}", n)
}

fn generate_round_id() -> String {
    let n = ROUND_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("round-{}", n)
}

// ── Error type ──────────────────────────────────────────────────────────────

/// Errors that can occur during federated RL operations.
#[derive(Debug, Clone)]
pub enum FederatedError {
    /// A policy with the given id was not found.
    PolicyNotFound(String),
    /// A round with the given id was not found.
    RoundNotFound(String),
    /// The round is not in a state that allows contribution.
    RoundNotActive(String),
    /// The specified policy has already been contributed to this round.
    PolicyAlreadyContributed(String),
    /// There are not enough contributors to complete the round.
    InsufficientContributors { have: u32, need: u32 },
    /// The policy data for a contributed policy was unexpectedly missing.
    MissingPolicyData(String),
}

impl std::fmt::Display for FederatedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PolicyNotFound(id) => write!(f, "policy not found: {id}"),
            Self::RoundNotFound(id) => write!(f, "distillation round not found: {id}"),
            Self::RoundNotActive(id) => write!(f, "round is not active: {id}"),
            Self::PolicyAlreadyContributed(id) => {
                write!(f, "policy already contributed to this round: {id}")
            }
            Self::InsufficientContributors { have, need } => {
                write!(
                    f,
                    "insufficient contributors: have {have}, need at least {need}"
                )
            }
            Self::MissingPolicyData(id) => write!(f, "policy data is missing for policy: {id}"),
        }
    }
}

impl std::error::Error for FederatedError {}

/// Convenience result alias for federated RL operations.
pub type Result<T> = std::result::Result<T, FederatedError>;

// ── Data structures ─────────────────────────────────────────────────────────

/// Status of a distillation round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DistillationStatus {
    /// Round has been created but is not yet accepting contributions.
    Pending,
    /// Round is accepting policy contributions.
    InProgress,
    /// Policies have been merged; round is complete.
    Completed,
    /// Round could not be completed due to an error.
    Failed,
}

/// A single policy snapshot submitted by a node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyEntry {
    /// Unique policy identifier.
    pub id: String,
    /// The node that submitted this policy.
    pub node_id: String,
    /// The type of task this policy was trained for.
    pub task_type: String,
    /// Opaque policy data (JSON blob).
    pub policy_data: String,
    /// Average reward achieved by this policy.
    pub reward_avg: f64,
    /// Number of training samples used to produce this policy.
    pub sample_count: u64,
    /// Timestamp (ms since epoch) when this entry was created.
    pub created_ms: u64,
    /// Timestamp (ms since epoch) when this entry was last updated.
    pub updated_ms: u64,
}

/// A distillation round that merges contributed policies into a single
/// merged policy via reward-weighted averaging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistillationRound {
    /// Unique round identifier.
    pub id: String,
    /// Sequential round number.
    pub round_number: u64,
    /// Timestamp (ms since epoch) when the round started.
    pub start_ms: u64,
    /// Timestamp (ms since epoch) when the round was completed.
    pub completed_ms: u64,
    /// Number of unique contributor nodes.
    pub contributor_count: u32,
    /// Policy IDs that have been contributed to this round.
    pub contributed_policy_ids: Vec<String>,
    /// The merged policy (JSON string) produced after completion.
    pub merged_policy: Option<String>,
    /// Current status of the round.
    pub status: DistillationStatus,
}

/// Configuration for the federated RL engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederatedConfig {
    /// Minimum number of contributors required to complete a round.
    pub min_contributors: u32,
    /// Minimum interval (ms) between merge rounds.
    pub merge_interval_ms: u64,
    /// Maximum number of policies to retain in the store.
    pub max_policies: usize,
    /// Minimum total samples across contributors to allow a merge.
    pub min_samples_for_merge: u64,
}

impl Default for FederatedConfig {
    fn default() -> Self {
        Self {
            min_contributors: 2,
            merge_interval_ms: 60_000,
            max_policies: 1000,
            min_samples_for_merge: 100,
        }
    }
}

/// Runtime profile / metrics snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederatedProfile {
    /// Total number of policies ever submitted.
    pub total_policies: usize,
    /// Total number of distillation rounds started.
    pub total_rounds: usize,
    /// Number of rounds that completed successfully.
    pub completed_rounds: usize,
    /// Set of distinct nodes that have contributed at least one policy.
    pub contributor_nodes: Vec<String>,
    /// Timestamp (ms since epoch) of the last completed merge.
    pub last_merge_ms: u64,
    /// Average reward across all stored policies.
    pub avg_reward_across_policies: f64,
}

// ── Internal state ──────────────────────────────────────────────────────────

/// All mutable state for the federated RL engine.
struct Inner {
    /// Policy store keyed by policy id.
    policies: HashMap<String, PolicyEntry>,
    /// Distillation rounds keyed by round id.
    rounds: HashMap<String, DistillationRound>,
    /// Monotonically increasing round counter.
    next_round_number: u64,
    /// Timestamp of the last completed merge.
    last_merge_ms: u64,
    /// Configuration.
    config: FederatedConfig,
}

// ── FederatedRL engine ──────────────────────────────────────────────────────

/// Thread-safe federated reinforcement learning engine.
///
/// Manages cross-node policy submission, distillation round orchestration,
/// and reward-weighted policy merging.
#[derive(Clone)]
pub struct FederatedRL {
    inner: Arc<Mutex<Inner>>,
}

impl FederatedRL {
    /// Create a new `FederatedRL` engine with the given configuration.
    pub fn new(config: FederatedConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                policies: HashMap::new(),
                rounds: HashMap::new(),
                next_round_number: 1,
                last_merge_ms: 0,
                config,
            })),
        }
    }

    // ── Policy management ─────────────────────────────────────────────────

    /// Submit a local policy snapshot.
    ///
    /// Returns the generated policy id.
    pub fn submit_policy(
        &self,
        node_id: String,
        task_type: String,
        policy_data: String,
        reward_avg: f64,
        sample_count: u64,
    ) -> String {
        let mut inner = lock_guard(&self.inner);
        let id = generate_policy_id();
        let ts = now_ms();

        // Evict oldest policies if capacity exceeded.
        if inner.policies.len() >= inner.config.max_policies {
            let oldest_id = inner
                .policies
                .iter()
                .min_by_key(|(_, p)| p.created_ms)
                .map(|(k, _)| k.clone());
            if let Some(k) = oldest_id {
                inner.policies.remove(&k);
            }
        }

        let entry = PolicyEntry {
            id: id.clone(),
            node_id,
            task_type,
            policy_data,
            reward_avg,
            sample_count,
            created_ms: ts,
            updated_ms: ts,
        };
        inner.policies.insert(id.clone(), entry);
        id
    }

    /// Get policy details by id.
    pub fn get_policy(&self, id: &str) -> Result<PolicyEntry> {
        let inner = lock_guard(&self.inner);
        inner
            .policies
            .get(id)
            .cloned()
            .ok_or_else(|| FederatedError::PolicyNotFound(id.to_string()))
    }

    /// List policies, optionally filtered by task type.
    ///
    /// When `task_type_filter` is `None`, all policies are returned.
    pub fn list_policies(&self, task_type_filter: Option<&str>) -> Vec<PolicyEntry> {
        let inner = lock_guard(&self.inner);
        match task_type_filter {
            Some(tt) => inner
                .policies
                .values()
                .filter(|p| p.task_type == tt)
                .cloned()
                .collect(),
            None => inner.policies.values().cloned().collect(),
        }
    }

    /// Get the policy with the highest average reward for a given task type.
    pub fn best_policy(&self, task_type: &str) -> Option<PolicyEntry> {
        let inner = lock_guard(&self.inner);
        inner
            .policies
            .values()
            .filter(|p| p.task_type == task_type)
            .max_by(|a, b| a.reward_avg.total_cmp(&b.reward_avg))
            .cloned()
    }

    // ── Distillation rounds ───────────────────────────────────────────────

    /// Start a new distillation round.
    ///
    /// Returns the generated round id. The round begins in `Pending` status.
    pub fn start_distillation_round(&self) -> String {
        let mut inner = lock_guard(&self.inner);
        let id = generate_round_id();
        let rn = inner.next_round_number;
        inner.next_round_number += 1;

        let round = DistillationRound {
            id: id.clone(),
            round_number: rn,
            start_ms: now_ms(),
            completed_ms: 0,
            contributor_count: 0,
            contributed_policy_ids: Vec::new(),
            merged_policy: None,
            status: DistillationStatus::Pending,
        };
        inner.rounds.insert(id.clone(), round);
        id
    }

    /// Contribute a policy to an active round.
    ///
    /// The round must be in `Pending` or `InProgress` status.
    /// The policy must exist and must not have been contributed to this round
    /// already.
    pub fn contribute_to_round(&self, round_id: &str, policy_id: &str) -> Result<()> {
        let mut inner = lock_guard(&self.inner);

        // Validate the policy exists.
        if !inner.policies.contains_key(policy_id) {
            return Err(FederatedError::PolicyNotFound(policy_id.to_string()));
        }

        let round = inner
            .rounds
            .get_mut(round_id)
            .ok_or_else(|| FederatedError::RoundNotFound(round_id.to_string()))?;

        // Round must be Pending or InProgress.
        if round.status != DistillationStatus::Pending
            && round.status != DistillationStatus::InProgress
        {
            return Err(FederatedError::RoundNotActive(round_id.to_string()));
        }

        // Policy must not be a duplicate for this round.
        if round
            .contributed_policy_ids
            .contains(&policy_id.to_string())
        {
            return Err(FederatedError::PolicyAlreadyContributed(
                policy_id.to_string(),
            ));
        }

        // Transition from Pending to InProgress on first contribution.
        if round.status == DistillationStatus::Pending {
            round.status = DistillationStatus::InProgress;
        }

        round.contributed_policy_ids.push(policy_id.to_string());
        round.contributor_count += 1;

        Ok(())
    }

    /// Complete a distillation round by merging contributed policies.
    ///
    /// Uses reward-weighted averaging to produce a merged policy.
    /// The merged policy is stored as a JSON string of the form:
    /// ```json
    /// {"merged_policy": "...", "weights": {...}, "contributor_count": N}
    /// ```
    ///
    /// # Requirements
    ///
    /// - At least `config.min_contributors` must have contributed.
    /// - Total sample count across contributors must meet
    ///   `config.min_samples_for_merge`.
    pub fn complete_round(&self, round_id: &str) -> Result<DistillationRound> {
        let mut inner = lock_guard(&self.inner);

        // Step 1 — resolve the round and validate its status.
        let has_pending_round = inner.rounds.contains_key(round_id);
        if !has_pending_round {
            return Err(FederatedError::RoundNotFound(round_id.to_string()));
        }

        {
            let round = inner
                .rounds
                .get(round_id)
                .expect("round must exist because contains_key check passed above");
            if round.status != DistillationStatus::InProgress
                && round.status != DistillationStatus::Pending
            {
                return Err(FederatedError::RoundNotActive(round_id.to_string()));
            }

            if round.contributor_count < inner.config.min_contributors {
                return Err(FederatedError::InsufficientContributors {
                    have: round.contributor_count,
                    need: inner.config.min_contributors,
                });
            }
        }

        // Step 2 — collect contributed policies and validate sample threshold.
        let contributed_ids: Vec<String>;
        let contributor_count: u32;

        {
            let round = inner
                .rounds
                .get(round_id)
                .expect("round must exist because contains_key check passed above");
            contributed_ids = round.contributed_policy_ids.clone();
            contributor_count = round.contributor_count;
        }

        let mut total_samples: u64 = 0;
        let mut policies_to_merge: Vec<PolicyEntry> = Vec::new();

        for pid in &contributed_ids {
            match inner.policies.get(pid) {
                Some(p) => {
                    total_samples += p.sample_count;
                    policies_to_merge.push(p.clone());
                }
                None => {
                    return Err(FederatedError::MissingPolicyData(pid.clone()));
                }
            }
        }

        if total_samples < inner.config.min_samples_for_merge {
            return Err(FederatedError::InsufficientContributors {
                have: contributor_count,
                need: inner.config.min_contributors,
            });
        }

        // Step 3 — reward-weighted averaging.
        let total_reward: f64 = policies_to_merge
            .iter()
            .map(|p| p.reward_avg * p.sample_count as f64)
            .sum();
        let weighted_avg_reward = total_reward / total_samples.max(1) as f64;

        let weights: HashMap<String, f64> = policies_to_merge
            .iter()
            .map(|p| {
                let weight = p.sample_count as f64 / total_samples.max(1) as f64;
                (p.id.clone(), weight)
            })
            .collect();

        let merged_payload = serde_json::json!({
            "weighted_avg_reward": weighted_avg_reward,
            "total_samples": total_samples,
            "contributor_count": contributor_count,
            "weights": weights,
            "policies": policies_to_merge.iter().map(|p| serde_json::json!({
                "id": p.id,
                "node_id": p.node_id,
                "reward_avg": p.reward_avg,
                "sample_count": p.sample_count,
            })).collect::<Vec<_>>(),
        });

        let merged_policy =
            serde_json::to_string(&merged_payload).unwrap_or_else(|_| "{}".to_string());

        // Step 4 — update the round record (borrow scope ensures no aliasing).
        let completed_ms = now_ms();
        {
            let round = inner
                .rounds
                .get_mut(round_id)
                .expect("round must exist because contains_key check passed above");
            round.status = DistillationStatus::Completed;
            round.completed_ms = completed_ms;
            round.merged_policy = Some(merged_policy);
        }

        inner.last_merge_ms = completed_ms;

        Ok(inner
            .rounds
            .get(round_id)
            .expect("round must exist because it was just updated above")
            .clone())
    }

    // ── Round querying ────────────────────────────────────────────────────

    /// Get distillation round details by id.
    pub fn get_round(&self, id: &str) -> Result<DistillationRound> {
        let inner = lock_guard(&self.inner);
        inner
            .rounds
            .get(id)
            .cloned()
            .ok_or_else(|| FederatedError::RoundNotFound(id.to_string()))
    }

    /// List all distillation rounds, ordered by round number (ascending).
    pub fn list_rounds(&self) -> Vec<DistillationRound> {
        let inner = lock_guard(&self.inner);
        let mut rounds: Vec<DistillationRound> = inner.rounds.values().cloned().collect();
        rounds.sort_by_key(|r| r.round_number);
        rounds
    }

    // ── Profile ───────────────────────────────────────────────────────────

    /// Return a snapshot of runtime metrics.
    pub fn profile(&self) -> FederatedProfile {
        let inner = lock_guard(&self.inner);

        let total_policies = inner.policies.len();
        let total_rounds = inner.rounds.len();
        let completed_rounds = inner
            .rounds
            .values()
            .filter(|r| r.status == DistillationStatus::Completed)
            .count();

        let mut contributor_nodes: Vec<String> = {
            let mut nodes: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
            for p in inner.policies.values() {
                nodes.insert(p.node_id.as_str());
            }
            nodes.into_iter().map(String::from).collect()
        };
        contributor_nodes.sort();

        let avg_reward_across_policies = if total_policies == 0 {
            0.0
        } else {
            let sum: f64 = inner.policies.values().map(|p| p.reward_avg).sum();
            sum / total_policies as f64
        };

        FederatedProfile {
            total_policies,
            total_rounds,
            completed_rounds,
            contributor_nodes,
            last_merge_ms: inner.last_merge_ms,
            avg_reward_across_policies,
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> FederatedConfig {
        FederatedConfig {
            min_contributors: 2,
            merge_interval_ms: 60_000,
            max_policies: 100,
            min_samples_for_merge: 10,
        }
    }

    // ── Test 1: new engine is empty ───────────────────────────────────────

    #[test]
    fn test_new_federated_rl_empty() {
        let frl = FederatedRL::new(default_config());
        let profile = frl.profile();
        assert_eq!(profile.total_policies, 0);
        assert_eq!(profile.total_rounds, 0);
        assert_eq!(profile.completed_rounds, 0);
        assert!(profile.contributor_nodes.is_empty());
        assert_eq!(profile.avg_reward_across_policies, 0.0);
    }

    // ── Test 2: submit a policy ────────────────────────────────────────────

    #[test]
    fn test_submit_policy() {
        let frl = FederatedRL::new(default_config());
        let id = frl.submit_policy(
            "node-1".into(),
            "classification".into(),
            "{\"weights\": [0.1, 0.2]}".into(),
            0.85,
            50,
        );
        let policy = frl.get_policy(&id).unwrap();
        assert_eq!(policy.node_id, "node-1");
        assert_eq!(policy.task_type, "classification");
        assert!((policy.reward_avg - 0.85).abs() < 1e-9);
        assert_eq!(policy.sample_count, 50);
        assert!(policy.created_ms > 0);
        assert_eq!(policy.updated_ms, policy.created_ms);
    }

    // ── Test 3: list policies by task type ─────────────────────────────────

    #[test]
    fn test_list_policies_by_task_type() {
        let frl = FederatedRL::new(default_config());
        frl.submit_policy("a".into(), "type-a".into(), "{}".into(), 0.9, 10);
        frl.submit_policy("b".into(), "type-b".into(), "{}".into(), 0.8, 20);
        frl.submit_policy("c".into(), "type-a".into(), "{}".into(), 0.7, 30);

        let type_a = frl.list_policies(Some("type-a"));
        assert_eq!(type_a.len(), 2);
        assert!(type_a.iter().all(|p| p.task_type == "type-a"));

        let all = frl.list_policies(None);
        assert_eq!(all.len(), 3);
    }

    // ── Test 4: start a distillation round ─────────────────────────────────

    #[test]
    fn test_start_distillation_round() {
        let frl = FederatedRL::new(default_config());
        let round_id = frl.start_distillation_round();
        let round = frl.get_round(&round_id).unwrap();
        assert_eq!(round.status, DistillationStatus::Pending);
        assert_eq!(round.contributor_count, 0);
        assert!(round.contributed_policy_ids.is_empty());
        assert!(round.merged_policy.is_none());
        assert_eq!(round.round_number, 1);
    }

    // ── Test 5: contribute to a round ──────────────────────────────────────

    #[test]
    fn test_contribute_to_round() {
        let frl = FederatedRL::new(default_config());
        let pid = frl.submit_policy("n1".into(), "cls".into(), "{}".into(), 0.9, 30);
        let rid = frl.start_distillation_round();

        frl.contribute_to_round(&rid, &pid).unwrap();

        let round = frl.get_round(&rid).unwrap();
        assert_eq!(round.status, DistillationStatus::InProgress);
        assert_eq!(round.contributor_count, 1);
        assert_eq!(round.contributed_policy_ids.len(), 1);
        assert_eq!(round.contributed_policy_ids[0], pid);
    }

    // ── Test 6: complete a round merges policies ───────────────────────────

    #[test]
    fn test_complete_round_merges_policies() {
        let frl = FederatedRL::new(default_config());

        let pid1 = frl.submit_policy("n1".into(), "cls".into(), "{}".into(), 0.9, 30);
        let pid2 = frl.submit_policy("n2".into(), "cls".into(), "{}".into(), 0.7, 20);

        let rid = frl.start_distillation_round();
        frl.contribute_to_round(&rid, &pid1).unwrap();
        frl.contribute_to_round(&rid, &pid2).unwrap();

        let completed = frl.complete_round(&rid).unwrap();
        assert_eq!(completed.status, DistillationStatus::Completed);
        assert!(completed.completed_ms > 0);
        assert!(completed.merged_policy.is_some());

        let merged_str = completed.merged_policy.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&merged_str).unwrap();
        // Weighted avg reward: (0.9*30 + 0.7*20) / 50 = (27 + 14) / 50 = 0.82
        let expected = (0.9 * 30.0 + 0.7 * 20.0) / 50.0;
        let actual = parsed["weighted_avg_reward"].as_f64().unwrap();
        assert!((actual - expected).abs() < 1e-9);
    }

    // ── Test 7: complete round requires min contributors ───────────────────

    #[test]
    fn test_complete_round_requires_min_contributors() {
        let frl = FederatedRL::new(FederatedConfig {
            min_contributors: 3,
            ..default_config()
        });

        let pid1 = frl.submit_policy("n1".into(), "cls".into(), "{}".into(), 0.9, 30);
        let pid2 = frl.submit_policy("n2".into(), "cls".into(), "{}".into(), 0.7, 20);

        let rid = frl.start_distillation_round();
        frl.contribute_to_round(&rid, &pid1).unwrap();
        frl.contribute_to_round(&rid, &pid2).unwrap();

        let err = frl.complete_round(&rid).unwrap_err();
        match err {
            FederatedError::InsufficientContributors { have, need } => {
                assert_eq!(have, 2);
                assert_eq!(need, 3);
            }
            other => panic!("expected InsufficientContributors, got {other:?}"),
        }
    }

    // ── Test 8: best policy ────────────────────────────────────────────────

    #[test]
    fn test_best_policy() {
        let frl = FederatedRL::new(default_config());
        frl.submit_policy("n1".into(), "cls".into(), "{}".into(), 0.5, 10);
        frl.submit_policy("n2".into(), "cls".into(), "{}".into(), 0.9, 20);
        frl.submit_policy("n3".into(), "reg".into(), "{}".into(), 0.8, 15);

        let best_cls = frl.best_policy("cls").unwrap();
        assert!((best_cls.reward_avg - 0.9).abs() < 1e-9);
        assert_eq!(best_cls.node_id, "n2");

        let best_reg = frl.best_policy("reg").unwrap();
        assert!((best_reg.reward_avg - 0.8).abs() < 1e-9);

        assert!(frl.best_policy("unknown").is_none());
    }

    // ── Test 9: profile reflects state ─────────────────────────────────────

    #[test]
    fn test_profile_reflects_state() {
        let frl = FederatedRL::new(default_config());

        let pid1 = frl.submit_policy("n1".into(), "cls".into(), "{}".into(), 0.9, 30);
        let pid2 = frl.submit_policy("n2".into(), "reg".into(), "{}".into(), 0.7, 20);

        let profile_before = frl.profile();
        assert_eq!(profile_before.total_policies, 2);
        assert_eq!(profile_before.total_rounds, 0);
        assert_eq!(profile_before.completed_rounds, 0);
        assert_eq!(profile_before.contributor_nodes.len(), 2);
        assert!((profile_before.avg_reward_across_policies - 0.8).abs() < 1e-9);

        let rid = frl.start_distillation_round();
        frl.contribute_to_round(&rid, &pid1).unwrap();
        frl.contribute_to_round(&rid, &pid2).unwrap();
        let _ = frl.complete_round(&rid).unwrap();

        let profile_after = frl.profile();
        assert_eq!(profile_after.total_policies, 2);
        assert_eq!(profile_after.total_rounds, 1);
        assert_eq!(profile_after.completed_rounds, 1);
        assert!(profile_after.last_merge_ms > 0);
    }

    // ── Test 10: get nonexistent policy fails ──────────────────────────────

    #[test]
    fn test_get_nonexistent_policy_fails() {
        let frl = FederatedRL::new(default_config());
        let err = frl.get_policy("nonexistent").unwrap_err();
        match err {
            FederatedError::PolicyNotFound(id) => assert_eq!(id, "nonexistent"),
            other => panic!("expected PolicyNotFound, got {other:?}"),
        }
    }

    // ── Bonus test: round lifecycle (Pending → InProgress → Completed) ─────

    #[test]
    fn test_round_lifecycle() {
        let frl = FederatedRL::new(default_config());

        // Phase 1: new round is Pending
        let rid = frl.start_distillation_round();
        assert_eq!(
            frl.get_round(&rid).unwrap().status,
            DistillationStatus::Pending
        );

        // Phase 2: first contribution transitions to InProgress
        let pid1 = frl.submit_policy("n1".into(), "cls".into(), "{}".into(), 0.9, 30);
        frl.contribute_to_round(&rid, &pid1).unwrap();
        assert_eq!(
            frl.get_round(&rid).unwrap().status,
            DistillationStatus::InProgress
        );

        // Phase 3: second contribution stays InProgress
        let pid2 = frl.submit_policy("n2".into(), "cls".into(), "{}".into(), 0.7, 20);
        frl.contribute_to_round(&rid, &pid2).unwrap();
        assert_eq!(
            frl.get_round(&rid).unwrap().status,
            DistillationStatus::InProgress
        );

        // Phase 4: complete transitions to Completed
        frl.complete_round(&rid).unwrap();
        assert_eq!(
            frl.get_round(&rid).unwrap().status,
            DistillationStatus::Completed
        );

        // Phase 5: list rounds returns it
        let rounds = frl.list_rounds();
        assert_eq!(rounds.len(), 1);
        assert_eq!(rounds[0].id, rid);
    }

    // ── Bonus test: duplicate contribution rejected ────────────────────────

    #[test]
    fn test_duplicate_contribution_rejected() {
        let frl = FederatedRL::new(default_config());
        let pid = frl.submit_policy("n1".into(), "cls".into(), "{}".into(), 0.9, 30);
        let rid = frl.start_distillation_round();

        frl.contribute_to_round(&rid, &pid).unwrap();
        let err = frl.contribute_to_round(&rid, &pid).unwrap_err();
        match err {
            FederatedError::PolicyAlreadyContributed(id) => assert_eq!(id, pid),
            other => panic!("expected PolicyAlreadyContributed, got {other:?}"),
        }
    }

    // ── Bonus test: completing an already completed round fails ────────────

    #[test]
    fn test_complete_already_completed_round_fails() {
        let frl = FederatedRL::new(default_config());
        let pid1 = frl.submit_policy("n1".into(), "cls".into(), "{}".into(), 0.9, 30);
        let pid2 = frl.submit_policy("n2".into(), "cls".into(), "{}".into(), 0.7, 20);
        let rid = frl.start_distillation_round();
        frl.contribute_to_round(&rid, &pid1).unwrap();
        frl.contribute_to_round(&rid, &pid2).unwrap();
        frl.complete_round(&rid).unwrap();

        let err = frl.complete_round(&rid).unwrap_err();
        match err {
            FederatedError::RoundNotActive(id) => assert_eq!(id, rid),
            other => panic!("expected RoundNotActive, got {other:?}"),
        }
    }
}
