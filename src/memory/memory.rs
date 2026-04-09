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

impl MemoryPolicy {
    pub fn default() -> Self {
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

    pub fn should_retain(&self, entry: &MemoryEntry) -> bool {
        entry.usefulness >= self.usefulness_threshold && entry.staleness <= self.staleness_max_days
    }
}

/// Memory store with policy management
pub struct MemoryStore {
    entries: HashMap<String, MemoryEntry>,
    policy: MemoryPolicy,
}

impl MemoryStore {
    #[allow(dead_code)]
    pub fn new(policy: MemoryPolicy) -> Self {
        Self {
            entries: HashMap::new(),
            policy,
        }
    }

    #[allow(dead_code)]
    pub fn store(&mut self, entry: MemoryEntry) {
        self.entries.insert(entry.id.clone(), entry);
    }

    #[allow(dead_code)]
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
}
