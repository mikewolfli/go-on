//! Runtime Hub — reserved for future multi-process architecture.
//!
//! ## Status
//! The Hub is **live via the CLI**: `go-on hub` (see `src/main/mod.rs`) starts
//! the daemon on a loopback port with discovery-file + Bearer-token auth.
//! It is gated behind the `sub-bus-distributed-memory` feature, which is only
//! enabled in the `simple-server`, `multi-users-server`, and `full` profiles.
//!
//! In the `local` profile (default), this module is **not compiled**.
//!
//! Core service paths (the ACP HTTP server and its `/health`, `/chat`, …
//! endpoints) are **not yet connected** to the Hub — that remains a design
//! reserve for the future multi-process architecture described below.
//!
//! ## Why core service paths are not wired yet
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
//! This module is a deliberate design reserve. It is feature-gated behind
//! `sub-bus-distributed-memory` (see `src/lib.rs`), so it is NOT compiled in
//! the default `local` profile. If you are cleaning up dead code, skip this
//! module — it is a deliberate reserve, not accidental dead weight.

pub mod discovery;
pub mod server;

#[allow(unused_imports)]
pub use discovery::HubDiscovery;
#[allow(unused_imports)]
pub use server::HubServer;
