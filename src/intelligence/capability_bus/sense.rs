//! Sensing subsystem — stage 1 of the capability bus lifecycle
//!
//! Gathers input from sub-buses (capability graph, reputation, learning bus,
//! observability, orchestration, optimization, protocol, transport).
//!
//! Extracted from `core.rs` to isolate the `sense()` method and its helpers.
//! (BLUE38 ARCH-13)

use super::core::{CapabilityBus, WorkflowLearningEvent};
use crate::governance::pua::TaskContext;

// ---------------------------------------------------------------------------
// Stage output type
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct SensingOutput {
    pub capability_agent_count: usize,
    pub reputation_snapshot: Vec<crate::intelligence::reputation::ReputationRecord>,
    pub recent_agents: Vec<String>,
    pub learning_snapshot: Vec<crate::intelligence::capability_bus::core::WorkflowLearningEvent>,
}

impl CapabilityBus {
    // ------------------------------------------------------------------
    // Stage 1: Sensing — gather input from sub-buses
    // ------------------------------------------------------------------

    pub fn sense(&self, task: &TaskContext) -> SensingOutput {
        // Include task risk score in heartbeat so `task` is unconditionally referenced
        // across all feature configurations.
        let cap_agents =
            crate::lock_or_recover!(&self.capability_graph, "intelligence").total_agents();
        // BLUE70: Read from UnifiedKnowledgeBus (replaces legacy ReputationStore)
        let rep_snapshot = {
            let ukb = crate::read_or_recover!(&self.unified_knowledge_bus, "intelligence");
            ukb.all_reputations()
                .into_iter()
                .map(|r| crate::intelligence::reputation::ReputationRecord {
                    agent: r.agent.clone(),
                    score: r.score,
                    total_tasks: r.total_tasks,
                    success_count: r.successful_tasks,
                    failure_count: r.total_tasks.saturating_sub(r.successful_tasks),
                })
                .collect::<Vec<_>>()
        };
        // BLUE70: Read from LearningOptimizationBus (replaces legacy WorkflowLearningBus)
        // Single snapshot, two derived views: `recent_agents` (names, for
        // recency scoring in decide) and `learning_snapshot` (full events).
        // Previously two full `events_snapshot()` clones were taken.
        let lob = crate::read_or_recover!(&self.learning_optimization_bus, "intelligence");
        let lob_events = lob.events_snapshot();
        let recent_agents = lob_events
            .iter()
            .map(|e| e.agent.clone())
            .collect::<Vec<_>>();
        let learning_snapshot: Vec<WorkflowLearningEvent> = lob_events
            .into_iter()
            .map(|e| WorkflowLearningEvent {
                task_type: e.task_type,
                agent: e.agent,
                success: e.success,
                duration_ms: e.duration_ms,
                token_cost: e.token_cost,
                quality_score: e.quality_score,
                timestamp_ms: e.timestamp_ms,
            })
            .collect();

        // Phase 4: Protocol recommendation (used for routing diagnostics)
        #[cfg(feature = "sub-bus-protocol")]
        {
            let task_type_str = format!("{:?}", task.task_type);
            let token_estimate = (task.file_count * 512) as u64;
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

        // Log sense heartbeat — previously sent via MultiChannelTransport
        // which was removed as dead code (~740 lines, only 1 usage).
        tracing::debug!(
            risk_score = task.risk_score,
            "sense: heartbeat (MultiChannelTransport removed)"
        );

        SensingOutput {
            capability_agent_count: cap_agents,
            reputation_snapshot: rep_snapshot,
            recent_agents,
            learning_snapshot,
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
