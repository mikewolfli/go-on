//! Memory policy layer for go-on (Phase 2/3)
//!
//! These structures are intentional framework definitions for Phase 0-9 architecture.
//! Memory classes and policies define how artifacts are retained and promoted,
//! to be integrated into the execution flow once promotion logic is wired.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

/// Memory store with policy management
#[derive(Debug)]
pub struct MemoryStore {
    entries: HashMap<String, MemoryEntry>,
    policy: MemoryPolicy,
}

impl MemoryStore {
    pub fn new(policy: MemoryPolicy) -> Self {
        Self {
            entries: HashMap::new(),
            policy,
        }
    }

    pub fn store(&mut self, entry: MemoryEntry) {
        self.entries.insert(entry.id.clone(), entry);
    }

    pub fn retrieve(&self, class: MemoryClass, limit: usize) -> Vec<MemoryEntry> {
        self.entries
            .values()
            .filter(|e| e.class == class && self.policy.should_retain(e))
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn gc(&mut self) {
        self.entries.retain(|_, e| self.policy.should_retain(e));
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
                entry.class = to_class;
            }
            promotion_map.push((id, from_name, to_name));
        }

        MemoryPromotionReport {
            promoted_count,
            promotion_map,
        }
    }
}
