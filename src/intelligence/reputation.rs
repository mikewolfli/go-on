//! S13: Node Reputation Tracker
//!
//! Maintains an EMA-based reliability score per agent/node.  Scores feed the
//! router's ranking to downweight consistently failing agents.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
