//! Server Startup Health End-to-End
//!
//! Validates that server startup configuration and core subsystems
//! initialize correctly. Uses structural assertions against config
//! defaults and module initialization functions.

#[cfg(test)]
mod test_server_startup_health {
    /// Verify that server startup config defaults do not panic and have
    /// expected initial values.
    #[test]
    fn test_default_config_is_well_formed() {
        let config = go_on::config::RuntimeConfig::default();
        // Default should always have a name and be non-empty
        assert!(!config.service_name.is_empty(), "service_name must be set");
        // Default governance should be disabled for local operation
        assert!(
            !config.governance_enabled,
            "governance must be disabled by default"
        );
        // Default health port should be reasonable
        assert!(
            config.health_port == 0 || (1024..=65535).contains(&config.health_port),
            "health_port must be 0 (disabled) or in ephemeral/dynamic range"
        );
    }

    /// Verify that the ObservabilityStack singleton can be initialised
    /// independently and is idempotent.
    #[test]
    fn test_observability_stack_init_idempotent() {
        let config = go_on::observability::ObservabilityConfig {
            service_name: "test".to_string(),
            otel_enabled: false,
            otlp_endpoint: None,
            sample_ratio: 1.0,
        };

        // First call should succeed
        let first = go_on::observability::init_independent_stack(&config);
        assert!(first, "first init_independent_stack call must return true");

        // Second call (same config) should be idempotent — return false
        let second = go_on::observability::init_independent_stack(&config);
        assert!(
            !second,
            "second init_independent_stack call must return false (idempotent)"
        );
    }

    /// Verify that the governance status module initialises correctly.
    #[test]
    fn test_governance_status_has_defaults() {
        let status = go_on::governance::status::GovernanceStatus::default();
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
}
