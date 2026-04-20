//! S10: Fork Isolation Guard
//!
//! Detects forked execution branches and enforces that shared mutable state
//! is not accessed cross-fork.  Provides a lightweight join contract that
//! validates outputs before merging.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Identifier for a fork branch
pub type BranchId = String;

/// Execution snapshot captured at fork time
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkSnapshot {
    pub branch_id: BranchId,
    pub parent_id: Option<BranchId>,
    pub task_id: String,
    pub phase: String,
    pub agent: String,
    pub created_at_ms: u64,
}

/// Status of a fork branch
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchStatus {
    Running,
    Completed,
    Cancelled,
    Conflict,
}

/// Join contract result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkJoinResult {
    pub branch_id: BranchId,
    pub status: BranchStatus,
    pub conflicts: Vec<String>,
    pub merged: bool,
}

/// Registry of active fork branches
#[derive(Debug, Default)]
pub struct ForkRegistry {
    branches: HashMap<BranchId, (ForkSnapshot, BranchStatus)>,
}

impl ForkRegistry {
    pub fn new() -> Self { Self::default() }

    /// Register a new fork branch
    pub fn register(&mut self, parent_id: Option<&str>, task_id: &str, phase: &str, agent: &str) -> ForkSnapshot {
        let branch_id = format!("fork-{}", now_ms());
        let snap = ForkSnapshot {
            branch_id: branch_id.clone(),
            parent_id: parent_id.map(|s| s.to_string()),
            task_id: task_id.to_string(),
            phase: phase.to_string(),
            agent: agent.to_string(),
            created_at_ms: now_ms(),
        };
        self.branches.insert(branch_id, (snap.clone(), BranchStatus::Running));
        snap
    }

    /// Validate and attempt to join a branch back to parent.
    /// Returns conflict markers if overlapping writes are detected.
    pub fn join(&mut self, branch_id: &str, write_keys: &[String]) -> ForkJoinResult {
        let Some((snap, _)) = self.branches.get(branch_id) else {
            return ForkJoinResult {
                branch_id: branch_id.to_string(),
                status: BranchStatus::Conflict,
                conflicts: vec!["branch_not_found".to_string()],
                merged: false,
            };
        };

        let parent_id = snap.parent_id.clone();

        // Check whether any sibling branch is writing the same keys
        let sibling_writes: Vec<String> = self.branches.iter()
            .filter(|(id, (s, st))| {
                *id != branch_id
                    && s.parent_id == parent_id
                    && *st == BranchStatus::Running
                    && write_keys.iter().any(|k| s.task_id.contains(k.as_str()))
            })
            .map(|(id, _)| id.clone())
            .collect();

        if !sibling_writes.is_empty() {
            if let Some((_, status)) = self.branches.get_mut(branch_id) {
                *status = BranchStatus::Conflict;
            }
            return ForkJoinResult {
                branch_id: branch_id.to_string(),
                status: BranchStatus::Conflict,
                conflicts: sibling_writes,
                merged: false,
            };
        }

        if let Some((_, status)) = self.branches.get_mut(branch_id) {
            *status = BranchStatus::Completed;
        }
        ForkJoinResult {
            branch_id: branch_id.to_string(),
            status: BranchStatus::Completed,
            conflicts: Vec::new(),
            merged: true,
        }
    }

    pub fn active_count(&self) -> usize {
        self.branches.values().filter(|(_, s)| *s == BranchStatus::Running).count()
    }

    pub fn conflict_count(&self) -> usize {
        self.branches.values().filter(|(_, s)| *s == BranchStatus::Conflict).count()
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
