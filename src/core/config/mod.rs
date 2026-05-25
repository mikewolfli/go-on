//! Configuration system implementation
//!
//! This module defines the configuration structures and validation logic for the go-on application.

pub mod autotune;
pub mod defaults;
pub mod hot_reload;
pub mod load;
pub mod schema_version;
pub mod types;

// Re-export everything from each sub-module so that existing import paths
// like `crate::core::config::AppConfig`, `crate::core::config::RuntimeConfig`,
// `crate::config::ConfigWarning`, etc. all continue to work unchanged.
pub use autotune::*;
pub use defaults::*;
pub use load::*;
pub use types::*;

// Suppress dead-code warnings for not-yet-integrated modules.
// These modules are publicly exported and will be fully wired in upcoming integrations.
#[cfg(test)]
mod integration_gate {
    fn _gate_schema_manager() {
        let _ = super::schema_version::SchemaManager::default();
    }
}
