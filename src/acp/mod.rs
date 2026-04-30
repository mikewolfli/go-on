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

// Re-export only the items that external consumers need.
// NOTE: Changing this to explicit re-exports would help detect dead code.
pub use prelude::*;
// `AcpServer` / `ServerBuilder` are re-exported for downstream consumers; allow unused here.
#[allow(unused_imports)]
pub use server::AcpServer;
#[allow(unused_imports)]
pub use server::ServerBuilder;

// Note: The tests module is only available in test configuration
#[cfg(test)]
pub mod tests;
#[cfg(test)]
#[allow(unused_imports)]
pub use tests::*;
