//! Configuration system implementation
//!
//! This module defines the configuration structures and validation logic for the go-on application.

pub mod autotune;
pub mod defaults;
pub mod load;
pub mod types;

// Re-export everything from each sub-module so that existing import paths
// like `crate::core::config::AppConfig`, `crate::core::config::RuntimeConfig`,
// `crate::config::ConfigWarning`, etc. all continue to work unchanged.
pub use autotune::*;
pub use defaults::*;
pub use load::*;
pub use types::*;
