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
// Agent-related implementation functions
pub mod agent;
// I/O implementation functions
pub mod io;
// Storage implementation functions
pub mod storage;
// CORS support for ACP HTTP server
pub mod cors;
// User session management
pub mod session;
// Extracted chat phases (BLUE62 ARCH-1)
pub mod chat_phases;

// Re-export for convenience — retained for ACP consumer API surface.
#[allow(unused_imports)]
pub use runtime::*; // re-exported for ACP consumer API surface
pub use session::UserSession;
