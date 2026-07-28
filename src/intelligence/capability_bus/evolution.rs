//! Evolution subsystem — capability lifecycle, self-model, consciousness, world model
//!
//! Extracted from `core.rs` to isolate evolution graph tracking (F-GAP-18),
//! self-model performance snapshots, consciousness awareness metrics, and
//! world-model entity state updates.
//!
//! Each method handles its own errors via `warn!()` and respects the
//! lock ordering discipline documented in `core::CapabilityBus`.

use super::core::CapabilityBus;
use crate::intelligence::consciousness::AwarenessMetricType;
use crate::intelligence::evolution_graph::{EvolutionStage, TrendDirection};

use crate::intelligence::self_model::SelfPerformanceSnapshot;
use crate::intelligence::world_model::EntityType;
use tracing::warn;

impl CapabilityBus {
    /// Update EvolutionGraph with capability trajectory.
    pub(crate) fn evolve_evolution_graph(
        &self,
        state: &(String, String),
        action: &str,
        success: bool,
        quality_score: f64,
    ) {
        let mut eg = crate::lock_or_recover!(&self.evolution_graph, "intelligence");
        let cap_name = format!("evolve_{}", action);
        if let Err(e) = eg.register_capability(&state.0, &cap_name, EvolutionStage::New) {
            warn!("evolve: evolution_graph.register_capability failed: {}", e);
        }
        if let Err(e) = eg.record_version(
            &state.0,
            &cap_name,
            if success { quality_score } else { 0.0 },
            0.0,
        ) {
            warn!("evolve: evolution_graph.record_version failed: {}", e);
        }
        if success && quality_score > 0.8 {
            if let Some(rec) = eg.get_history(&state.0, &cap_name) {
                let next_stage = match rec.current_stage {
                    EvolutionStage::New => Some(EvolutionStage::Learning),
                    EvolutionStage::Learning
                        if rec.versions.len() >= 3 && rec.trend == TrendDirection::Improving =>
                    {
                        Some(EvolutionStage::Mature)
                    }
                    _ => None,
                };
                if let Some(stage) = next_stage {
                    if let Err(e) = eg.advance_stage(&state.0, &cap_name, stage) {
                        warn!("evolve: evolution_graph.advance_stage failed: {}", e);
                    }
                }
            }
        }
    }

    /// Record performance snapshot in SelfModel.
    pub(crate) fn evolve_self_model(&self, now: u64, success: bool) {
        let snapshot = SelfPerformanceSnapshot {
            timestamp_ms: now,
            avg_latency_ms: 0.0,
            p50_latency_ms: 0.0,
            p95_latency_ms: 0.0,
            p99_latency_ms: 0.0,
            error_rate: if success { 0.0 } else { 1.0 },
            throughput: 1.0,
            agent_count: 1,
            tasks_processed: 1,
        };
        self.self_model.record_performance(snapshot);
    }

    /// Record awareness metrics in Consciousness.
    pub(crate) fn evolve_consciousness(
        &self,
        state: &(String, String),
        action: &str,
        quality_score: f64,
        success: bool,
    ) {
        let awareness_value = if success { quality_score } else { 0.1 };
        let _ = self.consciousness.record_metric(
            AwarenessMetricType::SelfAwareness,
            awareness_value,
            quality_score,
        );
        let _ = self.consciousness.record_metric(
            AwarenessMetricType::EnvironmentalAwareness,
            if quality_score > 0.5 { 0.7 } else { 0.3 },
            quality_score,
        );
        let profile = self.consciousness.profile();
        if profile.reflexion_count < 100 && success {
            let _ = self
                .consciousness
                .trigger_reflexion(&format!("evolve_cycle_{}_{}", state.0, action));
        }
    }

    /// Update WorldModel with entity state.
    pub(crate) fn evolve_world_model(&self, action: &str, state: &(String, String), reward: f64) {
        if let Err(e) = self
            .world_model
            .register_entity(&format!("action_{}", action), EntityType::System)
        {
            warn!("evolve: world_model.register_entity failed: {}", e);
        } else {
            let mut props = std::collections::HashMap::new();
            props.insert("state_0".to_string(), state.0.clone());
            props.insert("state_1".to_string(), state.1.clone());
            props.insert("reward".to_string(), reward.to_string());
            if let Err(e) = self
                .world_model
                .update_entity(&format!("action_{}", action), props)
            {
                warn!("evolve: world_model.update_entity failed: {}", e);
            }
        }
    }
}
