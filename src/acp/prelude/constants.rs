//! ACP Prelude - Constants
//!
//! Shared constants used throughout the ACP system.

use std::time::Duration;

/// Maximum checkpoints per conversation
pub const MAX_CHECKPOINTS_PER_CONVERSATION: usize = 256;
/// Maximum conversations tracked
///
/// Public API constant — re-exported for ACP consumers.
#[allow(dead_code)]
pub const MAX_CONVERSATIONS_TRACKED: usize = 512;

pub const ACP_LOCK_RUNTIME_CONFIG: &str = "runtime_config";
pub const ACP_LOCK_MEMORY_CACHE: &str = "memory_cache";
pub const ACP_LOCK_MEMORY_STORE: &str = "memory_store";
pub const ACP_LOCK_RESPONSE_CACHE: &str = "response_cache";
pub const ACP_LOCK_VECTOR_STORE: &str = "vector_store";
pub const ACP_LOCK_MAINTENANCE: &str = "maintenance_tracker";
pub const ACP_LOCK_LIFECYCLE: &str = "lifecycle_state";
pub const ACP_LOCK_CIRCUIT_BREAKERS: &str = "circuit_breakers";
pub const ACP_LOCK_PHASE_RATE_LIMITER: &str = "phase_rate_limiter";
pub const ACP_LOCK_INFLIGHT_LIMITER: &str = "inflight_limiter";

/// Slow lock wait threshold (used by lock monitor).
/// `pub(crate)` so sibling modules under `prelude/` can access it.
pub(crate) const ACP_LOCK_SLOW_WAIT_THRESHOLD: Duration = Duration::from_millis(5);
