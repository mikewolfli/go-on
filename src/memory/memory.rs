//! Memory policy layer for go-on (Phase 2/3)
//!
//! These structures are intentional framework definitions for Phase 0-9 architecture.
//! Memory classes and policies define how artifacts are retained and promoted,
//! to be integrated into the execution flow once promotion logic is wired.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Memory class types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum MemoryClass {
    Transient,
    Episodic,
    Semantic,
    ProjectState,
    Observation,
}

/// Memory entry with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub class: MemoryClass,
    pub content: String,
    pub timestamp: String,
    pub usefulness: f32,
    pub staleness: u32,
    /// Optional user_id for multi-user isolation.
    pub user_id: Option<String>,
}

/// Memory policy governs storage, promotion, retrieval, and GC
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryPolicy {
    pub transient_max_size: usize,
    pub episodic_max_size: usize,
    pub semantic_max_size: usize,
    pub project_state_max_size: usize,
    pub observation_max_size: usize,
    pub usefulness_threshold: f32,
    pub staleness_max_days: u32,
}

impl Default for MemoryPolicy {
    fn default() -> Self {
        Self {
            transient_max_size: 10,
            episodic_max_size: 50,
            semantic_max_size: 100,
            project_state_max_size: 20,
            observation_max_size: 200,
            usefulness_threshold: 0.5,
            staleness_max_days: 30,
        }
    }
}

impl MemoryPolicy {
    pub fn should_retain(&self, entry: &MemoryEntry) -> bool {
        entry.usefulness >= self.usefulness_threshold && entry.staleness <= self.staleness_max_days
    }
}

/// Promotion report returned by MemoryStore::promote
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryPromotionReport {
    pub promoted_count: usize,
    /// Each entry: (id, from_class_name, to_class_name)
    pub promotion_map: Vec<(String, String, String)>,
}

/// Maximum total entries across all memory classes (safety net beyond per-class limits).
const MAX_ENTRIES: usize = 500;

/// Memory store with policy management
#[derive(Debug)]
pub struct MemoryStore {
    entries: HashMap<String, MemoryEntry>,
    policy: MemoryPolicy,
    /// O(1) count of entries per class
    class_counts: HashMap<MemoryClass, usize>,
    /// O(log n) timestamp index per class: timestamp → entry_id
    entries_by_class: HashMap<MemoryClass, BTreeMap<u64, String>>,
    /// Monotonically increasing sequence for ordering entries
    store_sequence: u64,
}

impl MemoryStore {
    pub fn new(policy: MemoryPolicy) -> Self {
        Self {
            entries: HashMap::new(),
            policy,
            class_counts: HashMap::new(),
            entries_by_class: HashMap::new(),
            store_sequence: 0,
        }
    }

    /// Clear all entries from the memory store.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.class_counts.clear();
        self.entries_by_class.clear();
        self.store_sequence = 0;
    }

    /// Get the maximum number of entries allowed for a given memory class.
    fn class_max_size(&self, class: &MemoryClass) -> usize {
        match class {
            MemoryClass::Transient => self.policy.transient_max_size,
            MemoryClass::Episodic => self.policy.episodic_max_size,
            MemoryClass::Semantic => self.policy.semantic_max_size,
            MemoryClass::ProjectState => self.policy.project_state_max_size,
            MemoryClass::Observation => self.policy.observation_max_size,
        }
    }

    /// Store a memory entry, enforcing per-class capacity limits.
    ///
    /// If the class already has `max_size` entries, the oldest entry
    /// (by timestamp) is evicted to make room for the new one.
    ///
    /// Uses O(log n) lookup via class index trees.
    pub fn store(&mut self, entry: MemoryEntry) {
        let class = entry.class.clone();
        let max_size = self.class_max_size(&class);
        let seq = self.store_sequence;
        self.store_sequence += 1;

        // If an entry with the same id already exists, update it without eviction.
        if let Some(existing) = self.entries.get(&entry.id) {
            if existing.class != class {
                // Class changed — remove old class index entries
                if let Some(tree) = self.entries_by_class.get_mut(&existing.class) {
                    let old_seq = tree.iter().find(|(_, v)| *v == &entry.id).map(|(k, _)| *k);
                    if let Some(k) = old_seq {
                        tree.remove(&k);
                    }
                }
                *self.class_counts.entry(existing.class.clone()).or_insert(0) = self
                    .class_counts
                    .get(&existing.class)
                    .copied()
                    .unwrap_or(1)
                    .saturating_sub(1);
                // Add to new class
                self.class_counts
                    .entry(class.clone())
                    .and_modify(|c| *c += 1)
                    .or_insert(1);
                self.entries_by_class
                    .entry(class.clone())
                    .or_default()
                    .insert(seq, entry.id.clone());
            } else {
                // Same class — update sequence to reflect refresh
                if let Some(tree) = self.entries_by_class.get_mut(&class) {
                    let old_seq = tree.iter().find(|(_, v)| *v == &entry.id).map(|(k, _)| *k);
                    if let Some(k) = old_seq {
                        tree.remove(&k);
                    }
                    tree.insert(seq, entry.id.clone());
                }
            }
            self.entries.insert(entry.id.clone(), entry);
            return;
        }

        // O(1) class count check
        let class_count = self.class_counts.get(&class).copied().unwrap_or(0);

        if class_count >= max_size {
            // O(log n) oldest lookup via BTreeMap, then remove with index cleanup
            let oldest_id = self
                .entries_by_class
                .get(&class)
                .and_then(|tree| tree.first_key_value().map(|(_, id)| id.clone()));
            if let Some(id) = oldest_id {
                self.remove_entry(&id);
            }
        }

        let id = entry.id.clone();
        self.entries.insert(id.clone(), entry);
        self.class_counts
            .entry(class.clone())
            .and_modify(|c| *c += 1)
            .or_insert(1);
        self.entries_by_class
            .entry(class)
            .or_default()
            .insert(seq, id.clone());

        // Enforce total capacity safety net across all classes.
        if self.entries.len() > MAX_ENTRIES {
            // O(log n) global min across all class trees
            let oldest = self
                .entries_by_class
                .values()
                .filter_map(|tree| tree.first_key_value())
                .min_by_key(|(seq, _)| *seq)
                .map(|(_, id)| id.clone());
            if let Some(id) = oldest {
                self.remove_entry(&id);
            }
        }
    }

    /// Remove an entry and update indexes.
    fn remove_entry(&mut self, id: &str) {
        if let Some(entry) = self.entries.remove(id) {
            let class = &entry.class;
            if let Some(count) = self.class_counts.get_mut(class) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.class_counts.remove(class);
                }
            }
            if let Some(tree) = self.entries_by_class.get_mut(class) {
                let seq_to_remove: Vec<u64> = tree
                    .iter()
                    .filter(|(_, v)| *v == id)
                    .map(|(k, _)| *k)
                    .collect();
                for seq in seq_to_remove {
                    tree.remove(&seq);
                }
                if tree.is_empty() {
                    self.entries_by_class.remove(class);
                }
            }
        }
    }

    pub fn retrieve(&self, class: MemoryClass, limit: usize) -> Vec<MemoryEntry> {
        self.entries_by_class
            .get(&class)
            .into_iter()
            .flat_map(|tree| tree.values())
            .filter_map(|id| self.entries.get(id))
            .filter(|e| self.policy.should_retain(e))
            .take(limit)
            .cloned()
            .collect()
    }

    /// Run garbage collection: remove entries that fail `should_retain`.
    pub fn gc(&mut self) {
        let ids_to_remove: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, e)| !self.policy.should_retain(e))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids_to_remove {
            self.remove_entry(&id);
        }
    }

    /// Enforce capacity limits across all classes after bulk operations.
    /// Evicts the oldest entries per class that exceed their max_size.
    pub fn enforce_capacity(&mut self) {
        let classes = [
            MemoryClass::Transient,
            MemoryClass::Episodic,
            MemoryClass::Semantic,
            MemoryClass::ProjectState,
            MemoryClass::Observation,
        ];
        for class in classes {
            let max_size = self.class_max_size(&class);
            let class_count = self.class_counts.get(&class).copied().unwrap_or(0);
            if class_count > max_size {
                let excess = class_count - max_size;
                if let Some(tree) = self.entries_by_class.get(&class) {
                    let to_remove: Vec<String> =
                        tree.iter().take(excess).map(|(_, id)| id.clone()).collect();
                    for id in to_remove {
                        self.remove_entry(&id);
                    }
                }
            }
        }
    }

    /// Promote high-usefulness entries up one memory class level.
    ///
    /// Promotion thresholds:
    /// - Observation  (usefulness ≥ 0.75, staleness = 0) → Episodic
    /// - Episodic     (usefulness ≥ 0.80)                → Semantic
    /// - Semantic     (usefulness ≥ 0.90)                → ProjectState
    pub fn promote(&mut self) -> MemoryPromotionReport {
        let mut to_promote: Vec<(String, MemoryClass, MemoryClass)> = Vec::new();

        for (id, entry) in &self.entries {
            let new_class = match entry.class {
                MemoryClass::Observation if entry.usefulness >= 0.75 && entry.staleness == 0 => {
                    Some(MemoryClass::Episodic)
                }
                MemoryClass::Episodic if entry.usefulness >= 0.80 => Some(MemoryClass::Semantic),
                MemoryClass::Semantic if entry.usefulness >= 0.90 => {
                    Some(MemoryClass::ProjectState)
                }
                _ => None,
            };
            if let Some(new_class) = new_class {
                to_promote.push((id.clone(), entry.class.clone(), new_class));
            }
        }

        let promoted_count = to_promote.len();
        let mut promotion_map = Vec::with_capacity(promoted_count);

        for (id, from_class, to_class) in to_promote {
            let from_name = format!("{:?}", from_class);
            let to_name = format!("{:?}", to_class);
            if let Some(entry) = self.entries.get_mut(&id) {
                entry.class = to_class.clone();

                // Update entries_by_class index: remove from old class tree
                if let Some(tree) = self.entries_by_class.get_mut(&from_class) {
                    let old_seq = tree.iter().find(|(_, v)| *v == &id).map(|(k, _)| *k);
                    if let Some(k) = old_seq {
                        tree.remove(&k);
                    }
                    if tree.is_empty() {
                        self.entries_by_class.remove(&from_class);
                    }
                }

                // Update entries_by_class index: add to new class tree
                let seq = self.store_sequence;
                self.store_sequence += 1;
                self.entries_by_class
                    .entry(to_class.clone())
                    .or_default()
                    .insert(seq, id.clone());

                // Update class_counts: decrement old class
                if let Some(count) = self.class_counts.get_mut(&from_class) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        self.class_counts.remove(&from_class);
                    }
                }

                // Update class_counts: increment new class
                self.class_counts
                    .entry(to_class)
                    .and_modify(|c| *c += 1)
                    .or_insert(1);
            }
            promotion_map.push((id, from_name, to_name));
        }

        MemoryPromotionReport {
            promoted_count,
            promotion_map,
        }
    }
}
