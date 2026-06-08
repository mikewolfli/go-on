//! Sensing subsystem — stage 1 of the capability bus lifecycle
//!
//! Gathers input from sub-buses (capability graph, reputation, learning bus,
//! observability, orchestration, optimization, protocol, transport).
//!
//! Extracted from `core.rs` to isolate the `sense()` method and its helpers.
//! (BLUE38 ARCH-13)

use super::core::CapabilityBus;
use crate::governance::pua::TaskContext;
use crate::intelligence::{lock_guard, read_guard};

// ---------------------------------------------------------------------------
// Stage output type
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct SensingOutput {
    pub capability_agent_count: usize,
    pub reputation_snapshot: Vec<crate::intelligence::reputation::ReputationRecord>,
    pub recent_agents: Vec<String>,
    pub learning_snapshot: Vec<crate::intelligence::capability_bus::core::WorkflowLearningEvent>,
    /// Phase 4: healthy agents from ObservabilityBus
    #[cfg(feature = "sub-bus-observability")]
    pub healthy_agents: Vec<String>,
    /// Phase 4: available modes from OrchestrationBus
    #[cfg(feature = "sub-bus-orchestration")]
    pub available_modes: Vec<String>,
    /// Phase 4: optimization recommendation
    #[cfg(feature = "sub-bus-optimization")]
    pub optimization:
        Option<crate::intelligence::capability_bus::optimization_bus::OptimizationRecommendation>,
}

impl CapabilityBus {
    // ------------------------------------------------------------------
    // Stage 1: Sensing — gather input from sub-buses
    // ------------------------------------------------------------------

    pub fn sense(&self, task: &TaskContext) -> SensingOutput {
        // Include task risk score in heartbeat so `task` is unconditionally referenced
        // across all feature configurations.
        let cap_agents = lock_guard(&self.capability_graph).total_agents();
        let rep_snapshot = lock_guard(&self.reputation).snapshot();
        let _learning_rates = {
            let agents: Vec<String> = read_guard(&self.learning_bus)
                .snapshot()
                .iter()
                .map(|e| e.agent.clone())
                .collect();
            agents
        };
        let learning_snapshot = read_guard(&self.learning_bus).snapshot();

        // Phase 4: Query ObservabilityBus for healthy agents
        #[cfg(feature = "sub-bus-observability")]
        let healthy = self.observability_bus.healthy_agents(0.5);
        #[cfg(not(feature = "sub-bus-observability"))]
        let _healthy = Vec::<String>::new();

        // Phase 4: Query OrchestrationBus for available modes
        #[cfg(feature = "sub-bus-orchestration")]
        let modes = self.orchestration_bus.available_modes();
        #[cfg(not(feature = "sub-bus-orchestration"))]
        let _modes = Vec::<String>::new();

        // Phase 4: Get optimization recommendation
        #[cfg(any(feature = "sub-bus-optimization", feature = "sub-bus-protocol"))]
        let task_type_str = format!("{:?}", task.task_type);
        #[cfg(any(feature = "sub-bus-optimization", feature = "sub-bus-protocol"))]
        let token_estimate = (task.file_count * 512) as u64;
        #[cfg(feature = "sub-bus-optimization")]
        let opt =
            self.optimization_bus
                .recommend(&task_type_str, token_estimate.max(1024), "balanced");

        // Phase 4: Protocol recommendation (used for routing diagnostics)
        #[cfg(feature = "sub-bus-protocol")]
        {
            let proto_reco = self
                .protocol_bus
                .recommend_protocol(&task_type_str, token_estimate.max(1024));
            self.record_event(
                "sense",
                None,
                None,
                "protocol_recommend",
                serde_json::json!({
                    "preferred_protocol": proto_reco.preferred_protocol,
                    "confidence": proto_reco.confidence,
                }),
            );
        }

        // Send a heartbeat through the transport layer, including task risk score
        // so the transport is always informed of the current task context.
        let transport = lock_guard(&self.transport);
        let heartbeat = format!(
            "{{\"status\":\"alive\",\"risk_score\":{}}}",
            task.risk_score
        );
        let _ = transport.send_heartbeat("capability-bus", "harness-bus", &heartbeat);

        SensingOutput {
            capability_agent_count: cap_agents,
            reputation_snapshot: rep_snapshot,
            recent_agents: _learning_rates,
            learning_snapshot,
            #[cfg(feature = "sub-bus-observability")]
            healthy_agents: healthy,
            #[cfg(feature = "sub-bus-orchestration")]
            available_modes: modes,
            #[cfg(feature = "sub-bus-optimization")]
            optimization: Some(opt),
        }
    }

    /// Check if an agent is healthy via ObservabilityBus and OptimizationBus
    pub fn is_agent_healthy(&self, agent: &str) -> bool {
        // Check circuit breaker via OptimizationBus
        #[cfg(feature = "sub-bus-optimization")]
        if self.optimization_bus.is_circuit_broken(agent) {
            return false;
        }
        // Check error rate via ObservabilityBus
        #[cfg(feature = "sub-bus-observability")]
        if let Some(err_rate) = self.observability_bus.agent_error_rate(agent) {
            if err_rate.error_rate > 0.5 {
                return false;
            }
        }
        #[cfg(not(any(feature = "sub-bus-optimization", feature = "sub-bus-observability")))]
        let _ = agent;
        true
    }

    /// Get recommended execution mode via OrchestrationBus
    pub fn recommended_mode(&self, task_type: &str, complexity: f64) -> String {
        #[cfg(feature = "sub-bus-orchestration")]
        {
            self.orchestration_bus.recommend_mode(task_type, complexity)
        }
        #[cfg(not(feature = "sub-bus-orchestration"))]
        {
            let _ = (task_type, complexity);
            "auto".to_string()
        }
    }

    /// Get optimization recommendation for a task
    #[cfg(feature = "sub-bus-optimization")]
    pub fn optimization_recommendation(
        &self,
        task_type: &str,
        token_count: u64,
        priority: &str,
    ) -> crate::intelligence::capability_bus::optimization_bus::OptimizationRecommendation {
        self.optimization_bus
            .recommend(task_type, token_count, priority)
    }
}
