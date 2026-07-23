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
    /// Maximum wall-clock time in milliseconds.
    pub max_wall_clock_ms: Option<u64>,
}

impl AgentExecutionBudget {
    /// Create a new execution budget with default values.
    ///
    /// Defaults: no token ceiling, depth=10, concurrency=8, no time limit.
    pub fn new() -> Self {
        Self {
            aggregate_token_ceiling: None,
            aggregate_tokens_used: 0,
            max_depth: 10,
            max_concurrency: 8,
            active_children: 0,
            max_wall_clock_ms: None,
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

    /// Set the wall-clock time limit in milliseconds.
    pub fn with_max_wall_clock(mut self, ms: u64) -> Self {
        self.max_wall_clock_ms = Some(ms);
        self
    }

    /// Check whether spawning a child at the given depth is allowed.
    pub fn can_spawn(&self, current_depth: u32) -> bool {
        if current_depth >= self.max_depth {
            return false;
        }
        if self.active_children >= self.max_concurrency {
            return false;
        }
        true
    }

    /// Record that a child agent was spawned.
    /// Returns an error if limits are exceeded.
    pub fn record_spawn(&mut self, current_depth: u32) -> Result<(), String> {
        if !self.can_spawn(current_depth) {
            return Err(format!(
                "cannot spawn: depth={}/{} or concurrency={}/{} exceeded",
                current_depth, self.max_depth, self.active_children, self.max_concurrency
            ));
        }
        self.active_children += 1;
        Ok(())
    }

    /// Record that a child agent completed.
    pub fn record_completion(&mut self) {
        self.active_children = self.active_children.saturating_sub(1);
    }

    /// Record token usage. Returns true if ceiling was exceeded.
    pub fn record_tokens(&mut self, tokens: u64) -> bool {
        self.aggregate_tokens_used += tokens;
        if let Some(ceiling) = self.aggregate_token_ceiling {
            self.aggregate_tokens_used >= ceiling
        } else {
            false
        }
    }

    /// Reset the budget (for reuse).
    pub fn reset(&mut self) {
        self.aggregate_tokens_used = 0;
        self.active_children = 0;
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
    fn test_can_spawn_within_limits() {
        let budget = AgentExecutionBudget::new()
            .with_max_depth(3)
            .with_max_concurrency(2);
        assert!(budget.can_spawn(1)); // depth 1 < 3, active=0
    }

    #[test]
    fn test_cannot_spawn_exceed_depth() {
        let budget = AgentExecutionBudget::new().with_max_depth(2);
        assert!(!budget.can_spawn(2)); // depth 2 >= 2
        assert!(!budget.can_spawn(3)); // depth 3 >= 2
    }

    #[test]
    fn test_cannot_spawn_exceed_concurrency() {
        let mut budget = AgentExecutionBudget::new().with_max_concurrency(1);
        budget.active_children = 1;
        assert!(!budget.can_spawn(0));
    }

    #[test]
    fn test_record_spawn_and_completion() {
        let mut budget = AgentExecutionBudget::new()
            .with_max_depth(5)
            .with_max_concurrency(2);
        assert!(budget.record_spawn(0).is_ok());
        assert_eq!(budget.active_children, 1);
        assert!(budget.record_spawn(1).is_ok());
        assert_eq!(budget.active_children, 2);
        assert!(budget.record_spawn(2).is_err()); // concurrency exceeded

        budget.record_completion();
        assert_eq!(budget.active_children, 1);
        assert!(budget.record_spawn(2).is_ok());
    }

    #[test]
    fn test_token_ceiling() {
        let mut budget = AgentExecutionBudget::new().with_token_ceiling(1000);
        assert!(!budget.record_tokens(500));
        assert!(!budget.is_exceeded());
        assert!(budget.record_tokens(600)); // exceeded: 1100 >= 1000
        assert!(budget.is_exceeded());
    }

    #[test]
    fn test_reset() {
        let mut budget = AgentExecutionBudget::new()
            .with_token_ceiling(1000)
            .with_max_concurrency(5);
        let _ = budget.record_spawn(0);
        let _ = budget.record_tokens(500);
        budget.reset();
        assert_eq!(budget.aggregate_tokens_used, 0);
        assert_eq!(budget.active_children, 0);
    }
}
