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
//! - `session_log` - Append-only session log (M1.4: factual event source for conversations)
//! - `tests` - Test utilities

// Core modules
pub mod background;
pub mod helpers;
pub mod r#impl;
pub mod method_names;
pub mod prelude;
pub mod server;
pub mod session_log;
pub mod transport;
pub mod transport_factory;

// Explicit re-exports of items that external consumers need.
// Avoid `pub use prelude::*;` to make dead-code detection easier.
// Note: keep the full list — each entry also keeps the corresponding
// prelude re-export chain "used" in the binary crate (removing any entry
// surfaces unused-import warnings in `prelude/mod.rs` / `prelude/re_exports.rs`).
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
// `ServerBuilder` is re-exported for downstream consumers (referenced by the
// ACP dispatch tests); `AcpServer` is only used via the `server` sub-module
// path, so it is no longer re-exported here.
#[allow(unused_imports)]
pub use server::ServerBuilder; // re-exported for downstream consumers

// Note: The tests module is only available in test configuration
#[cfg(test)]
pub mod tests;
#[cfg(test)]
#[allow(unused_imports)]
pub use tests::*; // re-exported for test configuration
