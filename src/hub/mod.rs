//! Runtime Hub — reserved for future multi-process architecture.
//!
//! ## Status
//! This module is **intentionally preserved** as a design reserve but is
//! NOT wired into production code paths. It is gated behind the
//! `sub-bus-distributed-memory` feature, which is only enabled in the
//! `simple-server`, `multi-users-server`, and `full` profiles.
//!
//! In the `local` profile (default), this module is **not compiled**.
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
