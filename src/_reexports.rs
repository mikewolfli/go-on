// Shared re-exports — single source of truth included by both lib.rs and main.rs.
// Adding or renaming a module only needs a change here, not in two files.

pub use crate::agents::agent;
pub use crate::core::config;
pub use crate::core::config_validation;

pub use crate::core::error;

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
pub use crate::governance::status;
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
pub use crate::memory::vector;
pub use crate::observability::observability as observability_module;
pub use crate::observability::performance;
pub use crate::observability::telemetry_enhanced;
pub use crate::orchestration::flow;
pub use crate::orchestration::mode;
pub use crate::orchestration::orchestrator;
pub use crate::orchestration::roles;
pub use crate::orchestration::task_decomposer;
pub use crate::orchestration::task_router;
pub use crate::orchestration::tool;
pub use crate::protocol::mcp_server;
pub use crate::protocol::rpc_protocol;
