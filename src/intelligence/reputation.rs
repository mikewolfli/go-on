//! S13: Node Reputation Tracker
//!
//! Maintains an EMA-based reliability score per agent/node.  Scores feed the
//! router's ranking to downweight consistently failing agents.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::intelligence::now_ms;

/// Reputation record for a single agent/node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReputationRecord {
    pub agent: String,
    /// EMA reliability score 0.0–1.0
    pub score: f64,
    pub total_tasks: u64,
    pub success_count: u64,
    pub failure_count: u64,
    pub consecutive_failures: u32,
    pub last_updated_ms: u64,
}

impl ReputationRecord {
    fn new(agent: &str) -> Self {
        Self {
            agent: agent.to_string(),
            score: 1.0,
            total_tasks: 0,
            success_count: 0,
            failure_count: 0,
            consecutive_failures: 0,
            last_updated_ms: now_ms(),
        }
    }
}

/// Configuration for reputation tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReputationConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// EMA smoothing factor α (0.0–1.0); higher = faster adaption
    #[serde(default = "default_alpha")]
    pub ema_alpha: f64,
    /// Score threshold below which agent is marked "degraded"
    #[serde(default = "default_degraded")]
    pub degraded_threshold: f64,
    /// Score threshold below which agent is excluded from routing
    #[serde(default = "default_excluded")]
    pub exclusion_threshold: f64,
}

fn default_enabled() -> bool {
    true
}
fn default_alpha() -> f64 {
    0.2
}
fn default_degraded() -> f64 {
    0.65
}
fn default_excluded() -> f64 {
    0.30
}

impl Default for ReputationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ema_alpha: 0.2,
            degraded_threshold: 0.65,
            exclusion_threshold: 0.30,
        }
    }
}

/// Central reputation store
#[derive(Debug, Default)]
pub struct ReputationStore {
    config: ReputationConfig,
    records: HashMap<String, ReputationRecord>,
}

impl ReputationStore {
    pub fn new(config: ReputationConfig) -> Self {
        Self {
            config,
            records: HashMap::new(),
        }
    }

    fn record(&mut self, agent: &str) -> &mut ReputationRecord {
        self.records
            .entry(agent.to_string())
            .or_insert_with(|| ReputationRecord::new(agent))
    }

    /// Record a task outcome for an agent
    pub fn record_outcome(&mut self, agent: &str, success: bool) {
        let alpha = self.config.ema_alpha;
        let r = self.record(agent);

        // Capture the previous timestamp before updating it, for decay calculation
        let prev_updated_ms = r.last_updated_ms;

        r.total_tasks += 1;
        r.last_updated_ms = now_ms();
        if success {
            r.success_count += 1;
            r.consecutive_failures = 0;
        } else {
            r.failure_count += 1;
            r.consecutive_failures += 1;
        }
        let outcome = if success { 1.0f64 } else { 0.0f64 };
        r.score = alpha * outcome + (1.0 - alpha) * r.score;

        // Apply time-based decay: reduce weight of old scores
        let now_ms_val = r.last_updated_ms;
        let elapsed_ms = now_ms_val.saturating_sub(prev_updated_ms);
        if prev_updated_ms > 0 && elapsed_ms > 86_400_000 {
            // Apply decay factor for entries older than 24 hours since last update
            let elapsed_hours = elapsed_ms as f64 / 3_600_000.0;
            let decay = (-0.01 * (elapsed_hours - 24.0)).exp(); // exponential decay
            r.score = 1.0 + (r.score - 1.0) * decay;
        }
    }

    /// Current score for agent (1.0 for unknown agents)
    pub fn score(&self, agent: &str) -> f64 {
        self.records.get(agent).map(|r| r.score).unwrap_or(1.0)
    }

    pub fn is_degraded(&self, agent: &str) -> bool {
        self.score(agent) < self.config.degraded_threshold
    }

    pub fn is_excluded(&self, agent: &str) -> bool {
        self.score(agent) < self.config.exclusion_threshold
    }

    pub fn snapshot(&self) -> Vec<ReputationRecord> {
        let mut v: Vec<_> = self.records.values().cloned().collect();
        v.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        v
    }

    pub fn tracked_agent_count(&self) -> usize {
        self.records.len()
    }
}

// Use `crate::intelligence::now_ms()` instead — shared utility in mod.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_store_empty() {
        let store = ReputationStore::new(ReputationConfig::default());
        assert_eq!(store.tracked_agent_count(), 0);
    }

    #[test]
    fn test_record_outcome_creates_entry() {
        let mut store = ReputationStore::new(ReputationConfig::default());
        store.record_outcome("agent_a", true);
        assert_eq!(store.tracked_agent_count(), 1);
        let score = store.score("agent_a");
        assert!(score > 0.0);
    }

    #[test]
    fn test_score_increases_on_success() {
        let mut store = ReputationStore::new(ReputationConfig::default());
        // First failure lowers the score from 1.0
        store.record_outcome("agent_a", false);
        let s1 = store.score("agent_a");
        assert!(s1 < 1.0);
        // Then a success should increase it
        store.record_outcome("agent_a", true);
        let s2 = store.score("agent_a");
        assert!(s2 > s1);
    }

    #[test]
    fn test_score_decreases_on_failure() {
        let mut store = ReputationStore::new(ReputationConfig::default());
        store.record_outcome("agent_a", true);
        let s1 = store.score("agent_a");
        store.record_outcome("agent_a", false);
        let s2 = store.score("agent_a");
        assert!(s2 < s1);
    }

    #[test]
    fn test_is_degraded_after_many_failures() {
        let mut store = ReputationStore::new(ReputationConfig::default());
        for _ in 0..10 {
            store.record_outcome("agent_a", false);
        }
        assert!(store.is_degraded("agent_a"));
    }

    #[test]
    fn test_snapshot_includes_all_agents() {
        let mut store = ReputationStore::new(ReputationConfig::default());
        store.record_outcome("agent_a", true);
        store.record_outcome("agent_b", false);
        let snap = store.snapshot();
        assert_eq!(snap.len(), 2);
    }

    #[test]
    fn test_unknown_agent_score_is_default() {
        let store = ReputationStore::new(ReputationConfig::default());
        assert!((store.score("unknown") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_unknown_agent_not_degraded() {
        let store = ReputationStore::new(ReputationConfig::default());
        assert!(!store.is_degraded("unknown"));
    }
}
