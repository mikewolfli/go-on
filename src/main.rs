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

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");
    runtime.block_on(main_module::main());
    // Runtime::drop waits forever for blocking-pool threads when a
    // spawn_blocking task is stuck in uncancellable I/O (e.g. keyring D-Bus or
    // a peer that never closes a pipe). Bound the teardown so process exit can
    // never hang; graceful shutdown already happened in the server code.
    runtime.shutdown_timeout(std::time::Duration::from_secs(5));
}
