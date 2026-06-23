//! Causal reasoner — entity state-tracking correlation engine.
//!
//! This sub-module contains the [`CausalReasoner`] struct and all its methods,
//! which analyze entity state snapshots to discover correlations and causal chains.

use super::types::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

/// Maintains inference state for discovering causal relationships
/// by analyzing entity state changes over time windows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CausalReasoner {
    /// Historical entity state snapshots for correlation analysis.
    pub(crate) history: Vec<EntityStateSnapshot>,
    /// Discovered correlations between property changes.
    correlations: Vec<Correlation>,
    /// Maximum number of snapshots to retain.
    max_history: usize,
    /// Time window (ms) for considering two changes as potentially causal.
    window_ms: u64,
}

impl CausalReasoner {
    /// Creates a new reasoner with the given capacity and time window.
    pub fn new(max_history: usize, window_ms: u64) -> Self {
        Self {
            history: Vec::with_capacity(max_history),
            correlations: Vec::new(),
            max_history,
            window_ms,
        }
    }

    /// Records a state snapshot for an entity.
    /// Evicts the oldest snapshot when history is at capacity.
    pub fn record_state(
        &mut self,
        entity_id: &str,
        properties: HashMap<String, String>,
        timestamp_ms: u64,
    ) {
        if self.history.len() >= self.max_history {
            self.history.remove(0);
        }
        self.history.push(EntityStateSnapshot {
            entity_id: entity_id.to_string(),
            properties,
            timestamp_ms,
        });
    }

    /// Analyzes state history to discover correlations between property changes.
    ///
    /// For each pair of entity snapshots within `window_ms`, checks whether
    /// a property change in one correlates with a property change in another.
    /// Updates the internal `correlations` vector with co-occurrence counts
    /// and confidence scores.
    pub fn infer_correlations(&mut self) -> &[Correlation] {
        let window = self.window_ms as i64;
        #[allow(clippy::type_complexity)]
        let mut co_occurrences: HashMap<(String, String, String, String), (u64, Vec<i64>)> =
            HashMap::new();

        for i in 0..self.history.len() {
            for j in (i + 1)..self.history.len() {
                let snap_a = &self.history[i];
                let snap_b = &self.history[j];
                let delta = snap_b.timestamp_ms as i64 - snap_a.timestamp_ms as i64;

                if delta.abs() > window {
                    continue;
                }

                // Find properties that differ between consecutive snapshots
                // for the same entity, or track cross-entity correlations.
                if snap_a.entity_id == snap_b.entity_id {
                    // Same entity: detect which properties changed
                    let changed_a = Self::detect_changes(snap_a, snap_b);
                    for (prop_a, _) in &changed_a {
                        for (prop_b, _) in &changed_a {
                            if prop_a != prop_b {
                                let key = (
                                    snap_a.entity_id.clone(),
                                    prop_a.to_string(),
                                    snap_b.entity_id.clone(),
                                    prop_b.to_string(),
                                );
                                let entry = co_occurrences.entry(key).or_insert((0, Vec::new()));
                                entry.0 += 1;
                                entry.1.push(delta);
                            }
                        }
                    }
                } else {
                    // Different entities: cross-entity correlation
                    let changed_a = Self::detect_changes(snap_a, snap_b);
                    let changed_b = Self::detect_changes(snap_b, snap_a);
                    // For simplicity, check if both had concurrent changes
                    if !changed_a.is_empty() && !changed_b.is_empty() {
                        for (prop_a, _) in &changed_a {
                            for (prop_b, _) in &changed_b {
                                let key = (
                                    snap_a.entity_id.clone(),
                                    prop_a.to_string(),
                                    snap_b.entity_id.clone(),
                                    prop_b.to_string(),
                                );
                                let entry = co_occurrences.entry(key).or_insert((0, Vec::new()));
                                entry.0 += 1;
                                entry.1.push(delta);
                            }
                        }
                    }
                }
            }
        }

        // Convert co-occurrence map into correlation scores
        let total_pairs = if self.history.len() > 1 {
            (self.history.len() * (self.history.len() - 1)) / 2
        } else {
            1
        };

        self.correlations = co_occurrences
            .into_iter()
            .map(|((ce, cp, ee, ep), (count, deltas))| {
                let raw_score = count as f64 / total_pairs as f64;
                let confidence = (raw_score * 2.0).clamp(0.0, 1.0);
                let avg_delta = if deltas.is_empty() {
                    0
                } else {
                    deltas.iter().sum::<i64>() / deltas.len() as i64
                };
                Correlation {
                    cause_entity: ce,
                    cause_property: cp,
                    effect_entity: ee,
                    effect_property: ep,
                    co_occurrence_count: count,
                    confidence,
                    avg_time_delta_ms: avg_delta,
                }
            })
            .collect();

        // Sort by confidence descending
        self.correlations.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        &self.correlations
    }

    /// Returns the current set of discovered correlations.
    pub fn correlations(&self) -> &[Correlation] {
        &self.correlations
    }
}

impl fmt::Display for CausalReasoner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let cors = self.correlations();
        if cors.is_empty() {
            writeln!(f, "CausalReasoner: no correlations discovered")?;
        } else {
            writeln!(f, "CausalReasoner: {} correlations:", cors.len())?;
            for corr in cors.iter().take(10) {
                writeln!(
                    f,
                    "  {}:{} → {}:{} (confidence={:.3}, count={})",
                    corr.cause_entity,
                    corr.cause_property,
                    corr.effect_entity,
                    corr.effect_property,
                    corr.confidence,
                    corr.co_occurrence_count
                )?;
            }
            if cors.len() > 10 {
                writeln!(f, "  ... and {} more", cors.len() - 10)?;
            }
        }
        writeln!(
            f,
            "History: {} snapshots, window={}ms",
            self.history.len(),
            self.window_ms
        )?;
        Ok(())
    }
}

impl CausalReasoner {
    /// Chains discovered correlations into causal sequences.
    ///
    /// Starting from each correlation, greedily extends chains of the form
    /// A→B, B→C, ... up to `max_chain_length`. Returns all found chains
    /// sorted by length (longest first), then by starting confidence.
    pub fn infer_causal_chains(&self, max_chain_length: usize) -> Vec<Vec<Correlation>> {
        let chain_len = max_chain_length.max(2);
        if self.correlations.is_empty() {
            return Vec::new();
        }

        // Build a lookup: (effect_entity, effect_property) -> indices
        let mut effect_index: HashMap<(String, String), Vec<usize>> = HashMap::new();
        for (i, corr) in self.correlations.iter().enumerate() {
            effect_index
                .entry((corr.effect_entity.clone(), corr.effect_property.clone()))
                .or_default()
                .push(i);
        }

        let mut chains: Vec<Vec<Correlation>> = Vec::new();

        for start_idx in 0..self.correlations.len() {
            let mut chain = Vec::new();
            chain.push(self.correlations[start_idx].clone());

            // Greedily extend the chain by following effect -> cause matches
            while chain.len() < chain_len {
                let last = chain.last().unwrap();
                let next_key = (last.effect_entity.clone(), last.effect_property.clone());
                if let Some(next_indices) = effect_index.get(&next_key) {
                    // Pick the next correlation with highest confidence,
                    // avoiding cycles (don't revisit an entity already in the chain).
                    let already_in_chain: Vec<&str> =
                        chain.iter().map(|c| c.effect_entity.as_str()).collect();
                    if let Some(&best_idx) = next_indices
                        .iter()
                        .filter(|&&idx| {
                            !already_in_chain
                                .contains(&self.correlations[idx].effect_entity.as_str())
                        })
                        .max_by(|&&a, &&b| {
                            self.correlations[a]
                                .confidence
                                .partial_cmp(&self.correlations[b].confidence)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                    {
                        chain.push(self.correlations[best_idx].clone());
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }

            if chain.len() >= 2 {
                chains.push(chain);
            }
        }

        // Sort by length descending, then by first-link confidence descending
        chains.sort_by(|a, b| {
            b.len().cmp(&a.len()).then_with(|| {
                b.first()
                    .map(|c| c.confidence)
                    .unwrap_or(0.0)
                    .partial_cmp(&a.first().map(|c| c.confidence).unwrap_or(0.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        });

        // Deduplicate chains with identical (cause, effect) sequences
        chains.dedup_by(|a, b| {
            a.len() == b.len()
                && a.iter().zip(b.iter()).all(|(ca, cb)| {
                    ca.cause_entity == cb.cause_entity
                        && ca.cause_property == cb.cause_property
                        && ca.effect_entity == cb.effect_entity
                        && ca.effect_property == cb.effect_property
                })
        });

        chains
    }

    /// Infer causal chains with probabilistic confidence decay, branching path
    /// detection, feedback loop awareness, and confidence threshold filtering.
    ///
    /// This is a deeper variant of [`infer_causal_chains`] that returns structured
    /// [`CausalChain`] objects instead of raw correlation vectors. It applies:
    ///
    /// - **Confidence decay**: `confidence *= 1.0 / (1.0 + position * 0.15)`
    ///   so longer chains have exponentially decaying confidence.
    /// - **Branching paths**: When two different correlations share the same
    ///   `(effect_entity, effect_property)` as the start of another chain, they
    ///   are merged as `And` or `Or` branches.
    /// - **Feedback loop detection**: Chains that revisit an already-visited
    ///   entity are marked with `is_feedback_loop = true`.
    /// - **Confidence threshold filtering**: Links with confidence < 0.15 are
    ///   filtered out to reduce noise.
    ///
    /// # Parameters
    ///
    /// * `max_chain_length` — maximum number of links in any single chain.
    /// * `min_confidence` — minimum confidence for a link to be included
    ///   (default 0.15). Pass a value < 0.0 to disable filtering.
    ///
    /// # Returns
    ///
    /// A `Vec<CausalChain>` sorted by aggregate confidence descending.
    pub fn infer_causal_chains_deep(
        &self,
        max_chain_length: usize,
        min_confidence: f64,
    ) -> Vec<CausalChain> {
        let chain_len = max_chain_length.max(2);
        let effective_min_conf = if min_confidence < 0.0 {
            0.0
        } else {
            min_confidence
        };

        if self.correlations.is_empty() {
            return Vec::new();
        }

        // Build lookup: (cause_entity, cause_property) -> indices for AND/OR merging
        let mut cause_index: HashMap<(String, String), Vec<usize>> = HashMap::new();
        // Build lookup: (effect_entity, effect_property) -> indices for chain extension
        let mut effect_index: HashMap<(String, String), Vec<usize>> = HashMap::new();

        for (i, corr) in self.correlations.iter().enumerate() {
            // Filter out low-confidence correlations early
            if corr.confidence >= effective_min_conf {
                cause_index
                    .entry((corr.cause_entity.clone(), corr.cause_property.clone()))
                    .or_default()
                    .push(i);
            }
            // Always index effects so we can detect gaps
            effect_index
                .entry((corr.effect_entity.clone(), corr.effect_property.clone()))
                .or_default()
                .push(i);
        }

        let mut chains: Vec<CausalChain> = Vec::new();
        let mut seen_chains: Vec<Vec<(String, String, String, String)>> = Vec::new();

        for start_idx in 0..self.correlations.len() {
            if self.correlations[start_idx].confidence < effective_min_conf {
                continue;
            }

            let mut chain_indices: Vec<usize> = Vec::new();
            chain_indices.push(start_idx);

            // Track entities in chain for feedback loop detection
            let mut entities_in_chain: Vec<&str> = Vec::new();
            entities_in_chain.push(&self.correlations[start_idx].cause_entity);
            entities_in_chain.push(&self.correlations[start_idx].effect_entity);

            // Greedily extend the chain
            while chain_indices.len() < chain_len {
                let last = &self.correlations[*chain_indices.last().unwrap()];
                let next_key = (last.effect_entity.clone(), last.effect_property.clone());

                let candidates: Vec<usize> = match effect_index.get(&next_key) {
                    Some(indices) => indices
                        .iter()
                        .copied()
                        .filter(|&idx| {
                            self.correlations[idx].confidence >= effective_min_conf
                                && idx != start_idx
                        })
                        .collect(),
                    None => break,
                };

                if candidates.is_empty() {
                    break;
                }

                // Pick the highest-confidence next step
                let best_idx = candidates
                    .into_iter()
                    .max_by(|&a, &b| {
                        self.correlations[a]
                            .confidence
                            .partial_cmp(&self.correlations[b].confidence)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .unwrap();

                let next_effect = &self.correlations[best_idx].effect_entity;

                // Detect feedback loop: if the effect entity is already in chain
                if entities_in_chain.contains(&next_effect.as_str()) {
                    chain_indices.push(best_idx);
                    // Mark as feedback loop — will be set on the CausalChain
                    break;
                }

                chain_indices.push(best_idx);
                entities_in_chain.push(next_effect);
            }

            if chain_indices.len() < 2 {
                continue;
            }

            // Build the chain. Apply confidence decay: position 0 gets no decay.
            let mut links: Vec<CausalLink> = Vec::new();
            let mut confidence_product = 1.0;
            let mut is_feedback_loop = false;

            for (pos, &idx) in chain_indices.iter().enumerate() {
                let corr = &self.correlations[idx];
                let decay = 1.0 / (1.0 + pos as f64 * 0.15);
                let link_confidence = corr.confidence * decay;
                confidence_product *= link_confidence;

                // Check if this link causes a re-visit (feedback loop)
                if pos > 0
                    && entities_in_chain
                        .iter()
                        .take(pos + 1)
                        .filter(|&&e| *e == corr.effect_entity)
                        .count()
                        > 1
                {
                    is_feedback_loop = true;
                }

                links.push(CausalLink {
                    cause_entity_id: corr.cause_entity.clone(),
                    effect_entity_id: corr.effect_entity.clone(),
                    confidence: link_confidence.clamp(0.0, 1.0),
                    observation_count: corr.co_occurrence_count,
                    avg_delay_ms: corr.avg_time_delta_ms.max(0) as f64,
                    context_tags: vec![
                        "correlation-chain".to_string(),
                        format!("pos:{pos}"),
                        format!("confidence:{link_confidence:.3}"),
                    ],
                });
            }

            // Detect branching paths: look for correlations that share the same
            // (cause_entity, cause_property) as the first link in this chain.
            let first = &self.correlations[chain_indices[0]];
            let branch_key = (first.cause_entity.clone(), first.cause_property.clone());
            let path_type = match cause_index.get(&branch_key) {
                Some(indices) if indices.len() > 1 => {
                    let branch_entities: Vec<String> = indices
                        .iter()
                        .filter(|&&idx| idx != chain_indices[0])
                        .map(|&idx| self.correlations[idx].effect_entity.clone())
                        .collect();
                    if !branch_entities.is_empty() {
                        CausalPathType::Or(branch_entities)
                    } else {
                        CausalPathType::Direct
                    }
                }
                _ => CausalPathType::Direct,
            };

            let aggregate_confidence = confidence_product.clamp(0.0, 1.0);

            // Deduplicate: skip if we've already seen this exact entity sequence
            let signature: Vec<(String, String, String, String)> = chain_indices
                .iter()
                .map(|&idx| {
                    let c = &self.correlations[idx];
                    (
                        c.cause_entity.clone(),
                        c.cause_property.clone(),
                        c.effect_entity.clone(),
                        c.effect_property.clone(),
                    )
                })
                .collect();

            if seen_chains.contains(&signature) {
                continue;
            }
            seen_chains.push(signature);

            // Final confidence threshold filter on aggregate
            if aggregate_confidence < effective_min_conf && effective_min_conf > 0.0 {
                continue;
            }

            chains.push(CausalChain {
                links,
                confidence: aggregate_confidence,
                path_type,
                is_feedback_loop,
                chain_length: chain_indices.len(),
            });
        }

        // Sort by aggregate confidence descending, then by chain length descending
        chains.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.chain_length.cmp(&a.chain_length))
        });

        chains
    }

    /// Predicts probable next state changes for a given entity based on
    /// discovered correlations and historical snapshots.
    ///
    /// Returns a list of `(property, expected_value, confidence)` tuples.
    /// Unlike the basic version, this method examines historical snapshots
    /// to extract concrete expected values for each predicted property.
    pub fn predict_next_state(
        &self,
        entity_id: &str,
        current_properties: &HashMap<String, String>,
    ) -> Vec<(String, String, f64)> {
        let mut predictions: Vec<(String, String, f64)> = Vec::new();

        // Pre-index historical snapshots by entity_id for fast lookup.
        let mut history_by_entity: HashMap<&str, Vec<&EntityStateSnapshot>> = HashMap::new();
        for snap in &self.history {
            history_by_entity
                .entry(snap.entity_id.as_str())
                .or_default()
                .push(snap);
        }

        for corr in &self.correlations {
            // Determine if the correlation applies and what to predict.
            let (target_entity, target_prop, base_confidence, is_cause_side) =
                if corr.cause_entity == entity_id {
                    // This entity is the cause; predict the effect
                    let cause_val = current_properties.get(&corr.cause_property);
                    if cause_val.is_none() {
                        continue;
                    }
                    (
                        corr.effect_entity.as_str(),
                        corr.effect_property.as_str(),
                        corr.confidence * 0.8,
                        true,
                    )
                } else if corr.effect_entity == entity_id {
                    // This entity is the effect; predict the effect property change
                    (
                        corr.effect_entity.as_str(),
                        corr.effect_property.as_str(),
                        corr.confidence * 0.7,
                        false,
                    )
                } else {
                    continue;
                };

            // Try to extract an actual predicted value from historical data.
            // Look for the last snapshot where the target property had a
            // value that appeared after a matching cause-side change.
            let predicted_val = if is_cause_side {
                // We are predicting the effect entity's property.
                // Find the most recent snapshot of the effect entity where
                // this property had a value that changed from a prior snapshot
                // within the correlation's time window.
                let window = self.window_ms as i64;
                let mut best_val = String::new();

                if let Some(effect_snaps) = history_by_entity.get(target_entity) {
                    for snap in effect_snaps.iter().rev() {
                        if let Some(val) = snap.properties.get(target_prop) {
                            let causal_snaps = history_by_entity
                                .get(entity_id)
                                .map(|v| v.as_slice())
                                .unwrap_or(&[]);
                            for csnap in causal_snaps.iter().rev() {
                                let delta = snap.timestamp_ms as i64 - csnap.timestamp_ms as i64;
                                if delta > 0 && delta <= window {
                                    if let Some(cause_val) =
                                        current_properties.get(&corr.cause_property)
                                    {
                                        if csnap.properties.get(&corr.cause_property)
                                            != Some(cause_val)
                                        {
                                            // The cause entity's property changed
                                            // shortly before this snapshot.
                                            if !val.is_empty() {
                                                best_val = val.clone();
                                            }
                                        }
                                    }
                                    break;
                                }
                            }
                            if !best_val.is_empty() {
                                break;
                            }
                        }
                    }
                }
                best_val
            } else {
                // Entity is the effect itself: find the most recent historical
                // value for this property.
                let mut best_val = String::new();
                if let Some(snaps) = history_by_entity.get(entity_id) {
                    for snap in snaps.iter().rev() {
                        if let Some(val) = snap.properties.get(target_prop) {
                            if !val.is_empty() {
                                best_val = val.clone();
                                break;
                            }
                        }
                    }
                }
                best_val
            };

            let confidence = base_confidence + if predicted_val.is_empty() { 0.0 } else { 0.1 };
            predictions.push((target_prop.to_string(), predicted_val, confidence));
        }

        // Sort by confidence descending and deduplicate (keep highest confidence).
        predictions.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        predictions.dedup_by(|a, b| a.0 == b.0);
        predictions.truncate(10);
        predictions
    }

    /// Helper: checks if a property value changed between two snapshots
    /// (only meaningful when snap_b is later than snap_a for the same entity).
    fn detect_changes<'a>(
        snap_a: &'a EntityStateSnapshot,
        snap_b: &'a EntityStateSnapshot,
    ) -> Vec<(&'a str, &'a str)> {
        let mut changed = Vec::new();
        // For cross-entity analysis, just report both sets of properties.
        // For same entity, report properties whose values differ.
        if snap_a.entity_id == snap_b.entity_id {
            for (key, val_b) in &snap_b.properties {
                if snap_a.properties.get(key) != Some(val_b) {
                    changed.push((key.as_str(), val_b.as_str()));
                }
            }
            for key in snap_a.properties.keys() {
                if !snap_b.properties.contains_key(key) {
                    changed.push((key.as_str(), ""));
                }
            }
        } else {
            // Cross-entity: report both snapshots' properties as "changes"
            // to detect concurrent modifications
            for (key, val) in &snap_a.properties {
                changed.push((key.as_str(), val.as_str()));
            }
        }
        changed
    }
}
