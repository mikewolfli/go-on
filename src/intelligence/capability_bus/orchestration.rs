//! Orchestration subsystem — transport, council, and agent factory helpers
//!
//! Extracted from `core.rs` to isolate MultiChannelTransport,
//! OrchestrationCouncil, and AgentFactory integration within the
//! sense/decide/evolve pipeline (F-GAP-13, F-GAP-15, F-GAP-29).

use super::core::CapabilityBus;
use crate::intelligence::lock_guard;
use tracing::warn;

impl CapabilityBus {
    /// Send an evolve summary event through the transport layer.
    pub(crate) fn evolve_send_transport_event(
        &self,
        q_value: f64,
        exploration_rate: f64,
    ) {
        let transport = lock_guard(&self.transport);
        let summary = serde_json::json!({
            "q_value": q_value,
            "exploration_rate": exploration_rate,
        });
        if let Err(e) = transport.send_event("capability-bus", "monitor", &summary.to_string()) {
            warn!("evolve: transport.send_event failed: {}", e);
        }
    }
}
