//! ACP (Agent Coordination Protocol) module
//!
//! This module contains the core ACP server implementation and related components
//! for agent coordination, request handling, and system management.
//!
//! # Modular Structure
//! This module uses a proper modular structure organized as follows:
//! - `prelude` - Type definitions, constants, and utility functions
//! - `helpers` - Helper modules (context, policy, misc, requirement, conversation, metrics)
//! - `impl` - Implementation modules (runtime, request, chat, conversation, agent, io, storage)
//! - `server` - Main server implementation
//! - `background` - Background task management
//! - `tests` - Test utilities

// Core modules
pub mod background;
pub mod helpers;
pub mod r#impl;
pub mod prelude;
pub mod server;
pub mod transport_factory;

// Explicit re-exports of items that external consumers need.
// Avoid `pub use prelude::*;` to make dead-code detection easier.
#[allow(unused_imports)]
pub use prelude::{
    // re-exported for ACP consumer public API surface
    checkpoint_message_chars,
    enforce_checkpoint_capacity,
    evict_oldest_conversation,
    now_ts,
    now_ts_ms,
    touch_conversation_order,
    with_acp_lock,
    AcpLockMonitor,
    AcpLockSnapshot,
    ChatParams,
    CircuitBreakerAdmission,
    CircuitBreakerRegistry,
    CircuitBreakerSnapshot,
    ConversationCheckpoint,
    ConversationPruneResult,
    ConversationState,
    InflightGuard,
    InflightLimiter,
    LifecycleSnapshot,
    LifecycleState,
    MaintenanceSnapshot,
    MaintenanceTracker,
    MetricsSnapshot,
    OnlineControllerState,
    PhaseRateLimiter,
    ReviewDecision,
    ReviewGateOutcome,
    ReviewTimeoutPolicy,
    ReviewVerdict,
    RuntimeMetrics,
    ServerStatus,
    ACP_LOCK_CIRCUIT_BREAKERS,
    ACP_LOCK_INFLIGHT_LIMITER,
    ACP_LOCK_LIFECYCLE,
    ACP_LOCK_MAINTENANCE,
    ACP_LOCK_MEMORY_CACHE,
    ACP_LOCK_MEMORY_STORE,
    ACP_LOCK_PHASE_RATE_LIMITER,
    ACP_LOCK_RESPONSE_CACHE,
    ACP_LOCK_RUNTIME_CONFIG,
    ACP_LOCK_VECTOR_STORE,
    DEFAULT_BREAKER_FAILURE_THRESHOLD,
    DEFAULT_BREAKER_OPEN_SECONDS,
    HISTOGRAM_BUCKETS_SECONDS,
    MAX_BRANCH_ID_LEN,
    MAX_CHECKPOINTS_PER_CONVERSATION,
    MAX_CHECKPOINT_ID_LEN,
    MAX_CHECKPOINT_MESSAGE_CHARS,
    MAX_CONVERSATIONS_TRACKED,
    MAX_CONVERSATION_ID_LEN,
    MAX_STREAM_CHARS,
    MAX_STREAM_CHUNKS,
};
// `AcpServer` / `ServerBuilder` are re-exported for downstream consumers; allow unused here.
#[allow(unused_imports)]
pub use server::AcpServer; // re-exported for downstream consumers
#[allow(unused_imports)]
pub use server::ServerBuilder; // re-exported for downstream consumers

// Note: The tests module is only available in test configuration
#[cfg(test)]
pub mod tests;
#[cfg(test)]
#[allow(unused_imports)]
pub use tests::*; // re-exported for test configuration
