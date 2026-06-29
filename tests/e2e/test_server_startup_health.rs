//! Server Startup Health End-to-End
//!
//! Validates that server startup configuration and core subsystems
//! initialize correctly. Uses structural assertions against config
//! defaults and module initialization functions.

use go_on::config::RuntimeConfig;
use go_on::governance::status::GovernanceStatus;

/// Verify that server startup config defaults do not panic and have
/// expected initial values.
#[test]
fn test_default_config_is_well_formed() {
    let config = RuntimeConfig::default();
    // Default should always have a service name and be non-empty
    assert!(
        !config.otel_service_name.is_empty(),
        "otel_service_name must be set"
    );
    // Default governance should be enabled (default per schema)
    assert!(
        config.governance_enabled,
        "governance must be enabled by default"
    );
    // Default health interval should be reasonable (>0)
    assert!(
        config.health_interval_seconds > 0,
        "health_interval_seconds must be positive"
    );
}

/// Verify that the governance status module initialises correctly.
#[test]
fn test_governance_status_has_defaults() {
    let status = GovernanceStatus::default();
    // Default governance should not be healthy (no subsystems wired)
    assert!(!status.healthy, "default governance must not be healthy");
    // These subsystems are enabled by default
    assert!(
        status.subsystems.rationalization,
        "rationalization must be enabled by default"
    );
    assert!(
        status.subsystems.security_governor,
        "security_governor must be enabled by default"
    );
    assert!(status.subsystems.rbac, "rbac must be enabled by default");
}
