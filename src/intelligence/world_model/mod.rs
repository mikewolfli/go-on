//! BLUE38 F-GAP-23: World Model Pipeline (M7 "世界模型流水线")
//!
//! A thread-safe pipeline that maintains a structured representation of the
//! external environment — tracking entities, events, and state changes over
//! time. All state is guarded behind `Arc<Mutex<>>`.
//!
//! The production surface is deliberately small: entity registration/updates,
//! event recording, and observability (`profile()` feeds the capability-bus
//! status payload). The former Bayesian causal-graph scoring consumed by
//! `CapabilityBus::decide()` (`causal_agent_insight` /
//! `counterfactual_probability`) was removed because the write side only ever
//! registered `action_*` entities while the query side looked up agent names —
//! the scores were constant (0.5 / 0.0) and the per-candidate MCTS runs were
//! wasted work. Agent-success learning is served by the working `discovery`
//! channel (`state_{agent}` pattern consumed by `decide()`).

mod types;

pub use types::*;

use crate::i18n::runtime::tf;

use anyhow::{bail, Result};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

/// Internal state guarded by `Arc<Mutex<>>`.
#[derive(Debug)]
struct Inner {
    config: WorldModelConfig,
    entities: Vec<WorldEntity>,
    events: Vec<WorldEvent>,
    last_update_ms: u64,
    next_entity_id: u64,
    next_event_id: u64,
}

// ---------------------------------------------------------------------------
// Public API — WorldModel
// ---------------------------------------------------------------------------

/// Thread-safe world model pipeline that maintains a structured representation
/// of entities, relationships, events, and state changes over time.
#[derive(Debug, Clone)]
pub struct WorldModel {
    inner: Arc<Mutex<Inner>>,
}

impl WorldModel {
    /// Create a new world model with the given configuration.
    pub fn new(config: WorldModelConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                config,
                entities: Vec::new(),
                events: Vec::new(),
                last_update_ms: crate::shared::timestamps::now_ts_ms() as u64,
                next_entity_id: 1,
                next_event_id: 1,
            })),
        }
    }

    // -- Entity management -------------------------------------------------

    /// Register a new entity and return its assigned entity ID.
    ///
    /// Returns an error if an entity with the same `name` and `entity_type`
    /// already exists, or if the maximum number of entities has been reached.
    pub fn register_entity(&self, name: &str, entity_type: EntityType) -> Result<String> {
        let mut inner = crate::lock_or_recover!(&self.inner, "intelligence");
        let now = crate::shared::timestamps::now_ts_ms() as u64;

        // Check for duplicate by name + type.
        if inner
            .entities
            .iter()
            .any(|e| e.name == name && e.entity_type == entity_type)
        {
            bail!(
                "{}",
                tf(
                    "error.entity_already_registered",
                    &[
                        ("name", name),
                        ("entity_type", &format!("{:?}", entity_type))
                    ]
                )
            );
        }

        // Enforce max entities limit — evict the oldest entity if at capacity.
        while inner.entities.len() >= inner.config.max_entities {
            if let Some(pos) = inner
                .entities
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.last_seen_ms)
                .map(|(i, _)| i)
            {
                inner.entities.swap_remove(pos);
            }
        }

        let id = format!("ent_{}", inner.next_entity_id);
        inner.next_entity_id += 1;

        let entity = WorldEntity {
            id: id.clone(),
            name: name.to_string(),
            entity_type,
            properties: HashMap::new(),
            confidence: 1.0,
            last_seen_ms: now,
            created_ms: now,
        };

        inner.entities.push(entity);
        inner.last_update_ms = now;

        Ok(id)
    }

    /// Update properties of an existing entity.
    ///
    /// Merges the provided `properties` into the entity's existing properties.
    /// Returns an error if no entity with the given `id` exists.
    pub fn update_entity(&self, id: &str, properties: HashMap<String, String>) -> Result<()> {
        let mut inner = crate::lock_or_recover!(&self.inner, "intelligence");
        let now = crate::shared::timestamps::now_ts_ms() as u64;

        let entity = inner
            .entities
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or_else(|| anyhow::anyhow!("{}", tf("error.entity_not_found", &[("id", id)])))?;

        for (key, value) in properties {
            entity.properties.insert(key, value);
        }
        entity.last_seen_ms = now;
        inner.last_update_ms = now;

        Ok(())
    }

    // -- Event management --------------------------------------------------

    /// Record an event and return its assigned event ID.
    pub fn record_event(
        &self,
        event_type: &str,
        source: &str,
        payload: HashMap<String, String>,
    ) -> Result<String> {
        let mut inner = crate::lock_or_recover!(&self.inner, "intelligence");
        let now = crate::shared::timestamps::now_ts_ms() as u64;

        let id = format!("evt_{}", inner.next_event_id);
        inner.next_event_id += 1;

        let event = WorldEvent {
            id: id.clone(),
            event_type: event_type.to_string(),
            source: source.to_string(),
            target: None,
            payload,
            confidence: 1.0,
            timestamp_ms: now,
        };

        inner.events.push(event);

        // Enforce max events limit by trimming oldest.
        while inner.events.len() > inner.config.max_events {
            inner.events.remove(0);
        }

        inner.last_update_ms = now;

        Ok(id)
    }

    // -- Profile / observability ---------------------------------------------

    /// Return a read-only snapshot of the world model state so the accumulated
    /// entities/events are observable via the capability-bus profile.
    pub fn profile(&self) -> WorldModelProfile {
        let inner = crate::lock_or_recover!(&self.inner, "intelligence");
        let mut entities_by_type: HashMap<String, usize> = HashMap::new();
        for e in &inner.entities {
            *entities_by_type
                .entry(format!("{:?}", e.entity_type))
                .or_insert(0) += 1;
        }
        let recent_event_types = inner
            .events
            .iter()
            .rev()
            .take(10)
            .map(|e| e.event_type.clone())
            .collect();
        WorldModelProfile {
            entities: inner.entities.len(),
            events: inner.events.len(),
            last_update_ms: inner.last_update_ms,
            entities_by_type,
            recent_event_types,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: default config for tests with small capacity.
    fn test_config() -> WorldModelConfig {
        WorldModelConfig {
            max_entities: 100,
            max_events: 50,
            state_retention_ms: 3_600_000, // 1 hour (won't trigger in normal tests)
            confidence_threshold: 0.3,
        }
    }

    #[test]
    fn test_register_entity() {
        let wm = WorldModel::new(test_config());

        let id = wm
            .register_entity("Sensor-1", EntityType::Resource)
            .unwrap();

        // Verify the ID is non-empty.
        assert!(!id.is_empty());
        assert!(id.starts_with("ent_"));
    }

    #[test]
    fn test_register_duplicate_entity() {
        let wm = WorldModel::new(test_config());

        wm.register_entity("Dup", EntityType::Agent).unwrap();
        let result = wm.register_entity("Dup", EntityType::Agent);

        assert!(result.is_err());
    }

    #[test]
    fn test_update_entity() {
        let wm = WorldModel::new(test_config());

        let id = wm.register_entity("Updatable", EntityType::System).unwrap();

        let mut props = HashMap::new();
        props.insert("version".to_string(), "2.1.0".to_string());
        props.insert("status".to_string(), "online".to_string());

        wm.update_entity(&id, props).unwrap();

        // Verify the update is recorded by reading the entity back.
        let inner = wm.inner.lock().unwrap();
        let entity = inner.entities.iter().find(|e| e.id == id).unwrap();
        assert_eq!(entity.properties.get("version").unwrap(), "2.1.0");
        assert_eq!(entity.properties.get("status").unwrap(), "online");
    }

    #[test]
    fn test_record_event() {
        let wm = WorldModel::new(test_config());

        let id = wm
            .register_entity("EventSource", EntityType::System)
            .unwrap();

        let mut payload = HashMap::new();
        payload.insert("severity".to_string(), "critical".to_string());
        payload.insert("message".to_string(), "disk full".to_string());

        let event_id = wm.record_event("alert", &id, payload).unwrap();

        assert!(!event_id.is_empty());
        assert!(event_id.starts_with("evt_"));
    }
}
