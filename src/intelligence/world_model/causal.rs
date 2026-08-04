//! Causal reasoner — entity state-history tracking.
//!
//! Production keeps only the state-history side of the former correlation
//! engine: `WorldModel::update_entity` records snapshots here and reads the
//! `history` buffer directly to feed the Bayesian graph. The correlation
//! analysis (`infer_correlations` / `correlations` / `Display`) had zero
//! production consumers after the world-model inference batch API was removed.

use super::types::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Maintains historical entity state snapshots used to derive state
/// transitions for Bayesian graph feeding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CausalReasoner {
    /// Historical entity state snapshots for correlation analysis.
    pub(crate) history: Vec<EntityStateSnapshot>,
    /// Maximum number of snapshots to retain.
    max_history: usize,
}

impl CausalReasoner {
    /// Creates a new reasoner with the given history capacity.
    pub fn new(max_history: usize) -> Self {
        Self {
            history: Vec::with_capacity(max_history),
            max_history,
        }
    }

    /// Records a state snapshot for an entity.
    /// Evicts the oldest snapshot when history is at capacity.
    pub fn record_state(
        &mut self,
        entity_id: &str,
        properties: HashMap<String, String>,
        timestamp_ms: u64,
    ) {
        if self.history.len() >= self.max_history {
            self.history.remove(0);
        }
        self.history.push(EntityStateSnapshot {
            entity_id: entity_id.to_string(),
            properties,
            timestamp_ms,
        });
    }
}
