//! ReinforcementBus — merged QLearningAgent + FederatedRL (BLUE70 §2.2.2)
//!
//! Provides a unified reinforcement learning interface:
//! - Q-Learning for single-node routing decisions
//!
//! Q-Learning serves as the single-node algorithm; the former FederatedRL
//! half was removed — it was never activated in single-node deployments
//! (coordinator stayed None) and had zero production consumers.

use std::collections::HashMap;

/// Unified reinforcement learning bus (BLUE70 §2.2.2).
///
/// Design notes:
/// - Q-Learning runs as the default algorithm.
/// - Single-node deployment has zero overhead from federation.
///
/// Maximum number of (state, action) entries kept in the Q-table before the
/// least-recently-updated entry is evicted. Bounds memory for long-running
/// processes where the key space (task_type × agent) grows without limit.
const MAX_Q_TABLE_ENTRIES: usize = 10_000;

#[derive(Debug)]
pub struct ReinforcementBus {
    /// Q-table: (state, action) → value.
    q_table: HashMap<(String, String), f64>,
    /// Per-state maximum Q-value, kept in sync with `q_table` so the
    /// `max(Q(s', a'))` term is O(1) instead of a full-table scan per update.
    state_max_q: HashMap<String, f64>,
    /// Monotonic update sequence per entry, used for LRU eviction.
    last_updated: HashMap<(String, String), u64>,
    update_seq: u64,
    /// Learning rate (alpha).
    learning_rate: f64,
    /// Discount factor (gamma).
    discount_factor: f64,
    /// Exploration rate (epsilon).
    exploration_rate: f64,
}

impl ReinforcementBus {
    /// Create a new ReinforcementBus with default hyperparameters.
    pub fn new() -> Self {
        Self {
            q_table: HashMap::new(),
            state_max_q: HashMap::new(),
            last_updated: HashMap::new(),
            update_seq: 0,
            learning_rate: 0.1,
            discount_factor: 0.9,
            exploration_rate: 0.1,
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

    /// Overwrite the exploration rate (used by metacognitive feedback).
    pub fn set_exploration_rate(&mut self, epsilon: f64) {
        self.exploration_rate = epsilon.clamp(0.0, 1.0);
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

        // max over next state actions — O(1) via the per-state index.
        let max_next_q = self.state_max_q.get(next_state).copied().unwrap_or(0.0);

        let new_q =
            old_q + self.learning_rate * (reward + self.discount_factor * max_next_q - old_q);

        // Bound the table: evict the least-recently-updated entry when a new
        // (state, action) pair would exceed the cap.
        if !self.q_table.contains_key(&key) && self.q_table.len() >= MAX_Q_TABLE_ENTRIES {
            self.evict_lru();
        }
        self.q_table.insert(key.clone(), new_q);
        self.update_seq += 1;
        self.last_updated.insert(key, self.update_seq);

        // Maintain the per-state maximum (only the touched state may change).
        let state_max = self.state_max_q.entry(state.to_string()).or_insert(0.0);
        if new_q > *state_max {
            *state_max = new_q;
        }
    }

    /// Evict the least-recently-updated entry (called only at capacity).
    fn evict_lru(&mut self) {
        if let Some((key, _)) = self.last_updated.iter().min_by_key(|(_, seq)| **seq) {
            let key = key.clone();
            self.q_table.remove(&key);
            self.last_updated.remove(&key);
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
    }

    #[test]
    fn test_select_action() {
        let bus = ReinforcementBus::with_exploration_rate(ReinforcementBus::new(), 0.0);
        let actions = vec![
            "code".to_string(),
            "research".to_string(),
            "review".to_string(),
        ];
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
    fn test_set_exploration_rate() {
        let mut bus = ReinforcementBus::new();
        bus.set_exploration_rate(0.05);
        assert!((bus.exploration_rate - 0.05).abs() < 0.01);
        // Out-of-range values are clamped into [0, 1].
        bus.set_exploration_rate(5.0);
        assert!((bus.exploration_rate - 1.0).abs() < 0.01);
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
