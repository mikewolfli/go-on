//! F-GAP-02: Reputation scoring
//!
//! Production routing uses UnifiedKnowledgeBus for reputation scores; this
//! module defines the serializable `ReputationRecord` snapshot type consumed
//! by `CapabilityBus::sense()`.

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
}
