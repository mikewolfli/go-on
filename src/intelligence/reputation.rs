//! F-GAP-02: Reputation Store
//!
//! Maintains an EMA-based reliability score per agent/node.  Scores feed the
//! router's ranking to downweight consistently failing agents.
//!
//! NOTE: Production routing uses UnifiedKnowledgeBus for reputation scores.
//! ReputationStore is retained for API compatibility (agent_selector.rs type
//! signature) and is populated only in tests.

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

/// Central reputation store — kept minimal for API compatibility.
///
/// Only `score()` is used in production (via `agent_selector.rs`).
/// All mutation/persistence features are test-only.
#[derive(Debug)]
pub struct ReputationStore {
    records: HashMap<String, ReputationRecord>,
}

impl ReputationStore {
    /// Current score for agent (1.0 for unknown agents)
    pub fn score(&self, agent: &str) -> f64 {
        self.records.get(agent).map(|r| r.score).unwrap_or(1.0)
    }
}
