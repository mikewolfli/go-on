//! Type definitions for the world model pipeline.
//!
//! This file contains all data structures, enums, and configuration types
//! used by the world model. It is a sub-module of `world_model`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Type aliases for complex types used in causal analysis
// ---------------------------------------------------------------------------

/// Event data: (source, payload, timestamp_ms)
pub(crate) type EventData = (String, HashMap<String, String>, u64);
/// Collection of events grouped by source (payload, timestamp_ms).
/// The source is the map key, so only a 2-tuple is stored.
pub(crate) type SourceEvents = HashMap<String, Vec<(HashMap<String, String>, u64)>>;

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

/// The type of a causal path, describing how multiple causes relate to an outcome.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CausalPathType {
    /// A single, direct causal link (A → B).
    Direct,
    /// All listed causes must be present for the effect to occur (A ∧ B → C).
    And(Vec<String>),
    /// Any one of the listed causes can trigger the effect (A ∨ B → C).
    Or(Vec<String>),
}

/// A chain of causal links discovered from correlation analysis.
///
/// Supports branching paths (`And`/`Or`), feedback loop detection,
/// and probabilistic confidence decay over chain length.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CausalChain {
    /// The ordered sequence of causal links forming the chain.
    pub links: Vec<CausalLink>,
    /// Aggregate confidence for the entire chain (0.0 – 1.0),
    /// computed with decay over chain length.
    pub confidence: f64,
    /// Whether this chain is a linear path or a branching path.
    pub path_type: CausalPathType,
    /// Set to `true` when the chain forms a cycle (A→B→C→A).
    pub is_feedback_loop: bool,
    /// Number of links in this chain.
    pub chain_length: usize,
}

// ---------------------------------------------------------------------------
// Causal Reasoner — state-tracking correlation engine
// ---------------------------------------------------------------------------

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

/// A discovered correlation between two property changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Correlation {
    /// The entity whose property change is the suspected cause.
    pub cause_entity: String,
    /// The property on the cause entity that changed.
    pub cause_property: String,
    /// The entity whose property change is the suspected effect.
    pub effect_entity: String,
    /// The property on the effect entity that changed.
    pub effect_property: String,
    /// How many times this co-occurrence has been observed.
    pub co_occurrence_count: u64,
    /// Confidence score (0.0 – 1.0) based on co-occurrence frequency.
    pub confidence: f64,
    /// Average time delta (ms) between cause and effect observations.
    pub avg_time_delta_ms: i64,
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
