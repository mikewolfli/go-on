//! Feedback subsystem — stage 4 of the capability bus lifecycle
//!
//! Writes execution results back to sub-buses (learning bus, reputation,
//! observability, optimization, protocol, memory, distributed memory,
//! provenance ledger, self-model).
//!
//! Extracted from `core.rs` to isolate the `feedback()` method and its
//! feedback loop helpers. (BLUE38 ARCH-13)

use super::core::CapabilityBus;
#[cfg(feature = "sub-bus-orchestration")]
use crate::intelligence::capability_bus::orchestration_bus::OrchestrationBus;

use crate::shared::provenance_helpers::make_entry;

/// RAII guard that ensures `complete_flow` is called when `feedback()` returns,
/// even if an intermediate operation panics.
#[cfg(feature = "sub-bus-orchestration")]
struct FlowGuard<'a> {
    bus: &'a OrchestrationBus,
    flow_id: &'a str,
    task_id: &'a str,
}

#[cfg(feature = "sub-bus-orchestration")]
impl Drop for FlowGuard<'_> {
    fn drop(&mut self) {
        self.bus.complete_flow(self.flow_id, self.task_id);
    }
}

impl CapabilityBus {
    // ------------------------------------------------------------------
    // Stage 4: Feedback — write results to sub-buses
    // ------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub async fn feedback(
        &self,
        agent: &str,
        task_type: &str,
        task_id: &str,
        success: bool,
        duration_ms: u64,
        token_cost: u64,
        quality_score: f64,
    ) {
        let now_ms = crate::shared::timestamps::now_ts_ms_u64();

        #[cfg(feature = "sub-bus-orchestration")]
        let flow_id = format!("{}::{}", task_type, task_id);
        #[cfg(feature = "sub-bus-orchestration")]
        let _flow_guard = match self.orchestration_bus.start_flow(&flow_id, task_id) {
            Ok(_) => Some(FlowGuard {
                bus: &self.orchestration_bus,
                flow_id: &flow_id,
                task_id,
            }),
            Err(e) => {
                tracing::warn!("feedback: start_flow failed for {}: {}", flow_id, e);
                None::<FlowGuard>
            }
        };

        // 1. BLUE70: Write to LearningOptimizationBus (replaces legacy WorkflowLearningBus)
        {
            let mut lob = crate::write_or_recover!(&self.learning_optimization_bus, "intelligence");
            lob.record_and_optimize(
                crate::intelligence::capability_bus::learning_optimization_bus::LearningEvent {
                    task_type: task_type.to_string(),
                    agent: agent.to_string(),
                    success,
                    duration_ms,
                    token_cost,
                    quality_score,
                    timestamp_ms: now_ms,
                },
            );
        }

        // 2. BLUE70: Write to ReinforcementBus + UnifiedKnowledgeBus (replaces
        // legacy ReputationStore + QLearningAgent + ExperienceKnowledgeBase).
        // Lock order: reinforcement_bus (Level 1) is acquired and released
        // BEFORE unified_knowledge_bus (Level 3), matching the documented
        // ordering in core.rs. Previously the reward write happened while
        // holding the Level 3 write lock — a lock-order violation.
        {
            // Feed reward signal to reinforcement bus (Level 1 — acquire first).
            let reward = if success { 1.0 } else { -0.5 };
            let next_state = format!("{}/next", task_type);
            if let Ok(mut rb) = self.reinforcement_bus.try_write() {
                rb.record_reward(task_type, agent, reward, &next_state);
            }

            let mut ukb = crate::write_or_recover!(&self.unified_knowledge_bus, "intelligence");
            let outcome_summary = format!(
                "agent={} task={} success={} dur={}ms tokens={} quality={:.2}",
                agent, task_type, success, duration_ms, token_cost, quality_score
            );
            ukb.record_outcome(agent, task_type, success, outcome_summary);
        }

        // 3. Write to ObservabilityBus
        #[cfg(feature = "sub-bus-observability")]
        self.observability_bus
            .record_trace(agent, duration_ms, success);

        // 4. Write to OptimizationBus
        #[cfg(feature = "sub-bus-optimization")]
        self.optimization_bus
            .record_execution(agent, duration_ms, token_cost, success);

        // 4b. Update ProtocolBus with runtime latency on active transport.
        #[cfg(feature = "sub-bus-protocol")]
        {
            let active_transport = self.protocol_bus.active_transport();
            self.protocol_bus
                .record_protocol_latency(&active_transport, duration_ms);
        }

        // 4c. Persist execution summary to DistributedMemoryBus and share.
        // Note: the MemoryBus L1/L2 write path was removed — `store` was
        // called with a unique `task_type::task_id` key that no read path
        // ever matches (MemoryBus::lookup has no production caller), so every
        // request appended an unreachable entry to the L1 memory cache and the
        // L2 SQLite cache. DistributedMemoryBus below is the active
        // cross-node persistence channel.
        #[cfg(feature = "sub-bus-distributed-memory")]
        let memory_key = format!("{}::{}", task_type, task_id);
        #[cfg(feature = "sub-bus-distributed-memory")]
        {
            // Shared-memory TTL for outcome records propagated to peer nodes:
            // 5 minutes keeps transient feedback visible to peers without
            // polluting long-term distributed memory.
            const SHARED_MEM_TTL_MS: u64 = 300_000;
            let dist_id = self.distributed_memory_bus.store_local(
                &memory_key,
                &format!(
                    "agent={} success={} quality={:.3}",
                    agent, success, quality_score
                ),
                vec![task_type.to_string(), agent.to_string()],
                quality_score,
                SHARED_MEM_TTL_MS,
            );
            let _ = self.distributed_memory_bus.share_with_peers(&dist_id);
        }

        // 5. Record event
        let outcome = Self::action_outcome_label(success);
        self.record_event(
            "feedback",
            Some(agent.to_string()),
            Some(task_id.to_string()),
            outcome,
            Self::build_feedback_event_detail(duration_ms, token_cost, quality_score, success),
        );

        // 6. Record provenance
        self.provenance_ledger.append(make_entry(
            task_id,
            task_type,
            agent,
            "capability_bus",
            &serde_json::json!({"task_type": task_type, "quality_score": quality_score}),
            &serde_json::json!({"success": success, "duration_ms": duration_ms}),
            vec![],
        ));

        // 7. Record execution result in SelfModel for per-capability EMA tracking
        self.self_model
            .record_execution_result(agent, success, duration_ms);

        // `complete_flow` is called automatically by `FlowGuard` RAII guard.
    }
}
