//! ReinforcementBus — merged QLearningAgent + FederatedRL (BLUE70 §2.2.2)
//!
//! Provides a unified reinforcement learning interface combining:
//! - Q-Learning for single-node routing decisions
//! - Federated RL for cross-node policy aggregation
//!
//! Q-Learning serves as the single-node algorithm; FederatedRL components
//! are activated only when distributed mode is enabled.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single Q-table entry: (state, action) → value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QTableEntry {
    pub state: String,
    pub action: String,
    pub value: f64,
}

/// Federated RL coordinator (stub for distributed mode).
///
/// In single-node mode, this is None and Q-Learning runs in isolation.
#[derive(Debug)]
pub struct FederatedCoordinator {
    /// Node identifier in the federation.
    pub node_id: String,
    /// Peers in the federation.
    pub peers: Vec<String>,
    /// Pending sync operations.
    pending_syncs: Vec<(String, String, f64)>,
}

impl FederatedCoordinator {
    pub fn new(node_id: String) -> Self {
        Self {
            node_id,
            peers: Vec::new(),
            pending_syncs: Vec::new(),
        }
    }

    /// Schedule a Q-table update for federation sync.
    pub fn schedule_sync(&mut self, state: &str, action: &str, value: f64) {
        self.pending_syncs.push((
            state.to_string(),
            action.to_string(),
            value,
        ));
    }

    /// Number of pending syncs.
    pub fn pending_count(&self) -> usize {
        self.pending_syncs.len()
    }

    /// Drain pending syncs.
    pub fn drain_pending(&mut self) -> Vec<(String, String, f64)> {
        std::mem::take(&mut self.pending_syncs)
    }
}

/// Unified reinforcement learning bus (BLUE70 §2.2.2).
///
/// Design notes:
/// - Q-Learning runs as the default algorithm.
/// - FederatedRL coordinator is optional; only initialized for distributed mode.
/// - Single-node deployment has zero overhead from federation.
#[derive(Debug)]
pub struct ReinforcementBus {
    /// Q-table: (state, action) → value.
    q_table: HashMap<(String, String), f64>,
    /// FederatedRL coordinator (None = single-node mode).
    federated_coordinator: Option<FederatedCoordinator>,
    /// Learning rate (alpha).
    learning_rate: f64,
    /// Discount factor (gamma).
    discount_factor: f64,
    /// Exploration rate (epsilon).
    exploration_rate: f64,
    /// Total learning steps.
    total_steps: u64,
}

impl ReinforcementBus {
    /// Create a new ReinforcementBus with default hyperparameters.
    pub fn new() -> Self {
        Self {
            q_table: HashMap::new(),
            federated_coordinator: None,
            learning_rate: 0.1,
            discount_factor: 0.9,
            exploration_rate: 0.1,
            total_steps: 0,
        }
    }

    /// Set a custom learning rate.
    pub fn with_learning_rate(mut self, lr: f64) -> Self {
        self.learning_rate = lr.clamp(0.001, 1.0);
        self
    }

    /// Set a custom discount factor.
    pub fn with_discount_factor(mut self, gamma: f64) -> Self {
        self.discount_factor = gamma.clamp(0.0, 1.0);
        self
    }

    /// Set a custom exploration rate.
    pub fn with_exploration_rate(mut self, epsilon: f64) -> Self {
        self.exploration_rate = epsilon.clamp(0.0, 1.0);
        self
    }

    /// Enable federated learning with a coordinator.
    pub fn with_federation(mut self, coordinator: FederatedCoordinator) -> Self {
        self.federated_coordinator = Some(coordinator);
        self
    }

    // ── Action Selection ──────────────────────────────────────────

    /// Select the best action for a given state using the Q-table.
    ///
    /// Returns the action with the highest Q-value, or None if no actions available.
    pub fn select_action(&self, state: &str, available_actions: &[String]) -> Option<String> {
        if available_actions.is_empty() {
            return None;
        }
        // Epsilon-greedy: random action with probability exploration_rate
        if self.exploration_rate > 0.0 && fastrand::f64() < self.exploration_rate {
            let idx = fastrand::usize(..available_actions.len());
            return Some(available_actions[idx].clone());
        }
        available_actions
            .iter()
            .map(|a| {
                let value = self
                    .q_table
                    .get(&(state.to_string(), a.clone()))
                    .copied()
                    .unwrap_or(0.0);
                (a, value)
            })
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(a, _)| a.clone())
    }

    // ── Reward Recording ──────────────────────────────────────────

    /// Record a reward and update the Q-table using the Q-learning update rule:
    ///
    /// Q(s, a) = Q(s, a) + lr * [reward + gamma * max(Q(s', a')) - Q(s, a)]
    pub fn record_reward(&mut self, state: &str, action: &str, reward: f64, next_state: &str) {
        let key = (state.to_string(), action.to_string());
        let old_q = self.q_table.get(&key).copied().unwrap_or(0.0);

        // max over next state actions
        let max_next_q = self
            .q_table
            .iter()
            .filter(|((s, _), _)| s == next_state)
            .map(|(_, v)| *v)
            .fold(f64::NEG_INFINITY, f64::max);

        let max_next_q = if max_next_q == f64::NEG_INFINITY {
            0.0
        } else {
            max_next_q
        };

        let new_q =
            old_q + self.learning_rate * (reward + self.discount_factor * max_next_q - old_q);
        self.q_table.insert(key, new_q);
        self.total_steps += 1;

        // If federated, schedule sync
        if let Some(ref mut coordinator) = self.federated_coordinator {
            coordinator.schedule_sync(state, action, new_q);
        }
    }

    // ── Query ─────────────────────────────────────────────────────

    /// Get the Q-value for a specific state-action pair.
    pub fn get_q_value(&self, state: &str, action: &str) -> f64 {
        self.q_table
            .get(&(state.to_string(), action.to_string()))
            .copied()
            .unwrap_or(0.0)
    }

    /// Get the best Q-value for a state across all actions.
    pub fn best_q_value(&self, state: &str) -> f64 {
        self.q_table
            .iter()
            .filter(|((s, _), _)| s == state)
            .map(|(_, v)| *v)
            .fold(f64::NEG_INFINITY, f64::max)
            .max(0.0)
    }

    /// Get the size of the Q-table.
    pub fn table_size(&self) -> usize {
        self.q_table.len()
    }

    /// Total learning steps performed.
    pub fn total_steps(&self) -> u64 {
        self.total_steps
    }

    /// Get all Q-table entries (for snapshot/persistence).
    pub fn all_entries(&self) -> Vec<QTableEntry> {
        self.q_table
            .iter()
            .map(|((state, action), value)| QTableEntry {
                state: state.clone(),
                action: action.clone(),
                value: *value,
            })
            .collect()
    }

    /// Whether federation is enabled.
    pub fn is_federated(&self) -> bool {
        self.federated_coordinator.is_some()
    }

    /// Get a reference to the federated coordinator (if enabled).
    pub fn federated_coordinator(&self) -> Option<&FederatedCoordinator> {
        self.federated_coordinator.as_ref()
    }

    /// Get a mutable reference to the federated coordinator (if enabled).
    pub fn federated_coordinator_mut(&mut self) -> Option<&mut FederatedCoordinator> {
        self.federated_coordinator.as_mut()
    }

    /// Decay exploration rate by a factor.
    pub fn decay_exploration(&mut self, factor: f64) {
        self.exploration_rate *= factor.clamp(0.0, 1.0);
    }
}

impl Default for ReinforcementBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_bus() {
        let bus = ReinforcementBus::new();
        assert_eq!(bus.table_size(), 0);
        assert_eq!(bus.total_steps(), 0);
        assert!(!bus.is_federated());
    }

    #[test]
    fn test_select_action() {
        let bus = ReinforcementBus::with_exploration_rate(ReinforcementBus::new(), 0.0);
        let actions = vec!["code".to_string(), "research".to_string(), "review".to_string()];
        let selected = bus.select_action("state_1", &actions);
        assert!(selected.is_some());
        assert!(actions.contains(&selected.unwrap()));
    }

    #[test]
    fn test_select_action_empty() {
        let bus = ReinforcementBus::new();
        assert!(bus.select_action("state", &[]).is_none());
    }

    #[test]
    fn test_record_reward_updates_q_table() {
        let mut bus = ReinforcementBus::with_exploration_rate(ReinforcementBus::new(), 0.0);
        assert_eq!(bus.get_q_value("s1", "a1"), 0.0);

        bus.record_reward("s1", "a1", 1.0, "s2");
        // Q(s1, a1) should now be > 0
        assert!(bus.get_q_value("s1", "a1") > 0.0);
        assert_eq!(bus.total_steps(), 1);
    }

    #[test]
    fn test_record_reward_positive_reinforcement() {
        let mut bus = ReinforcementBus::with_exploration_rate(ReinforcementBus::new(), 0.0);

        // First record: s1→a1 gets reward 1.0
        bus.record_reward("s1", "a1", 1.0, "s2");
        let q1 = bus.get_q_value("s1", "a1");

        // Second record: s1→a1 gets reward 1.0 again (should increase)
        bus.record_reward("s1", "a1", 1.0, "s2");
        let q2 = bus.get_q_value("s1", "a1");

        assert!(q2 > q1, "Repeated positive rewards should increase Q-value");
    }

    #[test]
    fn test_record_reward_negative_reinforcement() {
        let mut bus = ReinforcementBus::with_exploration_rate(ReinforcementBus::new(), 0.0);

        bus.record_reward("s1", "a1", 1.0, "s2");
        let q1 = bus.get_q_value("s1", "a1");

        bus.record_reward("s1", "a1", -1.0, "s2");
        let q2 = bus.get_q_value("s1", "a1");

        assert!(q2 < q1, "Negative reward should decrease Q-value");
    }

    #[test]
    fn test_best_q_value() {
        let mut bus = ReinforcementBus::with_exploration_rate(ReinforcementBus::new(), 0.0);
        bus.record_reward("s1", "good_action", 1.0, "s2");
        bus.record_reward("s1", "bad_action", -1.0, "s2");

        assert!(bus.best_q_value("s1") >= bus.get_q_value("s1", "good_action"));
    }

    #[test]
    fn test_all_entries() {
        let mut bus = ReinforcementBus::with_exploration_rate(ReinforcementBus::new(), 0.0);
        bus.record_reward("s1", "a1", 1.0, "s2");
        bus.record_reward("s1", "a2", 0.5, "s2");

        let entries = bus.all_entries();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn test_federation_enabled() {
        let coordinator = FederatedCoordinator::new("node_1".to_string());
        let bus = ReinforcementBus::with_federation(ReinforcementBus::new(), coordinator);
        assert!(bus.is_federated());
    }

    #[test]
    fn test_federation_records_syncs() {
        let coordinator = FederatedCoordinator::new("node_1".to_string());
        let mut bus = ReinforcementBus::with_federation(ReinforcementBus::new(), coordinator);

        bus.record_reward("s1", "a1", 1.0, "s2");
        let coord = bus.federated_coordinator().unwrap();
        assert_eq!(coord.pending_count(), 1);
    }

    #[test]
    fn test_federated_coordinator_drain() {
        let mut coordinator = FederatedCoordinator::new("node_1".to_string());
        coordinator.schedule_sync("s1", "a1", 0.9);
        coordinator.schedule_sync("s2", "a2", 0.5);

        assert_eq!(coordinator.pending_count(), 2);
        let drained = coordinator.drain_pending();
        assert_eq!(drained.len(), 2);
        assert_eq!(coordinator.pending_count(), 0);
    }

    #[test]
    fn test_decay_exploration() {
        let mut bus = ReinforcementBus::new();
        assert!((bus.exploration_rate - 0.1).abs() < 0.01);
        bus.decay_exploration(0.5);
        assert!((bus.exploration_rate - 0.05).abs() < 0.01);
    }

    #[test]
    fn test_hyperparameters() {
        let bus = ReinforcementBus::new()
            .with_learning_rate(0.5)
            .with_discount_factor(0.8)
            .with_exploration_rate(0.2);
        assert!((bus.learning_rate - 0.5).abs() < 0.01);
        assert!((bus.discount_factor - 0.8).abs() < 0.01);
        assert!((bus.exploration_rate - 0.2).abs() < 0.01);
    }

    #[test]
    fn test_table_size() {
        let mut bus = ReinforcementBus::new();
        bus.record_reward("s1", "a1", 1.0, "s2");
        bus.record_reward("s1", "a2", 0.5, "s2");
        bus.record_reward("s2", "a1", 1.0, "s3");
        assert_eq!(bus.table_size(), 3);
    }
}
