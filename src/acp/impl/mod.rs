//! Implementation modules for ACP server
//!
//! This module contains the core implementation logic for the ACP server,
//! including runtime management, request handling, and server operations.

// Runtime implementation functions
pub mod runtime;
// Request handling implementation functions
pub mod request;
// Chat handling implementation functions
pub mod chat;
// Conversation handling implementation functions
pub mod conversation;
// Agent-related implementation functions
pub mod agent;
// I/O implementation functions
pub mod io;
// Storage implementation functions
pub mod storage;

// Re-export for convenience
pub use runtime::*;

// Note: During migration, this module serves as a bridge between
// the old include! structure and the new modular structure.
// Implementation modules will be added here as they are migrated.
