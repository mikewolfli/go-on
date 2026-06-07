//! Consensus subsystem — multi-agent voting governance
//!
//! Extracted from `core.rs` to isolate ConsensusEngine integration
//! (F-GAP-15) within the evolve pipeline.

use super::core::CapabilityBus;
use crate::intelligence::consensus::{ConsensusNode, ConsensusVote, NodeRole};
use tracing::warn;

impl CapabilityBus {
    /// Record evolve result as a round in ConsensusEngine.
    pub(crate) fn evolve_consensus(
        &self,
        state: &(String, String),
        action: &str,
        reward: f64,
        q_value: f64,
        success: bool,
        now: u64,
    ) {
        let _ = self.consensus.register_node(ConsensusNode {
            id: "capability-bus".to_string(),
            address: "internal://capability_bus".to_string(),
            weight: 1,
            role: NodeRole::Leader,
            is_online: true,
            last_heartbeat_ms: now,
        });
        let proposals = vec![serde_json::json!({
            "action": action,
            "state": state,
            "reward": reward,
            "q_value": q_value,
            "success": success,
        })];
        let proposal_id = format!("proposal_{}_{}", state.0, action);
        match self.consensus.start_round("capability-bus", proposals) {
            Ok(rid) => {
                if let Err(e) = self.consensus.cast_vote(ConsensusVote {
                    node_id: "capability-bus".to_string(),
                    round_id: rid,
                    proposal_id,
                    approve: success,
                    weight: 1,
                    vote_ms: now,
                }) {
                    warn!("evolve: consensus.cast_vote failed: {}", e);
                }
            }
            Err(e) => warn!("evolve: consensus.start_round failed: {}", e),
        }
    }
}
