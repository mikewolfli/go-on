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
pub mod method_names;
pub mod prelude;
pub mod server;
pub mod transport;
pub mod transport_factory;

// Explicit re-exports of items that external consumers need.
// Avoid `pub use prelude::*;` to make dead-code detection easier.
#[allow(unused_imports)]
pub use prelude::{
    // re-exported for ACP consumer public API surface
    enforce_checkpoint_capacity,
    now_ts,
    with_acp_lock,
    with_acp_lock_async,
    CircuitBreakerRegistry,
    CircuitBreakerSnapshot,
    ConversationCheckpoint,
    ConversationState,
    InflightLimiter,
    LifecycleSnapshot,
    LifecycleState,
    MaintenanceSnapshot,
    MaintenanceTracker,
    MetricsSnapshot,
    OnlineControllerState,
    PhaseRateLimiter,
    ReviewGateOutcome,
    ReviewTimeoutPolicy,
    RuntimeMetrics,
    ServerStatus,
    MAX_CHECKPOINTS_PER_CONVERSATION,
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
