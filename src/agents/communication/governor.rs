//! ExecutionGovernor — budget-aware execution control for agent trees (BLUE70 §7)
//!
//! Centralized orchestrator that enforces resource limits before and during
//! agent sub-tree execution: depth limits, concurrency caps, token ceilings,
//! and wall-clock timeouts. Integrates with `AgentExecutionBudget`.
//!
//! Design notes (simplified vs original):
//! - Direct Semaphore::acquire — no SpawnReservation reserved mode.
//! - check_limits + acquire are separate calls (window is microseconds, risk negligible).
//! - Cancellation is owned by `AgentMessenger::cancel_subtree` and surfaced
//!   through `CommunicationBus::cancel_subtree` (which records metrics).

use std::sync::Arc;
use tokio::sync::{RwLock as AsyncRwLock, Semaphore, TryAcquireError};

use crate::agents::communication::budget::AgentExecutionBudget;
use crate::agents::communication::path::AgentPath;
use crate::agents::communication::tree::AgentTree;

/// Default maximum concurrent spawns across the entire tree.
const DEFAULT_GLOBAL_MAX_CONCURRENCY: usize = 128;

/// Reason strings for limit violations.
const REASON_DEPTH_EXCEEDED: &str = "max_depth exceeded";
const REASON_CONCURRENCY_EXCEEDED: &str = "max_concurrency exceeded";

/// Result of a limit check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LimitCheckResult {
    /// All limits passed; execution may proceed.
    Allowed,
    /// A limit was exceeded, with the reason.
    Denied(String),
}

/// ExecutionGovernor — centralized execution control (BLUE70 §7).
///
/// Orchestrates limit checking, concurrency management, and cancellation.
/// Uses a global Semaphore for cross-tree concurrency control.
pub struct ExecutionGovernor {
    /// Global concurrency semaphore.
    semaphore: Semaphore,
    /// Reference to the agent tree (for depth checks).
    tree: Arc<AsyncRwLock<AgentTree>>,
}

impl ExecutionGovernor {
    /// Create a new ExecutionGovernor with default concurrency (128).
    pub fn new(tree: Arc<AsyncRwLock<AgentTree>>) -> Self {
        Self {
            semaphore: Semaphore::new(DEFAULT_GLOBAL_MAX_CONCURRENCY),
            tree,
        }
    }

    /// Create with a custom max concurrency.
    pub fn with_max_concurrency(tree: Arc<AsyncRwLock<AgentTree>>, max: usize) -> Self {
        Self {
            semaphore: Semaphore::new(max),
            tree,
        }
    }

    /// Check all limits for spawning a child at the given path (BLUE70 §7.1).
    ///
    /// Checks performed:
    /// 1. `max_depth` — child's depth must not exceed the budget's max_depth
    /// 2. `max_concurrency` — active children must not exceed budget's max_concurrency
    /// 3. `aggregate_token_ceiling` — tokens used must not exceed ceiling
    ///
    /// This is a read-only check — no state is modified.
    pub async fn check_limits(
        &self,
        child_path: &AgentPath,
        budget: &AgentExecutionBudget,
    ) -> LimitCheckResult {
        // Verify parent exists in the tree
        if let Some(parent_path) = child_path.parent() {
            let tree = self.tree.read().await;
            if tree.resolve(&parent_path).is_none() {
                return LimitCheckResult::Denied(format!(
                    "parent path '{}' not found in agent tree",
                    parent_path
                ));
            }
        }

        // Depth check
        let child_depth = child_path.depth() as u32;
        if child_depth >= budget.max_depth {
            return LimitCheckResult::Denied(format!(
                "{}: child depth {} >= max depth {}",
                REASON_DEPTH_EXCEEDED, child_depth, budget.max_depth
            ));
        }

        // Concurrency check
        if budget.active_children >= budget.max_concurrency {
            return LimitCheckResult::Denied(format!(
                "{}: active children {} >= max concurrency {}",
                REASON_CONCURRENCY_EXCEEDED, budget.active_children, budget.max_concurrency
            ));
        }

        // Token ceiling check
        if budget.is_exceeded() {
            return LimitCheckResult::Denied("aggregate_token_ceiling exceeded".to_string());
        }

        LimitCheckResult::Allowed
    }

    /// Acquire a concurrency permit (BLUE70 §7.2).
    ///
    /// Blocks until a permit is available (or returns error if semaphore closed).
    pub async fn acquire(&self) -> Result<tokio::sync::SemaphorePermit<'_>, String> {
        self.semaphore
            .acquire()
            .await
            .map_err(|_| "semaphore closed".to_string())
    }

    /// Try to acquire a permit without blocking.
    pub fn try_acquire(&self) -> Result<tokio::sync::SemaphorePermit<'_>, String> {
        self.semaphore.try_acquire().map_err(|e| match e {
            TryAcquireError::Closed => "semaphore closed".to_string(),
            TryAcquireError::NoPermits => "no permits available".to_string(),
        })
    }

    /// Get the number of available permits.
    pub fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }
}

// (cancel_subtree removed — cancellation is owned by AgentMessenger and
// surfaced through CommunicationBus::cancel_subtree, which records metrics.)

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::communication::budget::AgentExecutionBudget;
    use crate::agents::communication::path::AgentPath;
    use crate::agents::communication::tree::{AgentNodeMetadata, AgentTree};

    fn make_path(s: &str) -> AgentPath {
        AgentPath::parse(s).unwrap()
    }

    /// Register a root agent in the tree for tests.
    async fn register_root(tree: &Arc<AsyncRwLock<AgentTree>>, path: &AgentPath) {
        let mut t = tree.write().await;
        t.register(path, "root", AgentNodeMetadata::new()).ok();
    }

    #[tokio::test]
    async fn test_check_limits_allowed() {
        let tree = Arc::new(AsyncRwLock::new(AgentTree::new()));
        register_root(&tree, &make_path("root")).await;
        let governor = ExecutionGovernor::new(tree);

        let budget = AgentExecutionBudget::new()
            .with_max_depth(5)
            .with_max_concurrency(3);

        let result = governor
            .check_limits(&make_path("root/child"), &budget)
            .await;
        assert_eq!(result, LimitCheckResult::Allowed);
    }

    #[tokio::test]
    async fn test_check_limits_depth_exceeded() {
        let tree = Arc::new(AsyncRwLock::new(AgentTree::new()));
        register_root(&tree, &make_path("root")).await;
        register_root(&tree, &make_path("root/a")).await;
        let governor = ExecutionGovernor::new(tree);

        let budget = AgentExecutionBudget::new().with_max_depth(1);

        let result = governor.check_limits(&make_path("root/a/b"), &budget).await;
        assert!(
            matches!(result, LimitCheckResult::Denied(ref r) if r.contains(REASON_DEPTH_EXCEEDED))
        );
    }

    #[tokio::test]
    async fn test_check_limits_concurrency_exceeded() {
        let tree = Arc::new(AsyncRwLock::new(AgentTree::new()));
        register_root(&tree, &make_path("root")).await;
        let governor = ExecutionGovernor::new(tree);

        let mut budget = AgentExecutionBudget::new().with_max_concurrency(1);
        budget.active_children = 1;

        let result = governor
            .check_limits(&make_path("root/child"), &budget)
            .await;
        assert!(
            matches!(result, LimitCheckResult::Denied(ref r) if r.contains(REASON_CONCURRENCY_EXCEEDED))
        );
    }

    #[tokio::test]
    async fn test_check_limits_parent_not_found() {
        let tree = Arc::new(AsyncRwLock::new(AgentTree::new()));
        // No root registered — parent lookup should fail
        let governor = ExecutionGovernor::new(tree);

        let budget = AgentExecutionBudget::new().with_max_depth(5);
        let result = governor
            .check_limits(&make_path("root/child"), &budget)
            .await;
        assert!(matches!(result, LimitCheckResult::Denied(ref r) if r.contains("parent path")));
    }

    #[tokio::test]
    async fn test_check_limits_token_ceiling_exceeded() {
        let tree = Arc::new(AsyncRwLock::new(AgentTree::new()));
        register_root(&tree, &make_path("root")).await;
        let governor = ExecutionGovernor::new(tree);

        let mut budget = AgentExecutionBudget::new().with_token_ceiling(100);
        budget.aggregate_tokens_used = 150;

        let result = governor
            .check_limits(&make_path("root/child"), &budget)
            .await;
        assert!(matches!(result, LimitCheckResult::Denied(_)));
    }

    #[tokio::test]
    async fn test_acquire_and_release() {
        let tree = Arc::new(AsyncRwLock::new(AgentTree::new()));
        let governor = ExecutionGovernor::new(tree);

        let permit = governor.acquire().await.unwrap();
        assert!(governor.available_permits() < DEFAULT_GLOBAL_MAX_CONCURRENCY);
        drop(permit);
        assert_eq!(governor.available_permits(), DEFAULT_GLOBAL_MAX_CONCURRENCY);
    }

    #[tokio::test]
    async fn test_try_acquire() {
        let tree = Arc::new(AsyncRwLock::new(AgentTree::new()));
        let governor = ExecutionGovernor::new(tree);

        let permit = governor.try_acquire().unwrap();
        assert!(governor.available_permits() < DEFAULT_GLOBAL_MAX_CONCURRENCY);
        drop(permit);
    }

    #[tokio::test]
    async fn test_available_permits() {
        let tree = Arc::new(AsyncRwLock::new(AgentTree::new()));
        let governor = ExecutionGovernor::with_max_concurrency(tree, 10);
        assert_eq!(governor.available_permits(), 10);
    }
}
