//! Type definitions for the world model pipeline.
//!
//! This file contains all data structures, enums, and configuration types
//! used by the world model. It is a sub-module of `world_model`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
}

impl Default for WorldModelConfig {
    fn default() -> Self {
        Self {
            max_entities: 1000,
            max_events: 5000,
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

/// A read-only snapshot of the world model state, surfaced via the
/// capability-bus profile / status endpoints so the accumulated world state
/// is observable instead of write-only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldModelProfile {
    /// Number of tracked entities.
    pub entities: usize,
    /// Number of recorded events.
    pub events: usize,
    /// Unix millis of the last state update.
    pub last_update_ms: u64,
    /// Entity count grouped by entity type (debug-name → count).
    pub entities_by_type: HashMap<String, usize>,
    /// Most recent event types, most recent first (bounded).
    pub recent_event_types: Vec<String>,
}
