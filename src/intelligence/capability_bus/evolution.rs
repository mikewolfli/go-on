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
    ///
    /// Get-or-create semantics: `register_entity` returns the assigned id on
    /// first call (`ent_{n}`); on subsequent calls the same name+type pair is
    /// a duplicate, so we fall back to `find_entity_id`. The update always
    /// targets the entity's real id — previously the caller reused the
    /// `action_{action}` name as the id, which `update_entity` never matched
    /// (ids are `ent_{n}`), so properties (state/reward) were never written.
    pub(crate) fn evolve_world_model(&self, action: &str, state: &(String, String), reward: f64) {
        let name = format!("action_{}", action);
        let id = match self.world_model.register_entity(&name, EntityType::System) {
            Ok(id) => id,
            Err(_) => match self.world_model.find_entity_id(&name, EntityType::System) {
                Some(id) => id,
                None => {
                    warn!("evolve: world_model.find_entity_id failed for {name}");
                    return;
                }
            },
        };
        let mut props = std::collections::HashMap::new();
        props.insert("state_0".to_string(), state.0.clone());
        props.insert("state_1".to_string(), state.1.clone());
        props.insert("reward".to_string(), reward.to_string());
        if let Err(e) = self.world_model.update_entity(&id, props) {
            warn!("evolve: world_model.update_entity failed: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CapabilityBus;
    use crate::governance::harness_bus::default_harness_bus;
    use std::sync::Arc;

    /// Regression (P1): `evolve_world_model` must get-or-create entities so
    /// repeated evolve cycles do not duplicate the entity or log duplicate-
    /// registration warnings. The previous implementation used the
    /// `action_{name}` string as the entity id, which never matched the real
    /// `ent_{n}` id assigned by `register_entity`.
    #[tokio::test]
    async fn evolve_world_model_get_or_creates_single_entity() {
        let bus = CapabilityBus::new_default(Arc::new(default_harness_bus()), None);
        let state = ("ready".to_string(), "working".to_string());

        // Two evolve cycles for the same action: second must reuse, not
        // duplicate (duplication would also emit a warn per cycle).
        bus.evolve_world_model("analyze", &state, 0.75);
        bus.evolve_world_model("analyze", &state, 0.9);

        let id = bus
            .world_model
            .find_entity_id(
                "action_analyze",
                crate::intelligence::world_model::EntityType::System,
            )
            .expect("entity should exist after evolve_world_model");
        assert!(
            id.starts_with("ent_"),
            "get-or-create must return the real entity id, got {id}"
        );
        // Two cycles → one entity; the second call found the existing id
        // instead of failing registration.
        let profile = bus.world_model.profile();
        assert_eq!(profile.entities, 1, "one entity, not duplicates");
    }
}
