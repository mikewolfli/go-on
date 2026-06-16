//! ACP Prelude - Constants
//!
//! Shared constants used throughout the ACP system.

use std::time::Duration;

/// Default circuit breaker failure threshold
#[cfg_attr(not(test), allow(dead_code))] // F-GAP-49 — planned wiring
pub const DEFAULT_BREAKER_FAILURE_THRESHOLD: u32 = 3;
/// Default circuit breaker open time in seconds
#[cfg_attr(not(test), allow(dead_code))] // F-GAP-49 — planned wiring
pub const DEFAULT_BREAKER_OPEN_SECONDS: i64 = 60;
/// Maximum conversation ID length
#[cfg_attr(not(test), allow(dead_code))] // F-GAP-49 — planned wiring
pub const MAX_CONVERSATION_ID_LEN: usize = 128;
/// Maximum branch ID length
#[cfg_attr(not(test), allow(dead_code))] // F-GAP-49 — planned wiring
pub const MAX_BRANCH_ID_LEN: usize = 64;
/// Maximum checkpoint ID length
#[cfg_attr(not(test), allow(dead_code))] // F-GAP-49 — planned wiring
pub const MAX_CHECKPOINT_ID_LEN: usize = 128;
/// Maximum checkpoints per conversation
#[cfg_attr(not(test), allow(dead_code))] // F-GAP-49 — planned wiring
pub const MAX_CHECKPOINTS_PER_CONVERSATION: usize = 256;
/// Maximum checkpoint message characters
#[cfg_attr(not(test), allow(dead_code))] // F-GAP-49 — planned wiring
pub const MAX_CHECKPOINT_MESSAGE_CHARS: usize = 64_000;
/// Maximum conversations tracked
#[cfg_attr(not(test), allow(dead_code))] // F-GAP-49 — planned wiring
pub const MAX_CONVERSATIONS_TRACKED: usize = 512;
/// Maximum stream chunks
#[cfg_attr(not(test), allow(dead_code))] // F-GAP-49 — planned wiring
pub const MAX_STREAM_CHUNKS: usize = 4_096;
/// Maximum stream characters
#[cfg_attr(not(test), allow(dead_code))] // F-GAP-49 — planned wiring
pub const MAX_STREAM_CHARS: usize = 256_000;

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

/// Histogram buckets for latency measurements (seconds)
#[cfg_attr(not(test), allow(dead_code))] // F-GAP-49 — planned wiring
pub const HISTOGRAM_BUCKETS_SECONDS: [f64; 10] = [
    0.001, // 1ms
    0.005, // 5ms
    0.01,  // 10ms
    0.05,  // 50ms
    0.1,   // 100ms
    0.5,   // 500ms
    1.0,   // 1s
    5.0,   // 5s
    10.0,  // 10s
    60.0,  // 60s
];
