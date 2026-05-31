//! F-GAP-19: Federated Reinforcement Learning
//!
//! This module implements a federated learning system that enables multiple
//! reinforcement learning agents to collaboratively train a shared model
//! without sharing raw local data. Clients submit local model weights (e.g.
//! Q-table snapshots and policy parameters), and the central coordinator
//! aggregates them using configurable strategies (FedAvg, FedWeighted,
//! FedMedian).
//!
//! Thread-safety is provided via `Arc<Mutex<FederatedLearning>>`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

// ── Core data types ───────────────────────────────────────────────────────

/// Tracks the state of a single federated client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederatedClientState {
    pub client_id: String,
    pub weight: f64,
    pub last_contribution_ms: u64,
    pub contribution_count: u64,
    pub avg_improvement: f64,
}

/// Snapshot of a client's local model weights.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelWeights {
    pub q_table_snapshot: HashMap<String, f64>,
    pub policy_params: HashMap<String, f64>,
    pub version: u64,
}

/// Result of a single federated aggregation round.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederatedRound {
    pub round_id: u64,
    pub clients_participated: Vec<String>,
    pub global_weights: ModelWeights,
    pub aggregated_at_ms: u64,
    pub improvement_score: f64,
}

/// Configuration for the federated learning system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederatedConfig {
    pub min_clients: usize,
    pub round_interval_ms: u64,
    pub aggregation_method: AggregationMethod,
    pub contribution_weight: ContributionWeight,
}

impl Default for FederatedConfig {
    fn default() -> Self {
        Self {
            min_clients: 2,
            round_interval_ms: 3_600_000, // 1 hour
            aggregation_method: AggregationMethod::FedAvg,
            contribution_weight: ContributionWeight::Equal,
        }
    }
}

/// Strategy for aggregating client weights.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum AggregationMethod {
    FedAvg,
    FedWeighted,
    FedMedian,
}

/// How to weigh each client's contribution during aggregation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ContributionWeight {
    Equal,
    ByDataSize,
    ByPerformance,
}

/// High-level profile snapshot of the federated learning system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederatedProfile {
    pub total_clients: usize,
    pub total_rounds: u64,
    pub avg_improvement: f64,
    pub last_round_ms: u64,
}

// ── FederatedLearning ─────────────────────────────────────────────────────

/// Central coordinator for federated reinforcement learning.
///
/// Thread-safe wrapper around inner state. Use `Arc<Mutex<FederatedLearning>>`
/// to share an instance across tasks or agents.
#[derive(Debug)]
pub struct FederatedLearning {
    config: FederatedConfig,
    clients: HashMap<String, FederatedClientState>,
    pending_weights: HashMap<String, (ModelWeights, f64)>,
    global_weights: Option<ModelWeights>,
    round_counter: u64,
    total_improvement_sum: f64,
    /// Maximum number of registered clients before FIFO eviction
    max_clients: usize,
    /// Maximum number of pending weight submissions before FIFO eviction
    max_pending: usize,
}

impl FederatedLearning {
    /// Create a new federated learning coordinator with the given configuration.
    pub fn new(config: FederatedConfig) -> Self {
        Self {
            config,
            clients: HashMap::new(),
            pending_weights: HashMap::new(),
            global_weights: None,
            round_counter: 0,
            total_improvement_sum: 0.0,
            max_clients: 100,
            max_pending: 100,
        }
    }

    /// Register a new client with an optional weight (e.g. relative compute
    /// capacity or data volume). Returns an error if the client is already
    /// registered.
    pub fn register_client(&mut self, client_id: &str, weight: f64) -> Result<()> {
        if self.clients.contains_key(client_id) {
            bail!("client '{}' is already registered", client_id);
        }
        // Evict oldest client when at capacity.
        if self.clients.len() >= self.max_clients {
            if let Some(oldest) = self.clients.keys().next().cloned() {
                self.clients.remove(&oldest);
                self.pending_weights.remove(&oldest);
            }
        }
        self.clients.insert(
            client_id.to_string(),
            FederatedClientState {
                client_id: client_id.to_string(),
                weight,
                last_contribution_ms: 0,
                contribution_count: 0,
                avg_improvement: 0.0,
            },
        );
        Ok(())
    }

    /// Unregister a client. Returns an error if the client is not found.
    pub fn unregister_client(&mut self, client_id: &str) -> Result<()> {
        if self.clients.remove(client_id).is_none() {
            bail!("client '{}' is not registered", client_id);
        }
        // Also clean up any pending weights for this client.
        self.pending_weights.remove(client_id);
        Ok(())
    }

    /// Accept local model weights from a client for the next aggregation round.
    /// The `improvement` parameter is a scalar (e.g. delta in success rate)
    /// that the client observed since its last contribution.
    pub fn submit_local_weights(
        &mut self,
        client_id: &str,
        weights: ModelWeights,
        improvement: f64,
    ) -> Result<()> {
        let state = self
            .clients
            .get_mut(client_id)
            .with_context(|| format!("client '{}' is not registered", client_id))?;

        state.last_contribution_ms = elapsed_ms();
        state.contribution_count += 1;

        // Update running average of improvement.
        let n = state.contribution_count as f64;
        state.avg_improvement = state.avg_improvement * ((n - 1.0) / n) + improvement / n;

        // Evict the oldest pending weight entry when at capacity.
        if self.pending_weights.len() >= self.max_pending {
            if let Some(oldest) = self.pending_weights.keys().next().cloned() {
                self.pending_weights.remove(&oldest);
            }
        }

        self.pending_weights
            .insert(client_id.to_string(), (weights, improvement));
        Ok(())
    }

    /// Aggregate all pending client weights into a new global model using the
    /// configured aggregation method. Returns the resulting `FederatedRound`.
    ///
    /// # Errors
    ///
    /// Returns an error if fewer than `min_clients` have submitted weights.
    pub fn aggregate_round(&mut self) -> Result<FederatedRound> {
        let num_clients = self.pending_weights.len();
        if num_clients < self.config.min_clients {
            bail!(
                "insufficient clients: have {}, need {}",
                num_clients,
                self.config.min_clients
            );
        }

        let aggregated = match self.config.aggregation_method {
            AggregationMethod::FedAvg => self.aggregate_fed_avg(),
            AggregationMethod::FedWeighted => self.aggregate_fed_weighted(),
            AggregationMethod::FedMedian => self.aggregate_fed_median(),
        };

        let improvement_score = self
            .clients
            .values()
            .map(|c| c.avg_improvement)
            .sum::<f64>()
            / self.clients.len().max(1) as f64;

        self.round_counter += 1;
        self.total_improvement_sum += improvement_score;

        let clients_participated: Vec<String> = self.pending_weights.keys().cloned().collect();

        let round = FederatedRound {
            round_id: self.round_counter,
            clients_participated,
            global_weights: aggregated.clone(),
            aggregated_at_ms: elapsed_ms(),
            improvement_score,
        };

        self.global_weights = Some(aggregated);
        self.pending_weights.clear();
        Ok(round)
    }

    /// Return the current global weights, if any have been produced.
    pub fn get_global_weights(&self) -> Option<ModelWeights> {
        self.global_weights.clone()
    }

    /// Produce a local policy for a given client by combining the global model
    /// with the client's most recently submitted local weights.
    ///
    /// The local policy uses the global Q-table as a base and overlays any
    /// entries from the client's last submission that have higher values.
    pub fn distill_to_local_policy(&self, client_id: &str) -> Result<HashMap<String, f64>> {
        let _state = self
            .clients
            .get(client_id)
            .with_context(|| format!("client '{}' is not registered", client_id))?;

        let mut local_policy = HashMap::new();

        // Start with the global weights' Q-table (if available).
        if let Some(ref global) = self.global_weights {
            for (k, v) in &global.q_table_snapshot {
                local_policy.insert(k.clone(), *v);
            }
            for (k, v) in &global.policy_params {
                local_policy.insert(k.clone(), *v);
            }
        }

        // Overlay any entries from the client's last submitted weights where
        // the local value is higher.
        if let Some((local_w, _)) = self.pending_weights.get(client_id) {
            for (k, v) in &local_w.q_table_snapshot {
                let entry = local_policy.entry(k.clone()).or_insert(0.0);
                if *v > *entry {
                    *entry = *v;
                }
            }
            for (k, v) in &local_w.policy_params {
                let entry = local_policy.entry(k.clone()).or_insert(0.0);
                if *v > *entry {
                    *entry = *v;
                }
            }
        }

        Ok(local_policy)
    }

    /// Return a high-level profile snapshot of the federated learning system.
    pub fn profile(&self) -> FederatedProfile {
        FederatedProfile {
            total_clients: self.clients.len(),
            total_rounds: self.round_counter,
            avg_improvement: if self.round_counter > 0 {
                self.total_improvement_sum / self.round_counter as f64
            } else {
                0.0
            },
            last_round_ms: self
                .global_weights
                .as_ref()
                .map(|_| elapsed_ms())
                .unwrap_or(0),
        }
    }

    // ── Private helpers ───────────────────────────────────────────────────

    /// Simple average: for each parameter, average across all clients equally.
    fn aggregate_fed_avg(&self) -> ModelWeights {
        let n = self.pending_weights.len() as f64;
        let mut q_avg: HashMap<String, f64> = HashMap::new();
        let mut p_avg: HashMap<String, f64> = HashMap::new();

        for (w, _) in self.pending_weights.values() {
            for (k, v) in &w.q_table_snapshot {
                *q_avg.entry(k.clone()).or_insert(0.0) += v / n;
            }
            for (k, v) in &w.policy_params {
                *p_avg.entry(k.clone()).or_insert(0.0) += v / n;
            }
        }

        let max_version = self
            .pending_weights
            .values()
            .map(|(w, _)| w.version)
            .max()
            .unwrap_or(0);

        ModelWeights {
            q_table_snapshot: q_avg,
            policy_params: p_avg,
            version: max_version,
        }
    }

    /// Weighted average: weight each client's contribution by its registered
    /// `weight` field (normalised to sum to 1).
    fn aggregate_fed_weighted(&self) -> ModelWeights {
        let total_weight: f64 = self
            .pending_weights
            .keys()
            .filter_map(|id| self.clients.get(id))
            .map(|s| s.weight)
            .sum();

        let total_weight = if total_weight <= 0.0 {
            1.0
        } else {
            total_weight
        };

        let mut q_avg: HashMap<String, f64> = HashMap::new();
        let mut p_avg: HashMap<String, f64> = HashMap::new();

        for (id, (w, _)) in &self.pending_weights {
            let cw = self
                .clients
                .get(id)
                .map(|s| s.weight / total_weight)
                .unwrap_or(0.0);

            for (k, v) in &w.q_table_snapshot {
                *q_avg.entry(k.clone()).or_insert(0.0) += v * cw;
            }
            for (k, v) in &w.policy_params {
                *p_avg.entry(k.clone()).or_insert(0.0) += v * cw;
            }
        }

        let max_version = self
            .pending_weights
            .values()
            .map(|(w, _)| w.version)
            .max()
            .unwrap_or(0);

        ModelWeights {
            q_table_snapshot: q_avg,
            policy_params: p_avg,
            version: max_version,
        }
    }

    /// Median aggregation: for each parameter, take the median value across
    /// all clients.
    fn aggregate_fed_median(&self) -> ModelWeights {
        use std::collections::BTreeSet;

        // Collect all keys first.
        let mut all_q_keys: BTreeSet<String> = BTreeSet::new();
        let mut all_p_keys: BTreeSet<String> = BTreeSet::new();
        for (w, _) in self.pending_weights.values() {
            for k in w.q_table_snapshot.keys() {
                all_q_keys.insert(k.clone());
            }
            for k in w.policy_params.keys() {
                all_p_keys.insert(k.clone());
            }
        }

        let n = self.pending_weights.len();

        fn median_of_values(values: &mut [f64]) -> f64 {
            values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let mid = values.len() / 2;
            if values.len().is_multiple_of(2) {
                (values[mid - 1] + values[mid]) / 2.0
            } else {
                values[mid]
            }
        }

        let mut q_median: HashMap<String, f64> = HashMap::new();
        for key in &all_q_keys {
            let mut vals: Vec<f64> = self
                .pending_weights
                .values()
                .filter_map(|(w, _)| w.q_table_snapshot.get(key))
                .copied()
                .collect();
            if vals.len() < n {
                // Pad with 0.0 for clients that don't have this key.
                vals.resize(n, 0.0);
            }
            q_median.insert(key.clone(), median_of_values(&mut vals));
        }

        let mut p_median: HashMap<String, f64> = HashMap::new();
        for key in &all_p_keys {
            let mut vals: Vec<f64> = self
                .pending_weights
                .values()
                .filter_map(|(w, _)| w.policy_params.get(key))
                .copied()
                .collect();
            if vals.len() < n {
                vals.resize(n, 0.0);
            }
            p_median.insert(key.clone(), median_of_values(&mut vals));
        }

        let max_version = self
            .pending_weights
            .values()
            .map(|(w, _)| w.version)
            .max()
            .unwrap_or(0);

        ModelWeights {
            q_table_snapshot: q_median,
            policy_params: p_median,
            version: max_version,
        }
    }
}

// ── Utility ───────────────────────────────────────────────────────────────

/// Returns the current system timestamp in milliseconds since the Unix epoch.
fn elapsed_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── Thread-safe wrapper type ──────────────────────────────────────────────

/// A thread-safe handle to a `FederatedLearning` instance, intended for
/// sharing across agents or async tasks.
pub type SharedFederatedLearning = Arc<Mutex<FederatedLearning>>;

// =========================================================================
// FederatedRL — cross-node policy distillation (from federated_rl.rs)
// =========================================================================
//
// Manages policy submission, distillation round orchestration, and
// reward-weighted policy merging. Thread-safe via Arc<Mutex<Inner>>.
//
// F-GAP-19: Federated Reinforcement Learning (BLUE38)

use std::sync::atomic::{AtomicU64, Ordering};

static FRL_POLICY_ID_COUNTER: AtomicU64 = AtomicU64::new(1);
static FRL_ROUND_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

fn frl_generate_policy_id() -> String {
    let n = FRL_POLICY_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("policy-{}", n)
}

fn frl_generate_round_id() -> String {
    let n = FRL_ROUND_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("round-{}", n)
}

/// Errors that can occur during FederatedRL operations.
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
            Self::PolicyNotFound(id) => write!(f, "policy not found: {}", id),
            Self::RoundNotFound(id) => write!(f, "round not found: {}", id),
            Self::RoundNotActive(id) => write!(f, "round not active: {}", id),
            Self::PolicyAlreadyContributed(id) => {
                write!(f, "policy already contributed: {}", id)
            }
            Self::InsufficientContributors { have, need } => {
                write!(f, "insufficient contributors: have {}, need {}", have, need)
            }
            Self::MissingPolicyData(id) => write!(f, "missing policy data: {}", id),
        }
    }
}

impl std::error::Error for FederatedError {}

/// Convenience result alias for FederatedRL operations.
pub type FederatedResult<T> = std::result::Result<T, FederatedError>;

/// Status of a distillation round.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

/// A single policy entry submitted to the FederatedRL system.
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
    /// Number of training samples used.
    pub sample_count: u64,
    /// Timestamp (ms since epoch) when the policy was created.
    pub created_ms: u64,
    /// Timestamp (ms since epoch) when the policy was last updated.
    pub updated_ms: u64,
}

/// A single distillation round in the FederatedRL system.
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
    /// IDs of policies contributed to this round.
    pub contributed_policy_ids: Vec<String>,
    /// Merged policy data (JSON string), populated when the round is completed.
    pub merged_policy: Option<String>,
    /// Current status of the round.
    pub status: DistillationStatus,
}

/// Configuration for the FederatedRL distillation engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederatedRLConfig {
    /// Minimum number of contributors required to complete a round.
    pub min_contributors: u32,
    /// Minimum interval (ms) between merge rounds.
    pub merge_interval_ms: u64,
    /// Maximum number of policies to retain in the store.
    pub max_policies: usize,
    /// Minimum total samples across contributors to allow a merge.
    pub min_samples_for_merge: u64,
}

impl Default for FederatedRLConfig {
    fn default() -> Self {
        Self {
            min_contributors: 2,
            merge_interval_ms: 60_000,
            max_policies: 100,
            min_samples_for_merge: 1,
        }
    }
}

/// Profile snapshot of the FederatedRL system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederatedRLProfile {
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

struct FrlInner {
    policies: HashMap<String, PolicyEntry>,
    rounds: HashMap<String, DistillationRound>,
    next_round_number: u64,
    last_merge_ms: u64,
    config: FederatedRLConfig,
}

/// Cross-node policy distillation engine.
///
/// Nodes share local policy snapshots (reward + sample count), which are
/// merged via reward-weighted averaging during distillation rounds.
///
/// Thread-safe: `Arc<Mutex<FrlInner>>`
#[derive(Clone)]
pub struct FederatedRL {
    inner: Arc<Mutex<FrlInner>>,
}

impl FederatedRL {
    /// Create a new `FederatedRL` engine with the given configuration.
    pub fn new(config: FederatedRLConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(FrlInner {
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
        let mut inner = Self::inner_lock(&self.inner);
        let id = frl_generate_policy_id();
        let ts = now_millis();

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
    pub fn get_policy(&self, id: &str) -> FederatedResult<PolicyEntry> {
        let inner = Self::inner_lock(&self.inner);
        inner
            .policies
            .get(id)
            .cloned()
            .ok_or_else(|| FederatedError::PolicyNotFound(id.to_string()))
    }

    /// List policies, optionally filtered by task type.
    pub fn list_policies(&self, task_type_filter: Option<&str>) -> Vec<PolicyEntry> {
        let inner = Self::inner_lock(&self.inner);
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
        let inner = Self::inner_lock(&self.inner);
        inner
            .policies
            .values()
            .filter(|p| p.task_type == task_type)
            .max_by(|a, b| a.reward_avg.total_cmp(&b.reward_avg))
            .cloned()
    }

    // ── Distillation rounds ───────────────────────────────────────────────

    /// Start a new distillation round. Returns the generated round id.
    pub fn start_distillation_round(&self) -> String {
        let mut inner = Self::inner_lock(&self.inner);
        let id = frl_generate_round_id();
        let rn = inner.next_round_number;
        inner.next_round_number += 1;

        let round = DistillationRound {
            id: id.clone(),
            round_number: rn,
            start_ms: now_millis(),
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
    pub fn contribute_to_round(&self, round_id: &str, policy_id: &str) -> FederatedResult<()> {
        let mut inner = Self::inner_lock(&self.inner);

        if !inner.policies.contains_key(policy_id) {
            return Err(FederatedError::PolicyNotFound(policy_id.to_string()));
        }

        let round = inner
            .rounds
            .get_mut(round_id)
            .ok_or_else(|| FederatedError::RoundNotFound(round_id.to_string()))?;

        if round.status != DistillationStatus::Pending
            && round.status != DistillationStatus::InProgress
        {
            return Err(FederatedError::RoundNotActive(round_id.to_string()));
        }

        if round
            .contributed_policy_ids
            .contains(&policy_id.to_string())
        {
            return Err(FederatedError::PolicyAlreadyContributed(
                policy_id.to_string(),
            ));
        }

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
    pub fn complete_round(&self, round_id: &str) -> FederatedResult<DistillationRound> {
        let mut inner = Self::inner_lock(&self.inner);

        if !inner.rounds.contains_key(round_id) {
            return Err(FederatedError::RoundNotFound(round_id.to_string()));
        }

        {
            let round = inner.rounds.get(round_id).unwrap();
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

        let contributed_ids: Vec<String> = {
            let round = inner.rounds.get(round_id).unwrap();
            round.contributed_policy_ids.clone()
        };

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
                have: policies_to_merge.len() as u32,
                need: inner.config.min_contributors,
            });
        }

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
            "contributor_count": policies_to_merge.len(),
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

        let completed_ms = now_millis();
        {
            let round = inner.rounds.get_mut(round_id).unwrap();
            round.status = DistillationStatus::Completed;
            round.completed_ms = completed_ms;
            round.merged_policy = Some(merged_policy);
        }

        inner.last_merge_ms = completed_ms;

        Ok(inner.rounds.get(round_id).unwrap().clone())
    }

    // ── Round querying ────────────────────────────────────────────────────

    /// Get distillation round details by id.
    pub fn get_round(&self, id: &str) -> FederatedResult<DistillationRound> {
        let inner = Self::inner_lock(&self.inner);
        inner
            .rounds
            .get(id)
            .cloned()
            .ok_or_else(|| FederatedError::RoundNotFound(id.to_string()))
    }

    /// List all distillation rounds, ordered by round number (ascending).
    pub fn list_rounds(&self) -> Vec<DistillationRound> {
        let inner = Self::inner_lock(&self.inner);
        let mut rounds: Vec<DistillationRound> = inner.rounds.values().cloned().collect();
        rounds.sort_by_key(|r| r.round_number);
        rounds
    }

    // ── Profile ───────────────────────────────────────────────────────────

    /// Return a snapshot of runtime metrics.
    pub fn profile(&self) -> FederatedRLProfile {
        let inner = Self::inner_lock(&self.inner);

        let total_policies = inner.policies.len();
        let total_rounds = inner.rounds.len();
        let completed_rounds = inner
            .rounds
            .values()
            .filter(|r| r.status == DistillationStatus::Completed)
            .count();

        let mut contributor_nodes: Vec<String> = {
            let mut nodes: std::collections::BTreeSet<&str> =
                std::collections::BTreeSet::new();
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

        FederatedRLProfile {
            total_policies,
            total_rounds,
            completed_rounds,
            contributor_nodes,
            last_merge_ms: inner.last_merge_ms,
            avg_reward_across_policies,
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────────

    fn inner_lock(inner: &Arc<Mutex<FrlInner>>) -> std::sync::MutexGuard<'_, FrlInner> {
        match inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::warn!("FederatedRL mutex poisoned, recovering");
                poisoned.into_inner()
            }
        }
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod federated_rl_tests {
    use super::*;

    #[test]
    fn test_frl_new_empty() {
        let frl = FederatedRL::new(FederatedRLConfig::default());
        let profile = frl.profile();
        assert_eq!(profile.total_policies, 0);
        assert_eq!(profile.total_rounds, 0);
    }

    #[test]
    fn test_frl_submit_policy() {
        let frl = FederatedRL::new(FederatedRLConfig::default());
        let id = frl.submit_policy(
            "node1".into(),
            "test".into(),
            "data".into(),
            0.8,
            10,
        );
        assert!(id.starts_with("policy-"));
        let policy = frl.get_policy(&id).unwrap();
        assert_eq!(policy.node_id, "node1");
        assert_eq!(policy.reward_avg, 0.8);
    }

    #[test]
    fn test_frl_round_lifecycle() {
        let frl = FederatedRL::new(FederatedRLConfig {
            min_contributors: 2,
            ..Default::default()
        });

        let p1 = frl.submit_policy("node1".into(), "t".into(), "d1".into(), 0.9, 10);
        let p2 = frl.submit_policy("node2".into(), "t".into(), "d2".into(), 0.7, 20);

        let rid = frl.start_distillation_round();
        assert!(rid.starts_with("round-"));

        frl.contribute_to_round(&rid, &p1).unwrap();
        frl.contribute_to_round(&rid, &p2).unwrap();

        let round = frl.complete_round(&rid).unwrap();
        assert_eq!(round.status, DistillationStatus::Completed);
        assert!(round.merged_policy.is_some());
    }

    #[test]
    fn test_frl_insufficient_contributors() {
        let frl = FederatedRL::new(FederatedRLConfig {
            min_contributors: 3,
            ..Default::default()
        });

        let p1 = frl.submit_policy("node1".into(), "t".into(), "d1".into(), 0.5, 5);
        let p2 = frl.submit_policy("node2".into(), "t".into(), "d2".into(), 0.6, 5);

        let rid = frl.start_distillation_round();
        frl.contribute_to_round(&rid, &p1).unwrap();
        frl.contribute_to_round(&rid, &p2).unwrap();

        let err = frl.complete_round(&rid).unwrap_err();
        match err {
            FederatedError::InsufficientContributors { have, need } => {
                assert_eq!(have, 2);
                assert_eq!(need, 3);
            }
            _ => panic!("expected InsufficientContributors, got {:?}", err),
        }
    }

    #[test]
    fn test_frl_best_policy() {
        let frl = FederatedRL::new(FederatedRLConfig::default());
        frl.submit_policy("n1".into(), "t".into(), "d1".into(), 0.5, 5);
        frl.submit_policy("n2".into(), "t".into(), "d2".into(), 0.9, 5);
        frl.submit_policy("n3".into(), "t".into(), "d3".into(), 0.7, 5);

        let best = frl.best_policy("t").unwrap();
        assert_eq!(best.reward_avg, 0.9);
        assert_eq!(best.node_id, "n2");
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: create a set of simple model weights from a vector of
    // (state_action_key, value) pairs.
    fn make_weights(entries: Vec<(&str, f64)>, version: u64) -> ModelWeights {
        let mut q_table_snapshot = HashMap::new();
        let mut policy_params = HashMap::new();
        for (k, v) in entries {
            // Distribute roughly evenly: first half go to q_table, rest to
            // policy_params.
            if q_table_snapshot.len() <= policy_params.len() {
                q_table_snapshot.insert(k.to_string(), v);
            } else {
                policy_params.insert(k.to_string(), v);
            }
        }
        ModelWeights {
            q_table_snapshot,
            policy_params,
            version,
        }
    }

    // Helper: extract a single value from a flattened policy map.
    fn get_val(map: &HashMap<String, f64>, key: &str) -> f64 {
        map.get(key).copied().unwrap_or(f64::NAN)
    }

    #[test]
    fn test_new_federated_empty() {
        let config = FederatedConfig::default();
        let fl = FederatedLearning::new(config);
        assert_eq!(fl.clients.len(), 0);
        assert!(fl.global_weights.is_none());
        assert_eq!(fl.round_counter, 0);
    }

    #[test]
    fn test_register_client() {
        let mut fl = FederatedLearning::new(FederatedConfig::default());
        fl.register_client("alpha", 1.0).unwrap();
        assert_eq!(fl.clients.len(), 1);
        let state = fl.clients.get("alpha").unwrap();
        assert_eq!(state.client_id, "alpha");
        assert!((state.weight - 1.0).abs() < 1e-12);
        assert_eq!(state.contribution_count, 0);
    }

    #[test]
    fn test_register_duplicate_client_fails() {
        let mut fl = FederatedLearning::new(FederatedConfig::default());
        fl.register_client("alpha", 1.0).unwrap();
        let err = fl.register_client("alpha", 2.0).unwrap_err();
        let msg = format!("{:#}", err);
        assert!(msg.contains("already registered"), "got: {}", msg);
    }

    #[test]
    fn test_unregister_client() {
        let mut fl = FederatedLearning::new(FederatedConfig::default());
        fl.register_client("alpha", 1.0).unwrap();
        fl.register_client("beta", 1.0).unwrap();
        assert_eq!(fl.clients.len(), 2);

        fl.unregister_client("alpha").unwrap();
        assert_eq!(fl.clients.len(), 1);
        assert!(fl.clients.contains_key("beta"));
        assert!(!fl.clients.contains_key("alpha"));
    }

    #[test]
    fn test_unregister_nonexistent_client_fails() {
        let mut fl = FederatedLearning::new(FederatedConfig::default());
        let err = fl.unregister_client("ghost").unwrap_err();
        let msg = format!("{:#}", err);
        assert!(msg.contains("not registered"), "got: {}", msg);
    }

    #[test]
    fn test_submit_local_weights() {
        let mut fl = FederatedLearning::new(FederatedConfig::default());
        fl.register_client("alpha", 1.0).unwrap();

        let w = make_weights(vec![("s1_a1", 1.0), ("s1_a2", 2.0)], 1);
        fl.submit_local_weights("alpha", w, 0.15).unwrap();

        let state = fl.clients.get("alpha").unwrap();
        assert_eq!(state.contribution_count, 1);
        assert!((state.avg_improvement - 0.15).abs() < 1e-12);
        assert!(state.last_contribution_ms > 0);
    }

    #[test]
    fn test_aggregate_round_fed_avg() {
        let mut fl = FederatedLearning::new(FederatedConfig {
            min_clients: 2,
            ..Default::default()
        });

        fl.register_client("alpha", 1.0).unwrap();
        fl.register_client("beta", 1.0).unwrap();

        // alpha: Q values 10.0, 20.0; beta: Q values 30.0, 40.0
        let mut w1 = make_weights(vec![("q1", 10.0), ("q2", 20.0)], 1);
        let mut w2 = make_weights(vec![("q1", 30.0), ("q2", 40.0)], 2);
        // Force both q-table entries for the tests below.
        w1.policy_params.clear();
        w1.q_table_snapshot.insert("q2".into(), 20.0);
        w2.policy_params.clear();
        w2.q_table_snapshot.insert("q2".into(), 40.0);

        fl.submit_local_weights("alpha", w1, 0.0).unwrap();
        fl.submit_local_weights("beta", w2, 0.0).unwrap();

        let round = fl.aggregate_round().unwrap();
        assert_eq!(round.round_id, 1);
        assert_eq!(round.global_weights.version, 2);
        assert!(!round.global_weights.q_table_snapshot.is_empty());

        // FedAvg: (10+30)/2 = 20, (20+40)/2 = 30
        let q = &round.global_weights.q_table_snapshot;
        assert!(
            (get_val(q, "q1") - 20.0).abs() < 1e-10,
            "q1={}",
            get_val(q, "q1")
        );
        assert!(
            (get_val(q, "q2") - 30.0).abs() < 1e-10,
            "q2={}",
            get_val(q, "q2")
        );
    }

    #[test]
    fn test_aggregate_round_fed_weighted() {
        let mut fl = FederatedLearning::new(FederatedConfig {
            min_clients: 2,
            aggregation_method: AggregationMethod::FedWeighted,
            ..Default::default()
        });

        // alpha has weight 3.0, beta has weight 1.0 => total = 4.0
        fl.register_client("alpha", 3.0).unwrap();
        fl.register_client("beta", 1.0).unwrap();

        let mut w1 = make_weights(vec![("q1", 10.0), ("q2", 20.0)], 1);
        let mut w2 = make_weights(vec![("q1", 30.0), ("q2", 40.0)], 1);
        // Force both q-table entries for the tests below.
        w1.policy_params.clear();
        w1.q_table_snapshot.insert("q2".into(), 20.0);
        w2.policy_params.clear();
        w2.q_table_snapshot.insert("q2".into(), 40.0);

        fl.submit_local_weights("alpha", w1, 0.0).unwrap();
        fl.submit_local_weights("beta", w2, 0.0).unwrap();

        let round = fl.aggregate_round().unwrap();

        // FedWeighted: alpha weight 3/4, beta weight 1/4
        // q1 = 10*(3/4) + 30*(1/4) = 7.5 + 7.5 = 15.0
        // q2 = 20*(3/4) + 40*(1/4) = 15.0 + 10.0 = 25.0
        let q = &round.global_weights.q_table_snapshot;
        assert!(
            (get_val(q, "q1") - 15.0).abs() < 1e-10,
            "q1={}",
            get_val(q, "q1")
        );
        assert!(
            (get_val(q, "q2") - 25.0).abs() < 1e-10,
            "q2={}",
            get_val(q, "q2")
        );
    }

    #[test]
    fn test_aggregate_round_insufficient_clients() {
        let mut fl = FederatedLearning::new(FederatedConfig {
            min_clients: 2,
            ..Default::default()
        });

        fl.register_client("alpha", 1.0).unwrap();
        let w = make_weights(vec![("q1", 1.0)], 1);
        fl.submit_local_weights("alpha", w, 0.0).unwrap();

        let err = fl.aggregate_round().unwrap_err();
        let msg = format!("{:#}", err);
        // Should mention insufficient clients.
        assert!(
            msg.contains("insufficient clients") || msg.contains("min_clients"),
            "got: {}",
            msg
        );
    }

    #[test]
    fn test_distill_to_local_policy() {
        let mut fl = FederatedLearning::new(FederatedConfig {
            min_clients: 1,
            ..Default::default()
        });

        fl.register_client("alpha", 1.0).unwrap();

        // Submit and aggregate so global weights exist.
        let w1 = make_weights(vec![("g1", 100.0), ("g2", 200.0)], 1);
        fl.submit_local_weights("alpha", w1, 0.0).unwrap();
        fl.aggregate_round().unwrap();

        // Now submit a second local set with a higher value for g2.
        let w2 = make_weights(vec![("g1", 90.0), ("g2", 999.0)], 2);
        fl.submit_local_weights("alpha", w2, 0.0).unwrap();

        let policy = fl.distill_to_local_policy("alpha").unwrap();
        // g1 should keep global 100 (since 90 < 100).
        assert!(
            (get_val(&policy, "g1") - 100.0).abs() < 1e-10,
            "g1={}",
            get_val(&policy, "g1")
        );
        // g2 should use local 999 (since 999 > 200).
        assert!(
            (get_val(&policy, "g2") - 999.0).abs() < 1e-10,
            "g2={}",
            get_val(&policy, "g2")
        );
    }

    #[test]
    fn test_profile_reflects_state() {
        let mut fl = FederatedLearning::new(FederatedConfig {
            min_clients: 1,
            ..Default::default()
        });

        // Profile before any clients.
        let p = fl.profile();
        assert_eq!(p.total_clients, 0);
        assert_eq!(p.total_rounds, 0);

        fl.register_client("alpha", 1.0).unwrap();
        let w = make_weights(vec![("q1", 5.0)], 1);
        fl.submit_local_weights("alpha", w, 0.3).unwrap();
        fl.aggregate_round().unwrap();

        let p = fl.profile();
        assert_eq!(p.total_clients, 1);
        assert_eq!(p.total_rounds, 1);
        assert!((p.avg_improvement - 0.3).abs() < 1e-10);
        assert!(p.last_round_ms > 0);
    }

    #[test]
    fn test_get_global_weights_before_any_round() {
        let fl = FederatedLearning::new(FederatedConfig::default());
        assert!(fl.get_global_weights().is_none());
    }

    #[test]
    fn test_multiple_rounds_accumulate() {
        let mut fl = FederatedLearning::new(FederatedConfig {
            min_clients: 2,
            ..Default::default()
        });

        fl.register_client("alpha", 1.0).unwrap();
        fl.register_client("beta", 1.0).unwrap();

        // Round 1.
        let w1 = make_weights(vec![("q1", 10.0)], 1);
        let w2 = make_weights(vec![("q1", 30.0)], 1);
        fl.submit_local_weights("alpha", w1, 0.1).unwrap();
        fl.submit_local_weights("beta", w2, 0.2).unwrap();
        let r1 = fl.aggregate_round().unwrap();
        assert_eq!(r1.round_id, 1);
        assert!((get_val(&r1.global_weights.q_table_snapshot, "q1") - 20.0).abs() < 1e-10);

        // Round 2 with new values.
        let w1b = make_weights(vec![("q1", 50.0)], 2);
        let w2b = make_weights(vec![("q1", 70.0)], 2);
        fl.submit_local_weights("alpha", w1b, 0.3).unwrap();
        fl.submit_local_weights("beta", w2b, 0.4).unwrap();
        let r2 = fl.aggregate_round().unwrap();
        assert_eq!(r2.round_id, 2);
        assert!((get_val(&r2.global_weights.q_table_snapshot, "q1") - 60.0).abs() < 1e-10);

        // Global weights should now be from round 2.
        let gw = fl.get_global_weights().unwrap();
        assert_eq!(gw.version, 2);
        assert!((get_val(&gw.q_table_snapshot, "q1") - 60.0).abs() < 1e-10);

        // Profile reflects two rounds.
        // After round 1: alpha avg_improvement = 0.1, beta = 0.2
        //   improvement_score = (0.1 + 0.2)/2 = 0.15
        // After round 2: alpha avg_improvement = 0.1*(1/2)+0.3/2 = 0.2,
        //   beta = 0.2*(1/2)+0.4/2 = 0.3
        //   improvement_score = (0.2 + 0.3)/2 = 0.25
        // total_improvement_sum = 0.15 + 0.25 = 0.40
        // profile avg_improvement = 0.40 / 2 = 0.20
        let p = fl.profile();
        assert_eq!(p.total_rounds, 2);
        assert!(
            (p.avg_improvement - 0.20).abs() < 1e-10,
            "avg={}",
            p.avg_improvement
        );
    }

    #[test]
    fn test_submit_by_unregistered_client_fails() {
        let mut fl = FederatedLearning::new(FederatedConfig::default());
        let w = make_weights(vec![("q1", 1.0)], 1);
        let err = fl.submit_local_weights("ghost", w, 0.0).unwrap_err();
        let msg = format!("{:#}", err);
        assert!(msg.contains("not registered"), "got: {}", msg);
    }
}
