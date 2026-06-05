//! Configuration system implementation
//!
//! This module defines the configuration structures and validation logic for the go-on application.

pub mod autotune;
pub mod defaults;
pub mod hot_reload;
pub mod load;
pub mod schema_version;
pub mod types;

// Explicit re-exports from each sub-module so that existing import paths
// like `crate::core::config::AppConfig`, `crate::core::config::RuntimeConfig`,
// `crate::config::ConfigWarning`, etc. all continue to work unchanged.
//
// Internal / pub(crate) types are NOT re-exported.
pub use autotune::{AutoTuneConfig, AutoTuneState};
pub use defaults::default_non_ai_config_toml;

// pub(crate) re-exports for internal use (not part of the public API)
pub(crate) use crate::core::providers::provider_specs;
pub use load::{
    build_config_health_report, collect_config_warnings, collect_production_strict_violations,
    is_agent_env_ready, missing_env_vars, validate_external_secret_refs,
    validate_runtime_readiness, ConfigHealthReport, ConfigWarning, ConfigWarningSeverity,
};
pub use types::{
    AdaptiveConfig, AgentConfig, AppConfig, CacheConfig, ComplianceConfig, ConversationContext,
    FlowConfig, LearningPreferences, MinimalConfig, PhaseConfig, PhaseOptions, ReputationConfig,
    RuntimeConfig, SchedulerConfig, StartupContextConfig, VectorConfig, WorkflowType,
};

// Suppress dead-code warnings for not-yet-integrated modules.
// These modules are publicly exported and will be fully wired in upcoming integrations.
#[cfg(test)]
mod integration_gate {
    fn _gate_schema_manager() {
        let _ = super::schema_version::SchemaManager::default();
    }
}
