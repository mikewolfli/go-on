//! F-GAP-02: Reputation scoring
//!
//! Production routing uses UnifiedKnowledgeBus for reputation scores.
//! This module provides only the minimal API compatibility layer:
//! a simple `reputation_score()` function that always returns 1.0.

use serde::{Deserialize, Serialize};

/// Reputation record for a single agent/node.
///
/// Retained for API compatibility with `SensingOutput` snapshots.
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

/// Default reputation score for any agent (1.0 = fully trusted).
pub fn reputation_score(_agent: &str) -> f64 {
    1.0
}
