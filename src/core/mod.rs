pub mod bootstrap;
pub mod config;
pub mod config_validation;
pub mod context;
pub mod error;
pub mod onboarding;
pub mod setup;

// Provider specs are fully hardcoded in `built_in_provider_specs()`
// within `config.rs` and `setup.rs`. No external TOML file needed.
