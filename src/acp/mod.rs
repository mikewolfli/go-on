#![allow(dead_code)]

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

// Re-export for convenience
#[allow(unused_imports)]
pub use background::*;
#[allow(unused_imports)]
pub use helpers::*;
#[allow(unused_imports)]
pub use prelude::*;
#[allow(unused_imports)]
pub use r#impl::*;
#[allow(unused_imports)]
pub use server::*;

// Note: The tests module is only available in test configuration
#[cfg(test)]
pub mod tests;
#[cfg(test)]
#[allow(unused_imports)]
pub use tests::*;
