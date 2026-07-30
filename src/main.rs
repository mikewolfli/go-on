#![recursion_limit = "2048"]
// Production #[allow(deprecated)] annotations have been migrated to
// targeted #[expect(deprecated)] or removed.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod acp;
mod agents;
mod cli;
mod core;
mod fault_tolerance;
mod governance;

// Runtime Hub — reserved for multi-process architecture (daemon + client agents).
// Only compiled in distributed-memory server builds.
#[cfg(feature = "sub-bus-distributed-memory")]
mod hub;

mod i18n;
mod intelligence;
mod mcp;
mod memory;
mod multimodal;
mod observability;
mod optimization;
mod orchestration;
mod protocol;
mod resilience;
mod schema;
mod security;
mod shared;

#[path = "main/mod.rs"]
mod main_module;

// Shared re-exports — single source of truth at src/_reexports.rs.
include!("_reexports.rs");

#[tokio::main]
async fn main() {
    main_module::main().await;
}
