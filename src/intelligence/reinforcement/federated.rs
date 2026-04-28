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
        }
    }

    /// Register a new client with an optional weight (e.g. relative compute
    /// capacity or data volume). Returns an error if the client is already
    /// registered.
    pub fn register_client(&mut self, client_id: &str, weight: f64) -> Result<()> {
        if self.clients.contains_key(client_id) {
            bail!("client '{}' is already registered", client_id);
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
