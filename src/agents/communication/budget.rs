//! AgentExecutionBudget — execution control for agent trees (BLUE70 §3.5, §7)
//!
//! Tracks and enforces resource limits on agent sub-trees:
//! token ceilings, depth limits, concurrency caps, and wall-clock timeouts.

use serde::{Deserialize, Serialize};

/// Execution control state for a sub-tree of agents.
///
/// Design notes (simplified vs original):
/// - Removed `started_at_ms` — managed by the caller.
/// - Removed `SpawnReservation` — uses direct Semaphore::acquire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentExecutionBudget {
    /// Total token ceiling for the entire sub-tree.
    pub aggregate_token_ceiling: Option<u64>,
    /// Tokens currently used by this sub-tree.
    pub aggregate_tokens_used: u64,
    /// Maximum depth of the agent tree.
    pub max_depth: u32,
    /// Maximum concurrent child agents.
    pub max_concurrency: usize,
    /// Currently active child agents.
    pub active_children: usize,
}

impl AgentExecutionBudget {
    /// Create a new execution budget with default values.
    ///
    /// Defaults: no token ceiling, depth=10, concurrency=8.
    pub fn new() -> Self {
        Self {
            aggregate_token_ceiling: None,
            aggregate_tokens_used: 0,
            max_depth: 10,
            max_concurrency: 8,
            active_children: 0,
        }
    }

    /// Set the aggregate token ceiling.
    pub fn with_token_ceiling(mut self, ceiling: u64) -> Self {
        self.aggregate_token_ceiling = Some(ceiling);
        self
    }

    /// Set the maximum depth.
    pub fn with_max_depth(mut self, depth: u32) -> Self {
        self.max_depth = depth;
        self
    }

    /// Set the maximum concurrency.
    pub fn with_max_concurrency(mut self, concurrency: usize) -> Self {
        self.max_concurrency = concurrency;
        self
    }

    /// Whether the budget has been exceeded.
    pub fn is_exceeded(&self) -> bool {
        if let Some(ceiling) = self.aggregate_token_ceiling {
            if self.aggregate_tokens_used >= ceiling {
                return true;
            }
        }
        false
    }
}

impl Default for AgentExecutionBudget {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_budget() {
        let budget = AgentExecutionBudget::new();
        assert_eq!(budget.max_depth, 10);
        assert_eq!(budget.max_concurrency, 8);
        assert!(budget.aggregate_token_ceiling.is_none());
    }

    #[test]
    fn test_token_ceiling() {
        let mut budget = AgentExecutionBudget::new().with_token_ceiling(1000);
        budget.aggregate_tokens_used = 500;
        assert!(!budget.is_exceeded());
        budget.aggregate_tokens_used = 1100;
        assert!(budget.is_exceeded());
    }
}
