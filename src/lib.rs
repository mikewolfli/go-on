#![recursion_limit = "2048"]
// deprecated items used by tests of deprecated API surface —
// per-item #[allow(deprecated)] at each call site covers remaining uses

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
pub mod i18n;
pub mod intelligence;
pub mod mcp;
pub mod memory;
pub mod multimodal;
pub mod observability;
pub mod optimization;
pub mod orchestration;
pub mod protocol;
pub mod resilience;
pub mod schema;
pub mod security;
pub mod shared;

// ── Re-exports (mirrors main.rs so `crate::agent::*` etc. resolve) ────────
pub use crate::agents::agent;
pub use crate::core::config;
pub use crate::core::config_validation;
pub use crate::core::context;
pub use crate::core::error;
pub use crate::core::provider::OrchestrationProvider;
pub use crate::core::setup;
pub use crate::governance::audit;
pub use crate::governance::drift;
pub use crate::governance::hardening;
pub use crate::governance::harness_bus;
pub use crate::governance::pua;
pub use crate::governance::rationalization;
pub use crate::governance::rbac;
pub use crate::governance::review_controls;
pub use crate::governance::runtime_controls;
pub use crate::governance::security_governor;
pub use crate::i18n::runtime;
// Public re-export for external consumers (SDK / GUI integrations).
// Internal modules should use `crate::i18n::watcher` directly.
pub use crate::i18n::watcher as i18n_watcher;
pub use crate::intelligence::adaptive_selector;
pub use crate::intelligence::evaluation;
pub use crate::intelligence::model_selector;
pub use crate::intelligence::quality_models;
pub use crate::intelligence::reinforcement;
pub use crate::intelligence::verification;
pub use crate::memory::cache;
pub use crate::memory::memory as memory_module;
pub use crate::memory::memory_response_cache;
pub use crate::memory::vector;
pub use crate::observability::observability as observability_module;
pub use crate::observability::performance;
pub use crate::observability::telemetry;
pub use crate::observability::telemetry_enhanced;
pub use crate::optimization::failure_prevention;
pub use crate::orchestration::flow;
pub use crate::orchestration::flow_with_models;
pub use crate::orchestration::mode;
pub use crate::orchestration::orchestrator;
pub use crate::orchestration::roles;
pub use crate::orchestration::task_decomposer;
pub use crate::orchestration::task_graph;
pub use crate::orchestration::task_router;
pub use crate::orchestration::tool;
pub use crate::protocol::mcp_server;
pub use crate::protocol::rpc_protocol;

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
    feature = "profile-local",
    feature = "profile-simple-server",
    feature = "profile-multi-users-server",
    feature = "profile-full",
)))]
compile_error!(
    "No profile feature is enabled. Exactly one must be selected: \
     profile-local, profile-simple-server, profile-multi-users-server, or profile-full"
);

// Fail if more than one profile is selected simultaneously.
#[cfg(any(
    all(feature = "profile-local", feature = "profile-simple-server"),
    all(feature = "profile-local", feature = "profile-multi-users-server"),
    all(feature = "profile-local", feature = "profile-full"),
    all(
        feature = "profile-simple-server",
        feature = "profile-multi-users-server"
    ),
    all(feature = "profile-simple-server", feature = "profile-full"),
    all(feature = "profile-multi-users-server", feature = "profile-full"),
))]
compile_error!(
    "Exactly one profile feature must be enabled. \
     Choose one of: profile-local, profile-simple-server, profile-multi-users-server, or profile-full"
);
