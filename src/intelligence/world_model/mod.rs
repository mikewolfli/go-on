//! BLUE38 F-GAP-23: World Model Pipeline (M7 "世界模型流水线")
//!
//! A thread-safe pipeline that maintains a structured representation of the
//! external environment — tracking entities, relationships, events, and state
//! changes over time. All state is guarded behind `Arc<Mutex<>>`.
//!
//! The production surface is deliberately small: entity registration/updates,
//! event recording, and the two Bayesian scoring queries consumed by
//! `CapabilityBus::decide()` (`causal_agent_insight` /
//! `counterfactual_probability`). The former inference/prediction batch API
//! (`query_*`, `predict_*`, `snapshot`, `profile`, `infer_causal_links`, …)
//! had zero production callers and was removed.

mod causal;
mod types;

pub use causal::CausalReasoner;
pub use types::*;

use crate::i18n::runtime::tf;
use crate::intelligence::causal_bayesian_graph::CausalBayesianGraph;

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
    /// Probabilistic causal graph with MCTS for ultra-long chain reasoning.
    bayesian_graph: CausalBayesianGraph,
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
                events: Vec::new(),
                last_update_ms: crate::shared::timestamps::now_ts_ms() as u64,
                next_entity_id: 1,
                next_event_id: 1,
                causal_reasoner: CausalReasoner::new(5000),
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

        // Periodically run correlation inference and feed the Bayesian graph
        let should_infer = inner
            .update_counter
            .is_multiple_of(inner.correlation_inference_interval);

        if should_infer {
            // Extract state transitions from the causal reasoner's history
            // and feed them into infer_causal_chain for entity-state-level chains.
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
                    let mut inner = crate::lock_or_recover!(&self.inner, "intelligence");
                    for link in &chain_links {
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
        inner.update_counter += 1;

        Ok(id)
    }

    /// Answer a counterfactual query: "What would the probability of `effect`
    /// be if `cause` had NOT happened?"
    ///
    /// Uses Bayesian inversion on the internal causal graph:
    /// P(effect | ¬cause) = (P(effect) - P(cause) * P(effect|cause)) / (1 - P(cause))
    pub fn counterfactual_probability(&self, cause_entity: &str, effect_entity: &str) -> f64 {
        let inner = crate::lock_or_recover!(&self.inner, "intelligence");
        inner.bayesian_graph.counterfactual_probability(
            cause_entity,
            "state",
            effect_entity,
            "state",
        )
    }

    /// Query causal insight for agent selection hot path.
    ///
    /// Uses the Bayesian causal graph to evaluate how causally effective an agent
    /// has been for a given task type. Returns a score in [0.0, 1.0] where higher
    /// means the agent has a strong causal relationship with successful outcomes.
    ///
    /// IMPORTANT: All edges are recorded by `record_correlation` with property
    /// `"state"` for both cause and effect nodes. Queries MUST use `"state"` as
    /// the property name to match recorded observations — using `"status"` or
    /// `"success"` would cause zero matches and always return neutral 0.5.
    ///
    /// This is called from `CapabilityBus::decide()` as an additional scoring
    /// dimension alongside reputation, recency, and task-fit scores.
    pub fn causal_agent_insight(&self, agent_name: &str, task_type: &str) -> f64 {
        let inner = crate::lock_or_recover!(&self.inner, "intelligence");
        // Only meaningful if sufficient observations exist (at least 10 edges)
        if inner.bayesian_graph.edge_count() < 10 {
            return 0.5; // neutral — insufficient data
        }
        // Query: how does agent_name → (task_type) causal strength look?
        // Using "state" as property to match record_correlation() calls.
        let paths = inner.bayesian_graph.find_paths_mcts(
            agent_name, "state", task_type, "state", 5,    // max_path_length
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

    /// Infer a chain of causal links from an ordered sequence of raw state
    /// transitions `(from_state, to_state)`.
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

        // Verify the update is recorded by reading the entity back through the
        // causal reasoner's state history (the only observable read path).
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

    #[test]
    fn test_infer_causal_chain_empty() {
        let wm = WorldModel::new(test_config());
        let links = wm.infer_causal_chain(&[]);
        assert!(links.is_empty());
    }

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
}
