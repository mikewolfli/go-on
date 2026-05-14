pub mod config;
pub mod config_validation;
pub mod context;
pub mod error;
pub mod setup;

/// Compile-time embedded provider specs — ensures full provider list is always
/// available even when the binary is run from a directory without the file.
/// Shared by both `core/setup.rs` and `core/config.rs`.
pub(crate) const EMBEDDED_PROVIDERS_TOML: &str = include_str!("providers_data.toml");
