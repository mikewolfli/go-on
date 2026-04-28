
//! BLUE35 S10: Fork Registry — Sub-agent Process Isolation (ARCH-05)
//!
//! Tracks forked sub-agent executions and provides isolation boundaries
//! (sandbox level, resource limits, timeout policies) for each fork.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Isolation level for a forked sub-agent
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum IsolationLevel {
    /// No isolation — runs in the same process/memory space
    None,
    /// Light isolation — separate task queue, shared memory
    Light,
    /// Standard isolation — separate process, IPC channel
    Standard,
    /// Strict isolation — separate process, limited syscalls
    Strict,
    /// Full sandbox — container-level isolation
    Sandbox,
}

/// Resource budget for a forked execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkBudget {
    pub max_tokens: u64,
    pub max_tool_calls: u32,
    pub max_wall_clock_seconds: u64,
    pub max_memory_mb: u64,
}

impl Default for ForkBudget {
    fn default() -> Self {
        Self {
            max_tokens: 32000,
            max_tool_calls: 32,
            max_wall_clock_seconds: 300,
            max_memory_mb: 512,
        }
    }
}

/// A registered fork entry
#[derive(Debug, Clone)]
pub struct ForkEntry {
    pub fork_id: String,
    pub parent_task_id: String,
    pub isolation: IsolationLevel,
    pub budget: ForkBudget,
    pub created_at: Instant,
    pub completed: bool,
}

/// Registry of all active forks
#[derive(Debug, Default)]
pub struct ForkRegistry {
    forks: HashMap<String, ForkEntry>,
    max_forks: usize,
}

impl ForkRegistry {
    pub fn new(max_forks: usize) -> Self {
        Self {
            forks: HashMap::new(),
            max_forks,
        }
    }

    /// Register a new fork, returning its ID.
    /// Returns None if at capacity.
    pub fn register(
        &mut self,
        parent_task_id: &str,
        isolation: IsolationLevel,
        budget: ForkBudget,
    ) -> Option<String> {
        if self.forks.len() >= self.max_forks {
            return None;
        }
        let fork_id = format!("fork-{}-{}", parent_task_id, self.forks.len());
        self.forks.insert(
            fork_id.clone(),
            ForkEntry {
                fork_id: fork_id.clone(),
                parent_task_id: parent_task_id.to_string(),
                isolation,
                budget,
                created_at: Instant::now(),
                completed: false,
            },
        );
        Some(fork_id)
    }

    pub fn complete(&mut self, fork_id: &str) {
        if let Some(entry) = self.forks.get_mut(fork_id) {
            entry.completed = true;
        }
    }

    pub fn get(&self, fork_id: &str) -> Option<&ForkEntry> {
        self.forks.get(fork_id)
    }

    pub fn active_count(&self) -> usize {
        self.forks.values().filter(|e| !e.completed).count()
    }

    pub fn total_count(&self) -> usize {
        self.forks.len()
    }

    /// Clean up completed forks older than the given duration
    pub fn gc(&mut self, max_age: Duration) {
        let now = Instant::now();
        self.forks.retain(|_, entry| {
            if entry.completed && now.duration_since(entry.created_at) > max_age {
                return false;
            }
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_complete() {
        let mut reg = ForkRegistry::new(10);
        let fid = reg.register("parent-1", IsolationLevel::Light, ForkBudget::default());
        assert!(fid.is_some());
        assert_eq!(reg.active_count(), 1);
        reg.complete(&fid.unwrap());
        assert_eq!(reg.active_count(), 0);
    }

    #[test]
    fn test_max_forks() {
        let mut reg = ForkRegistry::new(2);
        assert!(reg
            .register("p1", IsolationLevel::None, ForkBudget::default())
            .is_some());
        assert!(reg
            .register("p2", IsolationLevel::None, ForkBudget::default())
            .is_some());
        assert!(reg
            .register("p3", IsolationLevel::None, ForkBudget::default())
            .is_none());
    }

    #[test]
    fn test_gc_removes_old_completed() {
        let mut reg = ForkRegistry::new(10);
        let fid = reg
            .register("p1", IsolationLevel::None, ForkBudget::default())
            .unwrap();
        reg.complete(&fid);
        assert_eq!(reg.total_count(), 1);
        reg.gc(Duration::from_secs(0));
        assert_eq!(reg.total_count(), 0);
    }
}
