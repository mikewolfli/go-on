//! SpawnGuard — RAII concurrency slot reservation (BLUE71 §5)
//!
//! Provides `SpawnGuard` for RAII-protected concurrency slot management
//! and `SpawnError` for capacity errors.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

// ── SpawnGuard — RAII concurrency slot reservation (BLUE71 §5) ────────

/// RAII guard that reserves a concurrency slot (BLUE71 §5.2).
///
/// On creation, atomically increments the budget counter.
/// On Drop (even during panic), decrements the counter.
///
/// This prevents concurrency slot leaks when agent spawns fail
/// partway through initialization.
#[derive(Debug)]
pub struct SpawnGuard {
    /// Shared atomic budget counter.
    budget: Arc<AtomicU64>,
}

impl SpawnGuard {
    /// Try to reserve a slot in the budget.
    ///
    /// Returns `Err(SpawnError::CapacityExceeded)` if at capacity.
    pub fn try_reserve(budget: Arc<AtomicU64>, max: u64) -> Result<Self, SpawnError> {
        let current = budget.fetch_add(1, Ordering::AcqRel);
        if current >= max {
            // Rollback: decrement the counter we just incremented.
            budget.fetch_sub(1, Ordering::AcqRel);
            return Err(SpawnError::CapacityExceeded { current, max });
        }
        Ok(Self { budget })
    }
}

impl Drop for SpawnGuard {
    fn drop(&mut self) {
        self.budget.fetch_sub(1, Ordering::AcqRel);
    }
}

// ── SpawnError — errors that can occur during agent spawn ─────────────

/// Errors that can occur when spawning an AgentThread.
#[derive(Debug, Clone, thiserror::Error)]
pub enum SpawnError {
    /// Concurrency capacity exceeded.
    #[error("capacity exceeded: current={current} max={max}")]
    CapacityExceeded {
        /// Current usage.
        current: u64,
        /// Maximum allowed.
        max: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_spawn_guard_capacity_exceeded() {
        let budget = Arc::new(AtomicU64::new(0)); // start at 0 used
        let max: u64 = 2;

        // First two should succeed.
        let g1 = SpawnGuard::try_reserve(budget.clone(), max).unwrap();
        let g2 = SpawnGuard::try_reserve(budget.clone(), max).unwrap();

        // Third should fail.
        let g3 = SpawnGuard::try_reserve(budget.clone(), max);
        assert!(g3.is_err());
        assert!(matches!(
            g3.unwrap_err(),
            SpawnError::CapacityExceeded { .. }
        ));

        // Drop g2 — slot released.
        drop(g2);

        // Now we can reserve again.
        let g4 = SpawnGuard::try_reserve(budget.clone(), max);
        assert!(g4.is_ok());

        drop(g1);
        drop(g4);
    }

    #[tokio::test]
    async fn test_spawn_guard_drop_releases_slot() {
        let budget = Arc::new(AtomicU64::new(0));

        let guard = SpawnGuard::try_reserve(budget.clone(), 2).unwrap();
        // Budget went from 0 → 1.
        assert_eq!(budget.load(Ordering::Acquire), 1);

        drop(guard);
        // Budget goes back to 0 on drop.
        assert_eq!(budget.load(Ordering::Acquire), 0);
    }
}
