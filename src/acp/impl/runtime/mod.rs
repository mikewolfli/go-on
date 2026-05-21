//! Runtime implementation functions for ACP server
//!
//! This module contains standalone functions that implement the core runtime
//! functionality previously in the `impl AcpServer` block.
//! These functions take `AcpServer` as their first parameter to maintain
//! compatibility with the original implementation.
//!
//! Sub-modules:
//! - `server`: Server creation, lifecycle, and HTTP routing
//! - `openai`: OpenAI-compatible chat completions API handlers
//! - `responses`: Responses API handlers

pub mod openai;
pub mod responses;
pub mod server;

// Re-export all public items from sub-modules so that
// `crate::acp::r#impl::runtime::*` works unchanged.
pub use openai::*;
pub use responses::*;
pub use server::*;
