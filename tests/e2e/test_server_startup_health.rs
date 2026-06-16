//! Server Startup Health End-to-End
//!
//! Validates that server startup configuration and core subsystems
//! initialize correctly. Uses structural assertions against config
//! defaults and module initialization functions.

use go_on::config::RuntimeConfig;
use go_on::governance::status::GovernanceStatus;
use go_on::observability::{init_independent_stack, ObservabilityConfig};

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

/// Verify that the ObservabilityStack singleton can be initialised
/// independently and is idempotent.
#[test]
fn test_observability_stack_init_idempotent() {
    let config = ObservabilityConfig {
        service_name: "test".to_string(),
        otel_enabled: false,
        otlp_endpoint: None,
        sample_ratio: 1.0,
    };

    // First call should succeed
    let first = init_independent_stack(&config);
    assert!(first, "first init_independent_stack call must return true");

    // Second call (same config) should be idempotent — return false
    let second = init_independent_stack(&config);
    assert!(
        !second,
        "second init_independent_stack call must return false (idempotent)"
    );
}

/// Verify that the governance status module initialises correctly.
#[test]
fn test_governance_status_has_defaults() {
    let status = GovernanceStatus::default();
    // Default governance should not be healthy (no subsystems wired)
    assert!(!status.healthy, "default governance must not be healthy");
    // All subsystems should default to false
    assert!(
        !status.subsystems.rationalization,
        "rationalization must be disabled by default"
    );
    assert!(
        !status.subsystems.security_governor,
        "security_governor must be disabled by default"
    );
    assert!(!status.subsystems.rbac, "rbac must be disabled by default");
}
