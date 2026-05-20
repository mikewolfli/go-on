//! BLUE38 F-GAP-23: World Model Pipeline (M7 "世界模型流水线")
//!
//! A thread-safe pipeline that maintains a structured representation of the
//! external environment — tracking entities, relationships, events, and state
//! changes over time. All state is guarded behind `Arc<Mutex<>>`.

use crate::i18n::runtime::tf;
use crate::intelligence::lock_guard;
use crate::intelligence::now_ms;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Causal inference & prediction
// ---------------------------------------------------------------------------

/// A causal link between two entities: action_entity causes effect_entity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CausalLink {
    /// The entity representing the cause/action
    pub cause_entity_id: String,
    /// The entity representing the effect/outcome
    pub effect_entity_id: String,
    /// Confidence in this causal relationship (0.0 – 1.0)
    pub confidence: f64,
    /// Number of times this causation has been observed
    pub observation_count: u64,
    /// Average time delay between cause and effect (ms)
    pub avg_delay_ms: f64,
    /// Context tags under which this causal link is valid
    pub context_tags: Vec<String>,
}

/// A prediction about a future entity state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prediction {
    /// The entity being predicted
    pub entity_id: String,
    /// The predicted attribute values
    pub predicted_attributes: serde_json::Value,
    /// Confidence in prediction (0.0 – 1.0)
    pub confidence: f64,
    /// Time horizon of the prediction (ms from now)
    pub horizon_ms: u64,
    /// What action/event this prediction is based on
    pub based_on: String,
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the world model pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldModelConfig {
    /// Maximum number of entities before rejection of new registrations.
    pub max_entities: usize,
    /// Maximum number of events stored in the ring buffer.
    pub max_events: usize,
    /// Time in milliseconds after which an entity/event is considered stale.
    pub state_retention_ms: u64,
    /// Minimum confidence required for an entity to be considered valid.
    pub confidence_threshold: f64,
}

impl Default for WorldModelConfig {
    fn default() -> Self {
        Self {
            max_entities: 1000,
            max_events: 5000,
            state_retention_ms: 3_600_000, // 1 hour
            confidence_threshold: 0.3,
        }
    }
}

// ---------------------------------------------------------------------------
// Enums — EntityType & RelationshipType
// ---------------------------------------------------------------------------

/// Classification of an entity in the world model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EntityType {
    Agent,
    Resource,
    System,
    User,
    Service,
    DataStore,
    External,
}

/// Classification of a relationship between two entities.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RelationshipType {
    DependsOn,
    Owns,
    CommunicatesWith,
    Contains,
    Manages,
    Unknown,
}

// ---------------------------------------------------------------------------
// Core data structures
// ---------------------------------------------------------------------------

/// A tracked entity in the world model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldEntity {
    /// Unique identifier for this entity.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Classification of this entity.
    pub entity_type: EntityType,
    /// Arbitrary key-value properties.
    pub properties: HashMap<String, String>,
    /// Confidence score in [0.0, 1.0].
    pub confidence: f64,
    /// Unix timestamp (milliseconds) when this entity was last observed.
    pub last_seen_ms: u64,
    /// Unix timestamp (milliseconds) when this entity was created.
    pub created_ms: u64,
}

/// A directed relationship between two entities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relationship {
    /// ID of the source entity.
    pub source_id: String,
    /// ID of the target entity.
    pub target_id: String,
    /// Classification of this relationship.
    pub rel_type: RelationshipType,
    /// Weight of the relationship in [0.0, 1.0].
    pub weight: f64,
    /// Unix timestamp (milliseconds) when this relationship was discovered.
    pub discovered_ms: u64,
}

/// An event that occurred in the world.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldEvent {
    /// Unique identifier for this event.
    pub id: String,
    /// Type/category of the event.
    pub event_type: String,
    /// Source entity that produced the event.
    pub source: String,
    /// Optional target entity that the event affects.
    pub target: Option<String>,
    /// Arbitrary key-value payload data.
    pub payload: HashMap<String, String>,
    /// Confidence score in [0.0, 1.0].
    pub confidence: f64,
    /// Unix timestamp (milliseconds) when this event occurred.
    pub timestamp_ms: u64,
}

/// A point-in-time snapshot of the world model's state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSnapshot {
    /// Unique identifier for this snapshot.
    pub snapshot_id: String,
    /// Entities at the time of capture.
    pub entities: Vec<WorldEntity>,
    /// Relationships at the time of capture.
    pub relationships: Vec<Relationship>,
    /// Unix timestamp (milliseconds) when this snapshot was captured.
    pub captured_ms: u64,
}

/// Runtime profile of the world model's current state.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorldModelProfile {
    /// Total number of registered entities.
    pub total_entities: usize,
    /// Total number of recorded relationships.
    pub total_relationships: usize,
    /// Total number of recorded events.
    pub total_events: usize,
    /// Average confidence across all entities.
    pub avg_entity_confidence: f64,
    /// Unix timestamp (milliseconds) of the last update.
    pub last_update_ms: u64,
    /// Number of entities that are currently stale.
    pub stale_entity_count: usize,
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

/// Internal state guarded by `Arc<Mutex<>>`.
#[derive(Debug)]
struct Inner {
    config: WorldModelConfig,
    entities: Vec<WorldEntity>,
    relationships: Vec<Relationship>,
    events: Vec<WorldEvent>,
    last_update_ms: u64,
    next_entity_id: u64,
    next_event_id: u64,
    next_snapshot_id: u64,
    causal_links: Vec<CausalLink>,
    #[allow(dead_code)] // F-GAP-08 — reserved for future learning/intelligence wiring
    predictions: Vec<Prediction>,
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
                relationships: Vec::new(),
                events: Vec::new(),
                last_update_ms: now_ms(),
                next_entity_id: 1,
                next_event_id: 1,
                next_snapshot_id: 1,
                causal_links: Vec::new(),
                predictions: Vec::new(),
            })),
        }
    }

    // -- Entity management -------------------------------------------------

    /// Register a new entity and return its assigned entity ID.
    ///
    /// Returns an error if an entity with the same `name` and `entity_type`
    /// already exists, or if the maximum number of entities has been reached.
    pub fn register_entity(&self, name: &str, entity_type: EntityType) -> Result<String> {
        let mut inner = lock_guard(&self.inner);
        let now = now_ms();

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

        // Enforce max entities limit.
        if inner.entities.len() >= inner.config.max_entities {
            bail!(
                "max entities ({}) reached — cannot register '{}'",
                inner.config.max_entities,
                name
            );
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
        let mut inner = lock_guard(&self.inner);
        let now = now_ms();

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

    /// Remove an entity (and all of its relationships) by ID.
    ///
    /// Returns an error if no entity with the given `id` exists.
    pub fn remove_entity(&self, id: &str) -> Result<()> {
        let mut inner = lock_guard(&self.inner);
        let now = now_ms();

        let pos = inner
            .entities
            .iter()
            .position(|e| e.id == id)
            .ok_or_else(|| anyhow::anyhow!("{}", tf("error.entity_not_found", &[("id", id)])))?;

        inner.entities.remove(pos);

        // Remove all relationships that reference this entity.
        inner
            .relationships
            .retain(|r| r.source_id != id && r.target_id != id);

        inner.last_update_ms = now;
        Ok(())
    }

    // -- Relationship management -------------------------------------------

    /// Record a relationship between two entities.
    ///
    /// Returns an error if either entity does not exist.
    pub fn record_relationship(
        &self,
        source_id: &str,
        target_id: &str,
        rel_type: RelationshipType,
        weight: f64,
    ) -> Result<()> {
        let mut inner = lock_guard(&self.inner);
        let now = now_ms();

        // Verify both entities exist.
        if !inner.entities.iter().any(|e| e.id == source_id) {
            bail!("{}", tf("error.entity_not_found", &[("id", source_id)]));
        }
        if !inner.entities.iter().any(|e| e.id == target_id) {
            bail!("{}", tf("error.entity_not_found", &[("id", target_id)]));
        }

        // Clamp weight to [0.0, 1.0].
        let clamped_weight = weight.clamp(0.0, 1.0);

        let relationship = Relationship {
            source_id: source_id.to_string(),
            target_id: target_id.to_string(),
            rel_type,
            weight: clamped_weight,
            discovered_ms: now,
        };

        inner.relationships.push(relationship);
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
        let mut inner = lock_guard(&self.inner);
        let now = now_ms();

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

    // -- Queries -----------------------------------------------------------

    /// Query entities, optionally filtered by `entity_type` and `min_confidence`.
    pub fn query_entities(
        &self,
        entity_type: Option<EntityType>,
        min_confidence: f64,
    ) -> Vec<WorldEntity> {
        let inner = lock_guard(&self.inner);

        inner
            .entities
            .iter()
            .filter(|e| {
                let type_match = match &entity_type {
                    Some(t) => e.entity_type == *t,
                    None => true,
                };
                type_match && e.confidence >= min_confidence
            })
            .cloned()
            .collect()
    }

    /// Query all relationships involving the given entity ID.
    pub fn query_relationships(&self, entity_id: &str) -> Vec<Relationship> {
        let inner = lock_guard(&self.inner);

        inner
            .relationships
            .iter()
            .filter(|r| r.source_id == entity_id || r.target_id == entity_id)
            .cloned()
            .collect()
    }

    /// Query events filtered by `event_type` and occurring after `since_ms`.
    pub fn query_events(&self, event_type: &str, since_ms: u64) -> Vec<WorldEvent> {
        let inner = lock_guard(&self.inner);

        inner
            .events
            .iter()
            .filter(|e| e.event_type == event_type && e.timestamp_ms >= since_ms)
            .cloned()
            .collect()
    }

    // -- Snapshot ----------------------------------------------------------

    /// Capture a point-in-time snapshot of the world model's state.
    pub fn snapshot(&self) -> StateSnapshot {
        let mut inner = lock_guard(&self.inner);
        let now = now_ms();

        let snapshot_id = format!("snap_{}", inner.next_snapshot_id);
        inner.next_snapshot_id += 1;

        StateSnapshot {
            snapshot_id,
            entities: inner.entities.clone(),
            relationships: inner.relationships.clone(),
            captured_ms: now,
        }
    }

    // -- Maintenance -------------------------------------------------------

    /// Remove entities, relationships, and events that are older than the
    /// retention period. Returns the number of entities that were removed.
    pub fn cleanup_stale(&self) -> usize {
        let mut inner = lock_guard(&self.inner);
        let now = now_ms();
        let cutoff = now.saturating_sub(inner.config.state_retention_ms);

        let before = inner.entities.len();

        // Collect IDs of stale entities.
        let stale_ids: std::collections::HashSet<String> = inner
            .entities
            .iter()
            .filter(|e| e.last_seen_ms < cutoff)
            .map(|e| e.id.clone())
            .collect();

        // Remove stale entities.
        inner.entities.retain(|e| e.last_seen_ms >= cutoff);

        // Remove relationships that referenced stale entities.
        inner
            .relationships
            .retain(|r| !stale_ids.contains(&r.source_id) && !stale_ids.contains(&r.target_id));

        // Remove stale events.
        inner.events.retain(|e| e.timestamp_ms >= cutoff);

        if !stale_ids.is_empty() {
            inner.last_update_ms = now;
        }

        before - inner.entities.len()
    }

    // -- Causal inference & prediction ------------------------------------

    /// Record a causal link between two entities.
    ///
    /// Increments observation_count if the link already exists, otherwise creates a new one.
    /// Updates confidence based on observation frequency.
    pub fn record_causal_link(
        &self,
        cause: &str,
        effect: &str,
        delay_ms: f64,
        context_tags: Vec<String>,
    ) {
        let mut inner = lock_guard(&self.inner);
        if let Some(existing) = inner
            .causal_links
            .iter_mut()
            .find(|l| l.cause_entity_id == cause && l.effect_entity_id == effect)
        {
            existing.observation_count += 1;
            existing.avg_delay_ms =
                (existing.avg_delay_ms * (existing.observation_count - 1) as f64 + delay_ms)
                    / existing.observation_count as f64;
            existing.confidence =
                (existing.confidence + 1.0 / (existing.observation_count as f64 + 1.0)).min(1.0);
            // Merge context tags
            for tag in context_tags {
                if !existing.context_tags.contains(&tag) {
                    existing.context_tags.push(tag);
                }
            }
        } else {
            inner.causal_links.push(CausalLink {
                cause_entity_id: cause.to_string(),
                effect_entity_id: effect.to_string(),
                confidence: 0.3, // Initial low confidence
                observation_count: 1,
                avg_delay_ms: delay_ms,
                context_tags,
            });
        }
        inner.last_update_ms = now_ms();
    }

    /// Predict the outcome of taking `action` on `target_entity`.
    ///
    /// Returns a list of predicted effects with confidence scores.
    pub fn predict_outcome(&self, action: &str, target_entity: &str) -> Vec<Prediction> {
        let inner = lock_guard(&self.inner);
        let mut results: Vec<Prediction> = Vec::new();

        // Helper to convert entity properties to a serde_json Value
        let props_to_json =
            |props: &std::collections::HashMap<String, String>| -> serde_json::Value {
                serde_json::json!(props)
            };

        // Find causal links where the action matches a known cause
        for link in &inner.causal_links {
            if link.cause_entity_id == action || link.cause_entity_id.contains(action) {
                // Find the effect entity's current state
                let effect_state = inner
                    .entities
                    .iter()
                    .find(|e| e.id == link.effect_entity_id)
                    .map(|e| props_to_json(&e.properties))
                    .unwrap_or_else(|| serde_json::json!({}));

                results.push(Prediction {
                    entity_id: link.effect_entity_id.clone(),
                    predicted_attributes: effect_state,
                    confidence: link.confidence,
                    horizon_ms: link.avg_delay_ms as u64,
                    based_on: format!("causal:{}→{}", link.cause_entity_id, link.effect_entity_id),
                });
            }
        }

        // If no causal link found, do a similarity-based prediction
        if results.is_empty() {
            // Find entities similar to target and see what actions affected them
            let similar_entities: Vec<&WorldEntity> = inner
                .entities
                .iter()
                .filter(|e| e.id.contains(target_entity) || target_entity.contains(&e.id))
                .collect();

            for similar in &similar_entities {
                for link in &inner.causal_links {
                    if link.cause_entity_id.contains(&similar.id)
                        || similar.id.contains(&link.cause_entity_id)
                    {
                        let effect_state = inner
                            .entities
                            .iter()
                            .find(|e| e.id == link.effect_entity_id)
                            .map(|e| props_to_json(&e.properties))
                            .unwrap_or_else(|| serde_json::json!({}));

                        results.push(Prediction {
                            entity_id: link.effect_entity_id.clone(),
                            predicted_attributes: effect_state,
                            confidence: link.confidence * 0.7, // Lower confidence for similarity-based
                            horizon_ms: link.avg_delay_ms as u64,
                            based_on: format!(
                                "similarity:{}→{}",
                                link.cause_entity_id, link.effect_entity_id
                            ),
                        });
                    }
                }
            }
        }

        results
    }

    /// Analyze recent events to discover new causal patterns.
    ///
    /// Scans events within `window_ms` and looks for recurring (action → outcome) patterns.
    pub fn discover_causal_patterns(&self, window_ms: u64) -> Vec<String> {
        let now = now_ms();
        let cutoff = now.saturating_sub(window_ms);

        // Collect event data while holding the lock, then drop it before mutation
        let event_data: Vec<(String, std::collections::HashMap<String, String>)> = {
            let inner = lock_guard(&self.inner);
            inner
                .events
                .iter()
                .filter(|e| e.timestamp_ms >= cutoff)
                .map(|e| (e.source.clone(), e.payload.clone()))
                .collect()
        };

        let mut discoveries = Vec::new();

        // Group events by source
        let mut by_source: std::collections::HashMap<
            String,
            Vec<std::collections::HashMap<String, String>>,
        > = std::collections::HashMap::new();
        for (source, payload) in event_data {
            by_source.entry(source).or_default().push(payload);
        }

        // Now work with cloned data only — borrow checker is happy
        let mut new_links: Vec<CausalLink> = Vec::new();

        for (source, events) in &by_source {
            if events.len() >= 3 {
                let entity_changes: Vec<&str> = events
                    .iter()
                    .filter_map(|payload| payload.get("target_entity").map(|v| v.as_str()))
                    .collect();

                if entity_changes.len() >= 3 {
                    new_links.push(CausalLink {
                        cause_entity_id: source.clone(),
                        effect_entity_id: entity_changes[0].to_string(),
                        confidence: 0.3,
                        observation_count: 1,
                        avg_delay_ms: 1000.0,
                        context_tags: vec!["discovered".to_string()],
                    });
                    discoveries.push(format!(
                        "Discovered causal pattern: {} → {} ({} events)",
                        source,
                        entity_changes[0],
                        events.len()
                    ));
                }
            }
        }

        // Push all discovered links into inner state
        if !new_links.is_empty() {
            if let Ok(mut inner) = self.inner.lock() {
                inner.causal_links.extend(new_links);
            }
        }

        discoveries
    }

    // -- Profile -----------------------------------------------------------

    /// Return a summary profile of the world model's current state.
    pub fn profile(&self) -> WorldModelProfile {
        let inner = lock_guard(&self.inner);
        let now = now_ms();
        let cutoff = now.saturating_sub(inner.config.state_retention_ms);

        let total_entities = inner.entities.len();
        let total_relationships = inner.relationships.len();
        let total_events = inner.events.len();

        let avg_entity_confidence = if total_entities > 0 {
            inner.entities.iter().map(|e| e.confidence).sum::<f64>() / total_entities as f64
        } else {
            0.0
        };

        let stale_entity_count = inner
            .entities
            .iter()
            .filter(|e| e.last_seen_ms < cutoff)
            .count();

        WorldModelProfile {
            total_entities,
            total_relationships,
            total_events,
            avg_entity_confidence,
            last_update_ms: inner.last_update_ms,
            stale_entity_count,
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

    // -----------------------------------------------------------------------
    // Test 1: New model is empty.
    // -----------------------------------------------------------------------
    #[test]
    fn test_new_model_empty() {
        let wm = WorldModel::new(WorldModelConfig::default());
        let p = wm.profile();

        assert_eq!(p.total_entities, 0);
        assert_eq!(p.total_relationships, 0);
        assert_eq!(p.total_events, 0);
        assert!((p.avg_entity_confidence - 0.0).abs() < 1e-9);
        assert_eq!(p.stale_entity_count, 0);
        assert!(p.last_update_ms > 0);
    }

    // -----------------------------------------------------------------------
    // Test 2: Register an entity and verify it's stored.
    // -----------------------------------------------------------------------
    #[test]
    fn test_register_entity() {
        let wm = WorldModel::new(test_config());

        let id = wm
            .register_entity("Sensor-1", EntityType::Resource)
            .unwrap();

        // Verify the ID is non-empty.
        assert!(!id.is_empty());
        assert!(id.starts_with("ent_"));

        let entities = wm.query_entities(None, 0.0);
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].id, id);
        assert_eq!(entities[0].name, "Sensor-1");
        assert_eq!(entities[0].entity_type, EntityType::Resource);
        assert!((entities[0].confidence - 1.0).abs() < 1e-9);
    }

    // -----------------------------------------------------------------------
    // Test 3: Register duplicate entity (same name + type) fails.
    // -----------------------------------------------------------------------
    #[test]
    fn test_register_duplicate_entity() {
        let wm = WorldModel::new(test_config());

        wm.register_entity("Dup", EntityType::Agent).unwrap();
        let result = wm.register_entity("Dup", EntityType::Agent);

        assert!(result.is_err());
        assert_eq!(wm.query_entities(None, 0.0).len(), 1);
    }

    // -----------------------------------------------------------------------
    // Test 4: Update entity properties.
    // -----------------------------------------------------------------------
    #[test]
    fn test_update_entity() {
        let wm = WorldModel::new(test_config());

        let id = wm.register_entity("Updatable", EntityType::System).unwrap();

        let mut props = HashMap::new();
        props.insert("version".to_string(), "2.1.0".to_string());
        props.insert("status".to_string(), "online".to_string());

        wm.update_entity(&id, props).unwrap();

        let entities = wm.query_entities(None, 0.0);
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].properties.get("version").unwrap(), "2.1.0");
        assert_eq!(entities[0].properties.get("status").unwrap(), "online");
    }

    // -----------------------------------------------------------------------
    // Test 5: Remove an entity (and its relationships).
    // -----------------------------------------------------------------------
    #[test]
    fn test_remove_entity() {
        let wm = WorldModel::new(test_config());

        let id_a = wm.register_entity("Alice", EntityType::Agent).unwrap();
        let id_b = wm.register_entity("Bob", EntityType::Agent).unwrap();

        // Create a relationship.
        wm.record_relationship(&id_a, &id_b, RelationshipType::CommunicatesWith, 0.8)
            .unwrap();

        assert_eq!(wm.query_entities(None, 0.0).len(), 2);
        assert_eq!(wm.query_relationships(&id_a).len(), 1);

        // Remove Alice.
        wm.remove_entity(&id_a).unwrap();

        assert_eq!(wm.query_entities(None, 0.0).len(), 1);
        // The relationship should also be gone.
        assert_eq!(wm.query_relationships(&id_a).len(), 0);
        assert_eq!(wm.query_relationships(&id_b).len(), 0);
    }

    // -----------------------------------------------------------------------
    // Test 6: Record a relationship between two entities.
    // -----------------------------------------------------------------------
    #[test]
    fn test_record_relationship() {
        let wm = WorldModel::new(test_config());

        let id_x = wm
            .register_entity("Service-X", EntityType::Service)
            .unwrap();
        let id_y = wm.register_entity("DB-Y", EntityType::DataStore).unwrap();

        wm.record_relationship(&id_x, &id_y, RelationshipType::DependsOn, 0.95)
            .unwrap();

        let rels = wm.query_relationships(&id_x);
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].source_id, id_x);
        assert_eq!(rels[0].target_id, id_y);
        assert_eq!(rels[0].rel_type, RelationshipType::DependsOn);
        assert!((rels[0].weight - 0.95).abs() < 1e-9);
    }

    // -----------------------------------------------------------------------
    // Test 7: Query relationships for a specific entity.
    // -----------------------------------------------------------------------
    #[test]
    fn test_query_relationships() {
        let wm = WorldModel::new(test_config());

        let id_a = wm.register_entity("A", EntityType::Agent).unwrap();
        let id_b = wm.register_entity("B", EntityType::Agent).unwrap();
        let id_c = wm.register_entity("C", EntityType::Agent).unwrap();

        wm.record_relationship(&id_a, &id_b, RelationshipType::CommunicatesWith, 0.5)
            .unwrap();
        wm.record_relationship(&id_a, &id_c, RelationshipType::Manages, 0.7)
            .unwrap();
        wm.record_relationship(&id_b, &id_c, RelationshipType::DependsOn, 0.3)
            .unwrap();

        // A has two relationships (source for both).
        let rels_a = wm.query_relationships(&id_a);
        assert_eq!(rels_a.len(), 2);

        // C has two relationships (target for both).
        let rels_c = wm.query_relationships(&id_c);
        assert_eq!(rels_c.len(), 2);

        // B has one as source, one as target = 2 total.
        let rels_b = wm.query_relationships(&id_b);
        assert_eq!(rels_b.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Test 8: Record an event.
    // -----------------------------------------------------------------------
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

    // -----------------------------------------------------------------------
    // Test 9: Query events by type and time range.
    // -----------------------------------------------------------------------
    #[test]
    fn test_query_events() {
        let wm = WorldModel::new(test_config());

        let id = wm.register_entity("Source", EntityType::System).unwrap();

        let mut p1 = HashMap::new();
        p1.insert("level".to_string(), "info".to_string());
        let mut p2 = HashMap::new();
        p2.insert("level".to_string(), "warn".to_string());
        let mut p3 = HashMap::new();
        p3.insert("level".to_string(), "error".to_string());

        let t0 = now_ms();

        wm.record_event("log", &id, p1).unwrap();
        wm.record_event("log", &id, p2).unwrap();
        wm.record_event("alert", &id, p3).unwrap();

        // Query "log" events since t0.
        let log_events = wm.query_events("log", t0);
        assert_eq!(log_events.len(), 2);

        // Query "alert" events since t0.
        let alert_events = wm.query_events("alert", t0);
        assert_eq!(alert_events.len(), 1);

        // Query with future timestamp returns nothing.
        let future = now_ms() + 10_000;
        let empty = wm.query_events("log", future);
        assert!(empty.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 10: Snapshot captures the current state.
    // -----------------------------------------------------------------------
    #[test]
    fn test_snapshot_captures_state() {
        let wm = WorldModel::new(test_config());

        let id_a = wm.register_entity("Entity-A", EntityType::Agent).unwrap();
        let id_b = wm
            .register_entity("Entity-B", EntityType::Resource)
            .unwrap();

        wm.record_relationship(&id_a, &id_b, RelationshipType::Owns, 1.0)
            .unwrap();

        let snap = wm.snapshot();

        assert!(snap.snapshot_id.starts_with("snap_"));
        assert_eq!(snap.entities.len(), 2);
        assert_eq!(snap.relationships.len(), 1);
        assert!(snap.captured_ms > 0);
    }

    // -----------------------------------------------------------------------
    // Test 11: Cleanup stale entities, relationships, and events.
    // -----------------------------------------------------------------------
    #[test]
    fn test_cleanup_stale() {
        // For a direct test of staleness, we use a config with a short retention
        // and manually set an entity's last_seen_ms to a very old value.
        let mut config = test_config();
        config.state_retention_ms = 1_000; // 1 second
        let wm = WorldModel::new(config);

        // Register entities inside the world model.
        let id_old = wm
            .register_entity("OldEntity", EntityType::External)
            .unwrap();
        let id_fresh = wm
            .register_entity("FreshEntity", EntityType::Agent)
            .unwrap();

        // Manually set the old entity's last_seen_ms to a very old value.
        {
            let mut inner = wm.inner.lock().unwrap();
            if let Some(old) = inner.entities.iter_mut().find(|e| e.id == id_old) {
                old.last_seen_ms = 1; // ancient
            }
            // Fresh entity gets the current timestamp from register_entity.
        }

        // Create a relationship between them.
        wm.record_relationship(&id_old, &id_fresh, RelationshipType::Unknown, 0.5)
            .unwrap();

        assert_eq!(wm.query_entities(None, 0.0).len(), 2);
        assert_eq!(wm.query_relationships(&id_old).len(), 1);

        // Cleanup stale (entity with last_seen_ms = 1 should be stale).
        let pruned = wm.cleanup_stale();
        assert_eq!(pruned, 1);

        let remaining = wm.query_entities(None, 0.0);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, id_fresh);

        // The relationship should have been cleaned up too.
        assert!(wm.query_relationships(&id_old).is_empty());
        assert!(wm.query_relationships(&id_fresh).is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 12: Profile accurately reflects the registered state.
    // -----------------------------------------------------------------------
    #[test]
    fn test_profile_reflects_state() {
        let wm = WorldModel::new(test_config());

        // Start empty.
        let p0 = wm.profile();
        assert_eq!(p0.total_entities, 0);
        assert_eq!(p0.total_relationships, 0);
        assert_eq!(p0.total_events, 0);
        assert!((p0.avg_entity_confidence - 0.0).abs() < 1e-9);
        assert_eq!(p0.stale_entity_count, 0);

        // Register entities.
        let id_a = wm.register_entity("Alpha", EntityType::Agent).unwrap();
        let id_b = wm.register_entity("Beta", EntityType::Service).unwrap();
        let _id_c = wm.register_entity("Gamma", EntityType::DataStore).unwrap();
        {
            let p = wm.profile();
            assert_eq!(p.total_entities, 3);
        }

        // Add a relationship.
        wm.record_relationship(&id_a, &id_b, RelationshipType::CommunicatesWith, 0.9)
            .unwrap();
        {
            let p = wm.profile();
            assert_eq!(p.total_relationships, 1);
        }

        // Record an event.
        wm.record_event("deploy", &id_a, HashMap::new()).unwrap();
        {
            let p = wm.profile();
            assert_eq!(p.total_events, 1);
        }

        // Final profile check.
        let p = wm.profile();
        assert_eq!(p.total_entities, 3);
        assert_eq!(p.total_relationships, 1);
        assert_eq!(p.total_events, 1);
        assert!(p.avg_entity_confidence > 0.0);
        assert!(p.last_update_ms > 0);
        assert_eq!(p.stale_entity_count, 0); // nothing is stale yet
    }
}
