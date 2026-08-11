#![recursion_limit = "2048"]
// Production #[allow(deprecated)] annotations have been migrated to
// targeted #[expect(deprecated)] or removed. Remaining test-only
// #[allow(deprecated)] are in #[cfg(test)] blocks.

//! go-on – ACP runtime proxy with integrated multi-agent orchestration.
//!
//! This library crate re-exports modules so that external consumers
//! (for example integration tests) can import them via `use go_on::…`.

// ── Module declarations (mirrors main.rs so internal crate paths resolve) ──
pub mod acp;
pub mod agents;
pub mod cli;
pub mod core;
pub mod fault_tolerance;
pub mod governance;

// Runtime Hub — reserved for multi-process architecture (daemon + client agents).
// Only compiled in distributed-memory server builds.
#[cfg(feature = "sub-bus-distributed-memory")]
pub mod hub;

pub mod i18n;
pub mod intelligence;
pub mod mcp;
pub mod memory;
pub mod multimodal;
pub mod observability;
pub mod orchestration;
pub mod protocol;
pub mod resilience;
pub mod schema;
pub mod security;
pub mod shared;

// ── Shared re-exports (single source of truth, also consumed by main.rs) ──
include!("_reexports.rs");

// ── Backend mutual exclusion ──────────────────────────────────────────────
// Exactly one backend feature must be selected.
#[cfg(all(feature = "backend-sqlite", feature = "backend-postgres"))]
compile_error!(
    "Exactly one backend feature must be enabled. \
     Choose one of: backend-sqlite, backend-postgres"
);

// ── Profile mutual exclusion ──────────────────────────────────────────────
// Exactly ONE profile must be selected. The Cargo feature system does not
// enforce mutual exclusion at the dependency level, so we check explicitly.

// Fail if no profile is selected (e.g. `cargo build --no-default-features`).
#[cfg(not(any(
    feature = "local",
    feature = "simple-server",
    feature = "multi-users-server",
    feature = "full",
)))]
compile_error!(
    "No profile feature is enabled. Exactly one must be selected: \
     local, simple-server, multi-users-server, or full"
);

// Fail if more than one profile is selected simultaneously.
#[cfg(any(
    all(feature = "local", feature = "simple-server"),
    all(feature = "local", feature = "multi-users-server"),
    all(feature = "local", feature = "full"),
    all(feature = "simple-server", feature = "multi-users-server"),
    all(feature = "simple-server", feature = "full"),
    all(feature = "multi-users-server", feature = "full"),
))]
compile_error!(
    "Exactly one profile feature must be enabled. \
     Choose one of: local, simple-server, multi-users-server, or full"
);
