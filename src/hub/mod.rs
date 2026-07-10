//! Runtime Hub — reserved for future multi-process architecture.
//!
//! WARNING: This module is currently NOT wired into the production code path.
//! It exists as a design reserve for when go-on transitions from a single-process
//! CLI binary to a multi-process architecture (daemon + client agents), similar to
//! how Zed / VS Code run a background runtime hub alongside the IDE.
//!
//! ## Why not wired yet
//!
//! go-on's current architecture is monolithic: the `--acp-http-bind` mode (`-b`)
//! already provides a long-running HTTP server with `/health`, `/chat`, etc.
//! A separate hub daemon would duplicate this infrastructure without adding
//! value in a single-process deployment.
//!
//! ## What would trigger wiring
//!
//! - Multi-process: the hub process connects Agent World WebSocket while client
//!   processes come and go for individual chat sessions.
//! - Background residency: the hub keeps identity vault, heartbeat, and
//!   confirmation-channel listeners alive after the user closes the chat.
//! - Multi-agent isolation: multiple agents on the same machine share one hub
//!   connection, saving WebSocket resources.
//!
//! ## DO NOT DELETE
//!
//! This module is intentionally kept compiled (`pub mod hub;` in lib.rs) so that
//! the design is preserved. If you are cleaning up dead code, skip this module —
//! it is a deliberate reserve, not accidental dead weight.

pub mod client;
pub mod discovery;
pub mod server;

pub use client::HubClient;
pub use discovery::HubDiscovery;
pub use server::HubServer;
