//! Type definitions for the world model pipeline.
//!
//! This file contains all data structures, enums, and configuration types
//! used by the world model. It is a sub-module of `world_model`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Causal inference
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

/// A snapshot of an entity's properties at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityStateSnapshot {
    /// The entity whose state was captured.
    pub entity_id: String,
    /// The entity's properties at this point.
    pub properties: HashMap<String, String>,
    /// Epoch millisecond when the snapshot was taken.
    pub timestamp_ms: u64,
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
// Entity classification
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
