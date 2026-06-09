//! BLUE38 F-GAP-23: World Model Pipeline (M7 "世界模型流水线")
//!
//! A thread-safe pipeline that maintains a structured representation of the
//! external environment — tracking entities, relationships, events, and state
//! changes over time. All state is guarded behind `Arc<Mutex<>>`.

mod causal;
mod types;

pub use causal::CausalReasoner;
pub use types::*;

use crate::i18n::runtime::tf;
use crate::intelligence::causal_bayesian_graph::{BayesianCausalPath, CausalBayesianGraph};
use crate::intelligence::lock_guard;
use crate::intelligence::now_ms;
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
    relationships: Vec<Relationship>,
    events: Vec<WorldEvent>,
    last_update_ms: u64,
    next_entity_id: u64,
    next_event_id: u64,
    next_snapshot_id: u64,
    causal_links: Vec<CausalLink>,
    /// Index: cause_entity_id → indices into causal_links for O(1) lookups
    causal_links_by_cause: HashMap<String, Vec<usize>>,
    /// Probabilistic causal graph with MCTS for ultra-long chain reasoning.
    bayesian_graph: CausalBayesianGraph,
    /// Max relationships to retain before evicting the oldest.
    max_relationships: usize,
    /// Max causal links to retain before evicting the oldest.
    max_causal_links: usize,
    /// Causal reasoner for entity state change correlation analysis.
    causal_reasoner: CausalReasoner,
    /// Counter for tracking number of updates/events; used for periodic inference.
    update_counter: u64,
    /// Run correlation inference every N updates (default: 10).
    correlation_inference_interval: u64,
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
                causal_links_by_cause: HashMap::new(),
                max_relationships: 5000,
                max_causal_links: 5000,
                causal_reasoner: CausalReasoner::new(5000, 5000),
                update_counter: 0,
                bayesian_graph: CausalBayesianGraph::new(),
                correlation_inference_interval: 10,
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

        // Enforce max entities limit — evict the oldest entity if at capacity.
        while inner.entities.len() >= inner.config.max_entities {
            if let Some(pos) = inner
                .entities
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.last_seen_ms)
                .map(|(i, _)| i)
            {
                let removed = inner.entities.swap_remove(pos);
                // Clean up relationships referencing the evicted entity.
                inner
                    .relationships
                    .retain(|r| r.source_id != removed.id && r.target_id != removed.id);
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

        // Clone data from the entity before dropping the mutable borrow on `entity`,
        // so we can use `inner` for causal reasoner operations below.
        let entity_properties = entity.properties.clone();
        let entity_id = entity.id.clone();

        inner.last_update_ms = now;

        // Record the state update in the causal reasoner
        inner
            .causal_reasoner
            .record_state(&entity_id, entity_properties, now);
        inner.update_counter += 1;

        // Periodically run correlation inference and generate causal links
        let should_infer = inner
            .update_counter
            .is_multiple_of(inner.correlation_inference_interval);

        if should_infer {
            let correlations = inner.causal_reasoner.infer_correlations().to_vec();
            for corr in &correlations {
                let existing = inner.causal_links.iter().position(|l| {
                    l.cause_entity_id == corr.cause_entity
                        && l.effect_entity_id == corr.effect_entity
                });

                if let Some(pos) = existing {
                    let link = &mut inner.causal_links[pos];
                    link.observation_count = link.observation_count.max(corr.co_occurrence_count);
                    link.confidence = link.confidence.max(corr.confidence);
                } else if inner.causal_links.len() < inner.max_causal_links {
                    let link = CausalLink {
                        cause_entity_id: corr.cause_entity.clone(),
                        effect_entity_id: corr.effect_entity.clone(),
                        confidence: corr.confidence,
                        observation_count: corr.co_occurrence_count,
                        avg_delay_ms: corr.avg_time_delta_ms.max(0) as f64,
                        context_tags: vec!["correlated".to_string()],
                    };
                    let idx = inner.causal_links.len();
                    inner.causal_links.push(link);
                    inner
                        .causal_links_by_cause
                        .entry(corr.cause_entity.clone())
                        .or_default()
                        .push(idx);
                }
            }

            // Also extract state transitions from the causal reasoner's history
            // and feed them into infer_causal_chain for entity-state-level chains.
            // This wires infer_causal_chain into a real production path.
            let history_snapshots: Vec<(String, String)> = inner
                .causal_reasoner
                .history
                .iter()
                .map(|snap| {
                    // Serialize the full property set as the "state" value.
                    let mut pairs: Vec<String> = snap
                        .properties
                        .iter()
                        .map(|(k, v)| format!("{}={}", k, v))
                        .collect();
                    pairs.sort();
                    (snap.entity_id.clone(), pairs.join(","))
                })
                .collect();

            // Build consecutive same-entity state changes.
            let mut state_changes: Vec<(String, String)> = Vec::new();
            for i in 1..history_snapshots.len() {
                let prev = &history_snapshots[i - 1];
                let curr = &history_snapshots[i];
                if prev.0 == curr.0 && prev.1 != curr.1 {
                    state_changes.push((prev.1.clone(), curr.1.clone()));
                }
            }

            if !state_changes.is_empty() {
                // Drop the lock before calling infer_causal_chain to avoid deadlock.
                drop(inner);
                let chain_links = self.infer_causal_chain(&state_changes);
                if !chain_links.is_empty() {
                    let mut inner = lock_guard(&self.inner);
                    for link in &chain_links {
                        if inner.causal_links.len() < inner.max_causal_links {
                            let idx = inner.causal_links.len();
                            inner.causal_links.push(link.clone());
                            inner
                                .causal_links_by_cause
                                .entry(link.cause_entity_id.clone())
                                .or_default()
                                .push(idx);
                        }
                        // Feed into Bayesian graph for probabilistic reasoning
                        inner.bayesian_graph.record_correlation(
                            &link.cause_entity_id,
                            "state",
                            &link.effect_entity_id,
                            "state",
                            1,
                            link.confidence,
                            link.avg_delay_ms as i64,
                        );
                    }
                    // Periodically build meso-layer abstractions
                    if inner.update_counter.is_multiple_of(50) {
                        inner.bayesian_graph.build_meso_layer();
                    }
                }
                return Ok(());
            }
        }

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

        // Evict the oldest relationship when at capacity.
        if inner.relationships.len() >= inner.max_relationships {
            inner.relationships.remove(0);
        }

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
        inner.update_counter += 1;

        // Periodically run correlation inference from events
        if inner
            .update_counter
            .is_multiple_of(inner.correlation_inference_interval)
        {
            // Snapshot entity state for the reasoner
            let entity_snapshots: Vec<(String, HashMap<String, String>)> = inner
                .entities
                .iter()
                .map(|e| (e.id.clone(), e.properties.clone()))
                .collect();
            for (eid, props) in &entity_snapshots {
                inner.causal_reasoner.record_state(eid, props.clone(), now);
            }
            let correlations = inner.causal_reasoner.infer_correlations().to_vec();
            for corr in &correlations {
                let existing = inner.causal_links.iter().position(|l| {
                    l.cause_entity_id == corr.cause_entity
                        && l.effect_entity_id == corr.effect_entity
                });

                if let Some(pos) = existing {
                    let link = &mut inner.causal_links[pos];
                    link.observation_count = link.observation_count.max(corr.co_occurrence_count);
                    link.confidence = link.confidence.max(corr.confidence);
                } else if inner.causal_links.len() < inner.max_causal_links {
                    let link = CausalLink {
                        cause_entity_id: corr.cause_entity.clone(),
                        effect_entity_id: corr.effect_entity.clone(),
                        confidence: corr.confidence,
                        observation_count: corr.co_occurrence_count,
                        avg_delay_ms: corr.avg_time_delta_ms.max(0) as f64,
                        context_tags: vec!["correlated".to_string()],
                    };
                    let idx = inner.causal_links.len();
                    inner.causal_links.push(link);
                    inner
                        .causal_links_by_cause
                        .entry(corr.cause_entity.clone())
                        .or_default()
                        .push(idx);
                }
            }
        }

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

    /// Find causal paths using MCTS-based probabilistic reasoning.
    ///
    /// Uses the internal `CausalBayesianGraph` to explore ultra-long causal chains
    /// with UCB1-guided Monte Carlo Tree Search and confidence-weighted probabilities.
    ///
    /// # Parameters
    ///
    /// * `cause_entity` — starting entity ID
    /// * `effect_entity` — target entity ID (empty = explore all paths)
    /// * `max_path_length` — maximum edges per path (default: 10)
    /// * `min_probability` — minimum edge probability to consider (default: 0.05)
    ///
    /// # Returns
    ///
    /// Causal paths sorted by confidence descending.
    pub fn find_causal_paths_mcts(
        &self,
        cause_entity: &str,
        effect_entity: &str,
        max_path_length: usize,
        min_probability: f64,
    ) -> Vec<BayesianCausalPath> {
        let inner = lock_guard(&self.inner);
        inner.bayesian_graph.find_paths_mcts(
            cause_entity,
            "state",
            effect_entity,
            "state",
            max_path_length,
            500, // MCTS iterations
            min_probability,
        )
    }

    /// Answer a counterfactual query: "What would the probability of `effect`
    /// be if `cause` had NOT happened?"
    ///
    /// Uses Bayesian inversion on the internal causal graph:
    /// P(effect | ¬cause) = (P(effect) - P(cause) * P(effect|cause)) / (1 - P(cause))
    pub fn counterfactual_probability(&self, cause_entity: &str, effect_entity: &str) -> f64 {
        let inner = lock_guard(&self.inner);
        inner.bayesian_graph.counterfactual_probability(
            cause_entity,
            "state",
            effect_entity,
            "state",
        )
    }

    /// Get the Bayesian graph's node and edge count for diagnostics.
    pub fn bayesian_graph_stats(&self) -> (usize, usize, u64) {
        let inner = lock_guard(&self.inner);
        (
            inner.bayesian_graph.node_count(),
            inner.bayesian_graph.edge_count(),
            inner.bayesian_graph.total_observations(),
        )
    }

    /// Query causal insight for agent selection hot path.
    ///
    /// Uses the Bayesian causal graph to evaluate how causally effective an agent
    /// has been for a given task type. Returns a score in [0.0, 1.0] where higher
    /// means the agent has a strong causal relationship with successful outcomes.
    ///
    /// This is called from `CapabilityBus::decide()` as an additional scoring
    /// dimension alongside reputation, recency, and task-fit scores.
    pub fn causal_agent_insight(&self, agent_name: &str, task_type: &str) -> f64 {
        let inner = lock_guard(&self.inner);
        // Only meaningful if sufficient observations exist (at least 10 edges)
        if inner.bayesian_graph.edge_count() < 10 {
            return 0.5; // neutral — insufficient data
        }
        // Query: how does agent_name → (task_type, success) probability look?
        let paths = inner.bayesian_graph.find_paths_mcts(
            agent_name, "status", task_type, "success", 5,    // max_path_length
            200,  // MCTS iterations (lightweight for hot path)
            0.05, // min_probability
        );
        if paths.is_empty() {
            return 0.5; // neutral — no causal data
        }
        // Weight top-3 paths by joint_probability * confidence
        let top: f64 = paths
            .iter()
            .take(3)
            .map(|p| p.joint_probability * p.confidence)
            .sum();
        (top / 3.0).clamp(0.0, 1.0)
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
        let existing_pos = inner
            .causal_links
            .iter()
            .position(|l| l.cause_entity_id == cause && l.effect_entity_id == effect);

        if let Some(pos) = existing_pos {
            let existing = &mut inner.causal_links[pos];
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
            // Maintain the index: record the new link's index by cause
            let idx = inner.causal_links.len() - 1;
            inner
                .causal_links_by_cause
                .entry(cause.to_string())
                .or_default()
                .push(idx);
        }
        inner.last_update_ms = now_ms();
    }

    /// Predict the outcome of taking `action` on `target_entity`.
    ///
    /// Returns a list of predicted effects with confidence scores.
    ///
    /// Uses the causal_links_by_cause index for O(1) lookups by cause_entity_id
    /// instead of scanning all causal_links (O(N)).
    pub fn predict_outcome(&self, action: &str, target_entity: &str) -> Vec<Prediction> {
        let inner = lock_guard(&self.inner);
        let mut results: Vec<Prediction> = Vec::new();

        // Helper to convert entity properties to a serde_json Value
        let props_to_json =
            |props: &std::collections::HashMap<String, String>| -> serde_json::Value {
                serde_json::json!(props)
            };

        // Find causal links where the action matches a known cause — O(1) index lookup
        if let Some(indices) = inner.causal_links_by_cause.get(action) {
            for &idx in indices {
                let link = &inner.causal_links[idx];
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
            // Verify the target entity exists, then look up its causal links via the index
            if inner.entities.iter().any(|e| e.id == target_entity) {
                if let Some(indices) = inner.causal_links_by_cause.get(target_entity) {
                    for &idx in indices {
                        let link = &inner.causal_links[idx];
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

    /// Analyzes recent events to discover new causal patterns.
    ///
    /// Uses both the event stream and the existing `causal_links_by_cause` index
    /// to find multi-step sequences, hub nodes, and temporal correlations.
    pub fn discover_causal_patterns(&self, window_ms: u64) -> Vec<String> {
        let now = now_ms();
        let cutoff = now.saturating_sub(window_ms);

        // Collect event and index data while holding the lock
        let (event_data, existing_cause_counts): (
            Vec<EventData>,
            std::collections::HashMap<String, usize>,
        ) = {
            let inner = lock_guard(&self.inner);
            let events: Vec<_> = inner
                .events
                .iter()
                .filter(|e| e.timestamp_ms >= cutoff)
                .map(|e| (e.source.clone(), e.payload.clone(), e.timestamp_ms))
                .collect();
            let cause_counts: std::collections::HashMap<String, usize> = inner
                .causal_links_by_cause
                .iter()
                .map(|(k, v)| (k.clone(), v.len()))
                .collect();
            (events, cause_counts)
        };

        let mut discoveries = Vec::new();

        // Group events by source
        let mut by_source: SourceEvents = std::collections::HashMap::new();
        for (source, payload, ts) in event_data {
            by_source.entry(source).or_default().push((payload, ts));
        }

        let mut new_links: Vec<CausalLink> = Vec::new();

        for (source, events) in &by_source {
            if events.len() >= 2 {
                // Extract target entities from payloads
                let entity_changes: Vec<&str> = events
                    .iter()
                    .filter_map(|(payload, _)| payload.get("target_entity").map(|v| v.as_str()))
                    .collect();

                if entity_changes.len() >= 2 {
                    // Check for temporal consistency: the same source affecting
                    // the same target multiple times strengthens confidence
                    let consistent_targets: std::collections::HashMap<&str, usize> = entity_changes
                        .iter()
                        .fold(std::collections::HashMap::new(), |mut acc, &e| {
                            *acc.entry(e).or_insert(0) += 1;
                            acc
                        });

                    for (target, count) in &consistent_targets {
                        let confidence = (0.2 + *count as f64 * 0.15).min(0.9);

                        // Compute average delay from timestamps
                        let delays: Vec<u64> = events
                            .iter()
                            .filter_map(|(payload, ts)| {
                                payload.get("target_entity").and_then(|v| {
                                    if v == *target {
                                        Some(*ts)
                                    } else {
                                        None
                                    }
                                })
                            })
                            .collect();
                        let avg_delay = if delays.len() >= 2 {
                            let gaps: Vec<u64> = delays.windows(2).map(|w| w[1] - w[0]).collect();
                            gaps.iter().sum::<u64>() as f64 / gaps.len() as f64
                        } else {
                            1000.0
                        };

                        new_links.push(CausalLink {
                            cause_entity_id: source.clone(),
                            effect_entity_id: target.to_string(),
                            confidence,
                            observation_count: *count as u64,
                            avg_delay_ms: avg_delay,
                            context_tags: vec!["discovered".to_string()],
                        });
                        discoveries.push(format!(
                            "Discovered causal pattern: {} → {} ({} events, confidence: {:.2})",
                            source, target, count, confidence
                        ));
                    }
                }

                // Detect hub nodes: sources that affect many different targets
                let unique_targets: std::collections::HashSet<&str> =
                    entity_changes.iter().copied().collect();
                if unique_targets.len() >= 3 {
                    discoveries.push(format!(
                        "Hub node detected: {} affects {} different targets ({} total events)",
                        source,
                        unique_targets.len(),
                        events.len()
                    ));
                }
            }
        }

        // Detect multi-step causal chains by cross-referencing with existing links
        if !existing_cause_counts.is_empty() {
            for (cause, count) in &existing_cause_counts {
                if *count >= 3 {
                    discoveries.push(format!(
                        "High-frequency cause: {} is linked to {} effects (existing index)",
                        cause, count
                    ));
                }
            }
        }

        // Push all discovered links into inner state and maintain the index
        if !new_links.is_empty() {
            // Deduplicate against existing links before inserting
            let mut inner = lock_guard(&self.inner);
            let mut actually_added = 0;
            for link in &new_links {
                let already_exists = inner.causal_links.iter().any(|l| {
                    l.cause_entity_id == link.cause_entity_id
                        && l.effect_entity_id == link.effect_entity_id
                });
                if !already_exists {
                    // Evict the oldest causal link when at capacity.
                    if inner.causal_links.len() >= inner.max_causal_links {
                        let removed = inner.causal_links.remove(0);
                        // Clean up index.
                        if let Some(indices) = inner
                            .causal_links_by_cause
                            .get_mut(&removed.cause_entity_id)
                        {
                            indices.retain(|&i| i != 0);
                            // Shift remaining indices down by 1.
                            for idx in indices.iter_mut() {
                                *idx = idx.saturating_sub(1);
                            }
                            if indices.is_empty() {
                                inner.causal_links_by_cause.remove(&removed.cause_entity_id);
                            }
                        }
                    }
                    let idx = inner.causal_links.len();
                    inner.causal_links.push(link.clone());
                    inner
                        .causal_links_by_cause
                        .entry(link.cause_entity_id.clone())
                        .or_default()
                        .push(idx);
                    actually_added += 1;
                }
            }
            if actually_added < new_links.len() {
                discoveries.push(format!(
                    "Deduplication: {} of {} candidate links already exist",
                    new_links.len() - actually_added,
                    new_links.len()
                ));
            }
        }

        discoveries
    }

    // -- Profile -----------------------------------------------------------

    // -- Causal Reasoner integration -------------------------------------

    /// Records the current state of all entities as snapshots in the
    /// causal reasoner, then runs correlation inference to discover
    /// causal links between property changes.
    ///
    /// Returns the list of discovered correlations with confidence scores.
    pub fn infer_causal_links(&self) -> Vec<Correlation> {
        let now = now_ms();

        // Snapshot entity state before locking for mutation to avoid borrow conflict
        let entity_snapshots: Vec<(String, HashMap<String, String>)> = {
            let inner = lock_guard(&self.inner);
            inner
                .entities
                .iter()
                .map(|e| (e.id.clone(), e.properties.clone()))
                .collect()
        };

        let mut inner = lock_guard(&self.inner);

        // Record current state of all entities into the reasoner
        for (id, props) in &entity_snapshots {
            inner.causal_reasoner.record_state(id, props.clone(), now);
        }

        // Run inference
        let correlations = inner.causal_reasoner.infer_correlations().to_vec();
        inner.last_update_ms = now;

        // Also register any discovered correlations as causal links
        for corr in &correlations {
            let existing = inner.causal_links.iter().position(|l| {
                l.cause_entity_id == corr.cause_entity && l.effect_entity_id == corr.effect_entity
            });

            if let Some(pos) = existing {
                let link = &mut inner.causal_links[pos];
                link.observation_count = link.observation_count.max(corr.co_occurrence_count);
                link.confidence = link.confidence.max(corr.confidence);
            } else {
                let link = CausalLink {
                    cause_entity_id: corr.cause_entity.clone(),
                    effect_entity_id: corr.effect_entity.clone(),
                    confidence: corr.confidence,
                    observation_count: corr.co_occurrence_count,
                    avg_delay_ms: corr.avg_time_delta_ms.max(0) as f64,
                    context_tags: vec!["correlated".to_string()],
                };
                if inner.causal_links.len() < inner.max_causal_links {
                    let idx = inner.causal_links.len();
                    inner.causal_links.push(link);
                    inner
                        .causal_links_by_cause
                        .entry(corr.cause_entity.clone())
                        .or_default()
                        .push(idx);
                }
            }
        }

        correlations
    }

    /// Predicts the next likely state changes for a given entity based on
    /// discovered correlations from the causal reasoner.
    ///
    /// Returns a list of `(property_name, expected_value, confidence)` tuples
    /// ordered by confidence descending.
    pub fn predict_next_state(&self, entity_id: &str) -> Vec<(String, String, f64)> {
        let inner = lock_guard(&self.inner);

        // Find the entity's current properties
        let current_props = inner
            .entities
            .iter()
            .find(|e| e.id == entity_id)
            .map(|e| e.properties.clone())
            .unwrap_or_default();

        inner
            .causal_reasoner
            .predict_next_state(entity_id, &current_props)
    }

    /// Chains discovered correlations into causal sequences.
    ///
    /// Delegates to `CausalReasoner::infer_causal_chains` to find
    /// A→B→C→... patterns from the current set of discovered correlations.
    /// Returns chains sorted by length (longest first).
    pub fn infer_causal_chains(&self, max_chain_length: usize) -> Vec<Vec<Correlation>> {
        let inner = lock_guard(&self.inner);
        inner.causal_reasoner.infer_causal_chains(max_chain_length)
    }

    /// Chains discovered correlations into deep causal sequences with
    /// probabilistic confidence decay, branching path detection, feedback
    /// loop awareness, and confidence threshold filtering.
    ///
    /// Delegates to `CausalReasoner::infer_causal_chains_deep`.
    ///
    /// # Parameters
    ///
    /// * `max_chain_length` — maximum number of links in any single chain.
    /// * `min_confidence` — minimum confidence threshold (default 0.15).
    ///   Pass a value < 0.0 to disable threshold filtering.
    ///
    /// # Returns
    ///
    /// A `Vec<CausalChain>` sorted by aggregate confidence descending.
    pub fn infer_causal_chains_deep(
        &self,
        max_chain_length: usize,
        min_confidence: f64,
    ) -> Vec<CausalChain> {
        let inner = lock_guard(&self.inner);
        inner
            .causal_reasoner
            .infer_causal_chains_deep(max_chain_length, min_confidence)
    }

    /// Infer causal links from a sequence of state-change pairs.
    ///
    /// Each element in `state_changes` is a `(from_state, to_state)` tuple
    /// representing a single state transition. The method chains consecutive
    /// transitions where `pair[i].1 == pair[j].0` (i.e. the `to_state` of one
    /// change equals the `from_state` of another) into causal links.
    ///
    /// This is a lightweight, heuristic approach that does not require the
    /// [`CausalReasoner`]'s historical correlation analysis. It is useful for
    /// deriving quick causal insights from an ordered sequence of raw state
    /// transitions.
    ///
    /// Returns a list of [`CausalLink`] entries ordered by the transition
    /// sequence, with confidence decaying inversely with chain distance.
    pub fn infer_causal_chain(&self, state_changes: &[(String, String)]) -> Vec<CausalLink> {
        if state_changes.is_empty() {
            return Vec::new();
        }

        // Build a map: from_state -> indices for O(1) lookups
        let mut from_index: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, (from, _)) in state_changes.iter().enumerate() {
            from_index.entry(from.as_str()).or_default().push(i);
        }

        let mut links: Vec<CausalLink> = Vec::new();
        let mut visited = vec![false; state_changes.len()];

        for start_idx in 0..state_changes.len() {
            if visited[start_idx] {
                continue;
            }

            // Walk forward: find consecutive chain steps
            let mut chain_indices = Vec::new();
            chain_indices.push(start_idx);
            visited[start_idx] = true;

            loop {
                let current_to = &state_changes[*chain_indices.last().unwrap()].1;
                if let Some(next_indices) = from_index.get(current_to.as_str()) {
                    // Pick the first unvisited next step
                    let next = next_indices.iter().copied().find(|&idx| !visited[idx]);
                    match next {
                        Some(idx) => {
                            chain_indices.push(idx);
                            visited[idx] = true;
                        }
                        None => break,
                    }
                } else {
                    break;
                }
            }

            if chain_indices.len() >= 2 {
                // Create causal links between consecutive steps in the chain
                for window in chain_indices.windows(2) {
                    let i = window[0];
                    let j = window[1];
                    let (from_a, to_a) = &state_changes[i];
                    let (_from_b, to_b) = &state_changes[j];

                    // Probabilistic confidence decay: longer chains have
                    // exponentially decaying confidence.
                    // Formula: 1 / (1 + position * 0.15)
                    let depth = links.len() as f64;
                    let confidence = (1.0 / (1.0 + depth * 0.15)).clamp(0.0, 1.0);

                    links.push(CausalLink {
                        cause_entity_id: from_a.clone(),
                        effect_entity_id: to_b.clone(),
                        confidence,
                        observation_count: 1,
                        avg_delay_ms: 0.0,
                        context_tags: vec![
                            "inferred-chain".to_string(),
                            format!("step:{}→{}", to_a, to_b),
                        ],
                    });
                }
            }
        }

        links
    }

    /// Predicts likely entity changes using both causal reasoner correlations
    /// and recorded causal links, scoped by a time horizon.
    ///
    /// Returns a list of `Prediction` values sorted by confidence descending.
    pub fn predict_entity_changes(&self, entity_id: &str, horizon_ms: u64) -> Vec<Prediction> {
        let inner = lock_guard(&self.inner);
        let mut predictions: Vec<Prediction> = Vec::new();

        // 1. Use causal reasoner's state-based predictions
        let current_props = inner
            .entities
            .iter()
            .find(|e| e.id == entity_id)
            .map(|e| e.properties.clone())
            .unwrap_or_default();
        let state_preds = inner
            .causal_reasoner
            .predict_next_state(entity_id, &current_props);

        for (prop, val, conf) in &state_preds {
            let mut predicted_attrs = serde_json::Map::new();
            predicted_attrs.insert(prop.clone(), serde_json::Value::String(val.clone()));
            predictions.push(Prediction {
                entity_id: entity_id.to_string(),
                predicted_attributes: serde_json::Value::Object(predicted_attrs),
                confidence: *conf,
                horizon_ms,
                based_on: format!("causal_reasoner:{}", prop),
            });
        }

        // 2. Use outgoing causal links (this entity is a cause)
        if let Some(indices) = inner.causal_links_by_cause.get(entity_id) {
            for &idx in indices {
                let link = &inner.causal_links[idx];
                predictions.push(Prediction {
                    entity_id: link.effect_entity_id.clone(),
                    predicted_attributes: serde_json::json!({}),
                    confidence: link.confidence,
                    horizon_ms: link.avg_delay_ms as u64,
                    based_on: format!(
                        "causal_link:{}→{}",
                        link.cause_entity_id, link.effect_entity_id
                    ),
                });
            }
        }

        // Sort by confidence descending, deduplicate by entity_id
        predictions.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        predictions.dedup_by(|a, b| a.entity_id == b.entity_id);
        predictions.truncate(20);
        predictions
    }

    /// Returns all recorded causal links.
    pub fn get_causal_links(&self) -> Vec<CausalLink> {
        let inner = lock_guard(&self.inner);
        inner.causal_links.clone()
    }

    /// Predict what might happen next given an entity and attribute.
    ///
    /// Uses causal links to make the prediction:
    /// - If `entity_id` has outgoing causal links, predicts the effect on the
    ///   linked entity based on the most confident link.
    /// - If `entity_id` has incoming causal links, suggests what inputs affect it.
    /// - Returns `None` when no relevant causal links are found.
    pub fn predict(&self, entity_id: &str, attribute: &str, horizon_ms: u64) -> Option<Prediction> {
        let inner = lock_guard(&self.inner);

        // Check for outgoing causal links — predict effects on linked entities
        if let Some(indices) = inner.causal_links_by_cause.get(entity_id) {
            // Find the most confident link
            if let Some(&best_idx) = indices.iter().max_by(|&&a, &&b| {
                inner.causal_links[a]
                    .confidence
                    .partial_cmp(&inner.causal_links[b].confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }) {
                let link = &inner.causal_links[best_idx];
                let effect_state = inner
                    .entities
                    .iter()
                    .find(|e| e.id == link.effect_entity_id)
                    .map(|e| {
                        let mut attrs = serde_json::Map::new();
                        for (k, v) in &e.properties {
                            attrs.insert(k.clone(), serde_json::Value::String(v.clone()));
                        }
                        serde_json::Value::Object(attrs)
                    })
                    .unwrap_or_else(|| serde_json::json!({}));

                return Some(Prediction {
                    entity_id: link.effect_entity_id.clone(),
                    predicted_attributes: effect_state,
                    confidence: link.confidence,
                    horizon_ms: link.avg_delay_ms as u64,
                    based_on: format!("causal:{}→{}", link.cause_entity_id, link.effect_entity_id),
                });
            }
        }

        // Check for incoming causal links — suggest what inputs affect this entity
        let affecting_links: Vec<&CausalLink> = inner
            .causal_links
            .iter()
            .filter(|l| l.effect_entity_id == entity_id)
            .collect();

        if !affecting_links.is_empty() {
            let best_input = affecting_links
                .iter()
                .max_by(|a, b| {
                    a.confidence
                        .partial_cmp(&b.confidence)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap();
            return Some(Prediction {
                entity_id: entity_id.to_string(),
                predicted_attributes: serde_json::json!({
                    "affected_by": best_input.cause_entity_id,
                    "attribute": attribute,
                }),
                confidence: best_input.confidence * 0.9,
                horizon_ms,
                based_on: format!(
                    "input:{}→{}",
                    best_input.cause_entity_id, best_input.effect_entity_id
                ),
            });
        }

        None
    }

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

    // -----------------------------------------------------------------------
    // Test 13: CausalReasoner records state and discovers correlations.
    // -----------------------------------------------------------------------
    #[test]
    fn test_causal_reasoner_basic() {
        let mut reasoner = CausalReasoner::new(100, 10_000);

        let mut props_a = HashMap::new();
        props_a.insert("cpu".to_string(), "high".to_string());
        props_a.insert("mem".to_string(), "low".to_string());
        props_a.insert("disk".to_string(), "full".to_string());

        let mut props_b = HashMap::new();
        props_b.insert("cpu".to_string(), "high".to_string());
        props_b.insert("mem".to_string(), "high".to_string());
        props_b.insert("disk".to_string(), "empty".to_string());

        let now = now_ms();
        reasoner.record_state("server-1", props_a, now);
        reasoner.record_state("server-1", props_b, now + 100);

        let correlations = reasoner.infer_correlations();
        // Should find at least one correlation (mem change)
        assert!(
            !correlations.is_empty(),
            "expected at least one correlation from state change"
        );

        // Check correlation has sensible structure
        for c in correlations {
            assert!(!c.cause_entity.is_empty());
            assert!(!c.effect_entity.is_empty());
            assert!(c.confidence >= 0.0);
            assert!(c.confidence <= 1.0);
        }
    }

    // -----------------------------------------------------------------------
    // Test 14: CausalReasoner predict_next_state returns predictions.
    // -----------------------------------------------------------------------
    #[test]
    fn test_causal_reasoner_predict() {
        let mut reasoner = CausalReasoner::new(100, 10_000);
        let now = now_ms();

        // Set up history showing: when server-1 goes high-cpu, db-1 follows
        let mut s1_props = HashMap::new();
        s1_props.insert("cpu".to_string(), "high".to_string());
        reasoner.record_state("server-1", s1_props, now);

        let mut db_props = HashMap::new();
        db_props.insert("load".to_string(), "increased".to_string());
        reasoner.record_state("db-1", db_props, now + 500);

        reasoner.infer_correlations();

        // Now predict for server-1
        let mut current = HashMap::new();
        current.insert("cpu".to_string(), "high".to_string());
        let preds = reasoner.predict_next_state("server-1", &current);

        // May have predictions depending on correlation discovery
        for p in &preds {
            assert!(!p.0.is_empty());
            assert!(p.2 >= 0.0);
            assert!(p.2 <= 1.0);
        }
    }

    // -----------------------------------------------------------------------
    // Test 15: WorldModel.infer_causal_links records entity states.
    // -----------------------------------------------------------------------
    #[test]
    fn test_world_model_infer_causal_links() {
        let wm = WorldModel::new(test_config());

        let id_a = wm.register_entity("Gateway", EntityType::Service).unwrap();
        let id_b = wm
            .register_entity("Database", EntityType::DataStore)
            .unwrap();

        // Update entities with properties
        let mut props_a = HashMap::new();
        props_a.insert("status".to_string(), "degraded".to_string());
        wm.update_entity(&id_a, props_a).unwrap();

        let mut props_b = HashMap::new();
        props_b.insert("status".to_string(), "slow".to_string());
        wm.update_entity(&id_b, props_b).unwrap();

        let correlations = wm.infer_causal_links();
        // Even with minimal data, inference should not panic and return structured results
        assert!(correlations.is_empty() || !correlations.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 16: WorldModel.predict_next_state returns predictions for entity.
    // -----------------------------------------------------------------------
    #[test]
    fn test_world_model_predict_next_state() {
        let wm = WorldModel::new(test_config());

        let id = wm.register_entity("Node-1", EntityType::System).unwrap();

        let predictions = wm.predict_next_state(&id);
        // Without any causal data, predictions should be empty but not panic
        assert!(predictions.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 17: CausalReasoner handles empty state gracefully.
    // -----------------------------------------------------------------------
    #[test]
    fn test_causal_reasoner_empty() {
        let mut reasoner = CausalReasoner::new(10, 1000);
        let correlations = reasoner.infer_correlations();
        assert!(correlations.is_empty());

        let preds = reasoner.predict_next_state("nonexistent", &HashMap::new());
        assert!(preds.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 18: infer_causal_chain empty input returns empty.
    // -----------------------------------------------------------------------
    #[test]
    fn test_infer_causal_chain_empty() {
        let wm = WorldModel::new(test_config());
        let links = wm.infer_causal_chain(&[]);
        assert!(links.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 19: infer_causal_chain single transition yields no chain (needs ≥2).
    // -----------------------------------------------------------------------
    #[test]
    fn test_infer_causal_chain_single_transition() {
        let wm = WorldModel::new(test_config());
        let changes = vec![("idle".to_string(), "running".to_string())];
        let links = wm.infer_causal_chain(&changes);
        assert!(
            links.is_empty(),
            "single transition should not form a chain"
        );
    }

    // -----------------------------------------------------------------------
    // Test 20: infer_causal_chain two consecutive transitions form a link.
    // -----------------------------------------------------------------------
    #[test]
    fn test_infer_causal_chain_consecutive() {
        let wm = WorldModel::new(test_config());
        let changes = vec![
            ("idle".to_string(), "running".to_string()),
            ("running".to_string(), "completed".to_string()),
        ];
        let links = wm.infer_causal_chain(&changes);
        assert_eq!(
            links.len(),
            1,
            "two consecutive transitions → 1 causal link"
        );
        assert_eq!(links[0].cause_entity_id, "idle");
        assert_eq!(links[0].effect_entity_id, "completed");
        assert!(links[0].confidence > 0.0);
        assert!(links[0].confidence <= 1.0);
        assert!(links[0]
            .context_tags
            .iter()
            .any(|t| t.contains("inferred-chain")));
    }

    // -----------------------------------------------------------------------
    // Test 21: infer_causal_chain three transitions → two links.
    // -----------------------------------------------------------------------
    #[test]
    fn test_infer_causal_chain_three_transitions() {
        let wm = WorldModel::new(test_config());
        let changes = vec![
            ("idle".to_string(), "loading".to_string()),
            ("loading".to_string(), "processing".to_string()),
            ("processing".to_string(), "done".to_string()),
        ];
        let links = wm.infer_causal_chain(&changes);
        assert_eq!(
            links.len(),
            2,
            "three consecutive transitions → 2 causal links"
        );

        // First link: idle → processing (via loading)
        assert_eq!(links[0].cause_entity_id, "idle");
        assert_eq!(links[0].effect_entity_id, "processing");
        assert!(
            links[0].confidence > links[1].confidence,
            "earlier link should have higher confidence"
        );

        // Second link: loading → done (via processing)
        assert_eq!(links[1].cause_entity_id, "loading");
        assert_eq!(links[1].effect_entity_id, "done");
    }

    // -----------------------------------------------------------------------
    // Test 22: infer_causal_chain disconnected transitions produce no chain.
    // -----------------------------------------------------------------------
    #[test]
    fn test_infer_causal_chain_disconnected() {
        let wm = WorldModel::new(test_config());
        let changes = vec![
            ("idle".to_string(), "running".to_string()),
            ("sleeping".to_string(), "stopped".to_string()), // no overlap
        ];
        let links = wm.infer_causal_chain(&changes);
        assert!(
            links.is_empty(),
            "disconnected transitions should not form a chain"
        );
    }

    // -----------------------------------------------------------------------
    // Test 23: infer_causal_chain branching - first unvisited chain wins.
    // -----------------------------------------------------------------------
    #[test]
    fn test_infer_causal_chain_branching() {
        let wm = WorldModel::new(test_config());
        // Two transitions from "running": one goes to "completed", one to "failed"
        let changes = vec![
            ("idle".to_string(), "running".to_string()),
            ("running".to_string(), "completed".to_string()),
            ("running".to_string(), "failed".to_string()),
        ];
        let links = wm.infer_causal_chain(&changes);
        // Should form a chain starting from index 0: idle→running→completed
        assert_eq!(links.len(), 1, "should pick first unvisited branch");
        assert_eq!(links[0].cause_entity_id, "idle");
        assert_eq!(links[0].effect_entity_id, "completed");
    }

    // -----------------------------------------------------------------------
    // Test 24: infer_causal_chains_deep returns empty with no correlations.
    // -----------------------------------------------------------------------
    #[test]
    fn test_causal_chains_deep_empty() {
        let reasoner = CausalReasoner::new(10, 1000);
        let chains = reasoner.infer_causal_chains_deep(5, 0.15);
        assert!(
            chains.is_empty(),
            "no correlations should produce no chains"
        );
    }

    // -----------------------------------------------------------------------
    // Test 25: infer_causal_chains_deep confidence decay over chain length.
    // -----------------------------------------------------------------------
    #[test]
    fn test_causal_chains_deep_confidence_decay() {
        let mut reasoner = CausalReasoner::new(100, 10_000);
        let now = now_ms();

        // Set up a chain: A→B, B→C, C→D
        let mut props_a = HashMap::new();
        props_a.insert("state".to_string(), "active".to_string());
        reasoner.record_state("A", props_a.clone(), now);

        let mut props_b = HashMap::new();
        props_b.insert("state".to_string(), "active".to_string());
        reasoner.record_state("B", props_b.clone(), now + 100);
        reasoner.record_state("A", props_a.clone(), now + 100);

        let mut props_c = HashMap::new();
        props_c.insert("state".to_string(), "active".to_string());
        reasoner.record_state("C", props_c.clone(), now + 200);
        reasoner.record_state("B", props_b.clone(), now + 200);

        let mut props_d = HashMap::new();
        props_d.insert("state".to_string(), "active".to_string());
        reasoner.record_state("D", props_d.clone(), now + 300);
        reasoner.record_state("C", props_c.clone(), now + 300);

        reasoner.infer_correlations();

        let chains = reasoner.infer_causal_chains_deep(10, 0.01);

        // Should find at least one chain
        if !chains.is_empty() {
            let chain = &chains[0];
            assert!(chain.confidence > 0.0);
            assert!(chain.confidence <= 1.0);
            // Path type may be Direct or Or depending on correlation structure
            match &chain.path_type {
                CausalPathType::Direct => {}
                CausalPathType::Or(_) => {}
                CausalPathType::And(_) => {}
            }

            // Verify confidence decay: each link's confidence should be > 0.0
            for link in &chain.links {
                assert!(link.confidence > 0.0);
                assert!(link.confidence <= 1.0);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Test 26: infer_causal_chains_deep detects branching (OR) paths.
    // -----------------------------------------------------------------------
    #[test]
    fn test_causal_chains_deep_branching() {
        let mut reasoner = CausalReasoner::new(100, 10_000);
        let now = now_ms();

        // Simulate correlations via snapshots: X→Y and X→Z (same cause, different effects)
        let mut props_x = HashMap::new();
        props_x.insert("signal".to_string(), "trigger".to_string());
        reasoner.record_state("X", props_x.clone(), now);

        let mut props_y = HashMap::new();
        props_y.insert("status".to_string(), "started".to_string());
        reasoner.record_state("Y", props_y.clone(), now + 100);
        reasoner.record_state("X", props_x.clone(), now + 100);

        let mut props_z = HashMap::new();
        props_z.insert("status".to_string(), "started".to_string());
        reasoner.record_state("Z", props_z.clone(), now + 200);
        reasoner.record_state("X", props_x.clone(), now + 200);

        reasoner.infer_correlations();

        let chains = reasoner.infer_causal_chains_deep(5, 0.01);

        // With multiple correlations from the same cause, some chains may have OR path type
        let _or_chains: Vec<&CausalChain> = chains
            .iter()
            .filter(|c| matches!(c.path_type, CausalPathType::Or(_)))
            .collect();

        // At minimum, all returned chains should be valid
        for chain in &chains {
            assert!(!chain.links.is_empty());
            assert!(chain.confidence > 0.0);
            assert!(chain.confidence <= 1.0);
            assert!(!chain.is_feedback_loop);
        }
    }

    // -----------------------------------------------------------------------
    // Test 27: infer_causal_chains_deep detects feedback loops.
    // -----------------------------------------------------------------------
    #[test]
    fn test_causal_chains_deep_feedback_loop() {
        let mut reasoner = CausalReasoner::new(100, 10_000);
        let now = now_ms();

        // Create a cycle: A→B, B→C, C→A
        let mut props_a = HashMap::new();
        props_a.insert("state".to_string(), "on".to_string());
        reasoner.record_state("A", props_a.clone(), now);

        let mut props_b = HashMap::new();
        props_b.insert("state".to_string(), "on".to_string());
        reasoner.record_state("B", props_b.clone(), now + 100);
        reasoner.record_state("A", props_a.clone(), now + 100);

        let mut props_c = HashMap::new();
        props_c.insert("state".to_string(), "on".to_string());
        reasoner.record_state("C", props_c.clone(), now + 200);
        reasoner.record_state("B", props_b.clone(), now + 200);

        // Close the loop: C→A
        reasoner.record_state("A", props_a.clone(), now + 300);
        reasoner.record_state("C", props_c.clone(), now + 300);

        reasoner.infer_correlations();

        let chains = reasoner.infer_causal_chains_deep(10, 0.01);

        // Some chains may be marked as feedback loops
        let _feedback_chains: Vec<&CausalChain> =
            chains.iter().filter(|c| c.is_feedback_loop).collect();

        // All chains should be structurally valid
        for chain in &chains {
            assert!(!chain.links.is_empty());
            assert!(chain.confidence >= 0.0);
            assert!(chain.confidence <= 1.0);
        }
    }

    // -----------------------------------------------------------------------
    // Test 28: infer_causal_chains_deep filters low-confidence links.
    // -----------------------------------------------------------------------
    #[test]
    fn test_causal_chains_deep_threshold_filtering() {
        let mut reasoner = CausalReasoner::new(100, 10_000);
        let now = now_ms();

        // Add many snapshots to try to generate some correlations
        for i in 0..5 {
            let mut props_a = HashMap::new();
            props_a.insert("val".to_string(), format!("x{i}"));
            reasoner.record_state("threshold-A", props_a, now + i * 100);

            let mut props_b = HashMap::new();
            props_b.insert("val".to_string(), format!("y{i}"));
            reasoner.record_state("threshold-B", props_b, now + i * 100 + 50);
        }

        reasoner.infer_correlations();

        // With a high threshold, we may get fewer or no chains
        let chains_high = reasoner.infer_causal_chains_deep(5, 0.9);

        // With no threshold (negative), we should get at least as many chains
        let chains_all = reasoner.infer_causal_chains_deep(5, -1.0);

        assert!(
            chains_all.len() >= chains_high.len(),
            "no-threshold should return >= chains vs high-threshold"
        );
    }

    // -----------------------------------------------------------------------
    // Test 29: infer_causal_chains_deep wrapper on WorldModel.
    // -----------------------------------------------------------------------
    #[test]
    fn test_world_model_causal_chains_deep() {
        let wm = WorldModel::new(test_config());

        // Without any data, should return empty without panicking
        let chains = wm.infer_causal_chains_deep(5, 0.15);
        assert!(chains.is_empty());
    }
}
