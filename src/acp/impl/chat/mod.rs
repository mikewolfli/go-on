//! Chat handling implementation
//!
//! This module is the modular entry point for chat handling.
//! It re-exports all public items from its sub-modules so that
//! existing import paths like `crate::acp::impl::chat::handle_chat`
//! and `crate::acp::impl::chat::ChatParams` continue to work unchanged.

mod helpers;
mod params;
mod pipeline;
mod risk;

pub use helpers::*;
pub use params::*;
pub use pipeline::*;
pub use risk::*;
