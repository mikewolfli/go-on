pub mod bootstrap;
pub mod config;
pub mod config_validation;

pub mod error;
pub mod onboarding;
pub mod providers;
pub mod setup;

// Provider specs are now in a single location: `core::providers`
// (`provider_specs()`, `provider_spec_by_name()`, `provider_spec_by_agent_type()`).
