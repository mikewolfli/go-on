//! Chaos Drill — Integration tests for fault injection and recovery validation.
//!
//! These tests validate that the RecoveryAction chain operates correctly
//! under various simulated failure conditions. They use the ChaosEngine
//! to inject faults and verify expected recovery behaviour.

#[cfg(test)]
mod chaos_drill_tests {
    use go_on::resilience::chaos::*;

    #[test]
    fn drill_network_resilience() {
        let engine = ChaosEngine::new();
        let scenario = network_resilience_scenario();

        let result = tokio::runtime::Runtime::new()
            .expect("create tokio runtime")
            .block_on(engine.run_drills(&scenario));

        assert!(
            result.passed,
            "Network resilience drill failed: {} / {} recoveries failed",
            result.failed_recoveries, result.total_injections
        );
        assert_eq!(result.total_injections, 3);
        assert_eq!(result.successful_recoveries, 3);
    }

    #[test]
    fn drill_storage_resilience() {
        let engine = ChaosEngine::new();
        let scenario = storage_resilience_scenario();

        let result = tokio::runtime::Runtime::new()
            .expect("create tokio runtime")
            .block_on(engine.run_drills(&scenario));

        assert!(result.passed, "Storage resilience drill failed");
        assert_eq!(result.total_injections, 3);
        assert_eq!(result.successful_recoveries, 3);
    }

    #[test]
    fn drill_resource_exhaustion() {
        let engine = ChaosEngine::new();
        let scenario = resource_exhaustion_scenario();

        let result = tokio::runtime::Runtime::new()
            .expect("create tokio runtime")
            .block_on(engine.run_drills(&scenario));

        assert!(result.passed, "Resource exhaustion drill failed");
        assert_eq!(result.total_injections, 2);
    }

    #[test]
    fn drill_no_fault_when_disabled() {
        let engine = ChaosEngine::new();
        assert!(
            engine.check_fault("any_tool").is_none(),
            "Should not inject faults when disabled"
        );
    }

    #[test]
    fn drill_clear_injections() {
        let engine = ChaosEngine::new();
        engine.set_enabled(true);

        let scenario = network_resilience_scenario();
        engine.load_scenario(&scenario);
        assert!(engine.check_fault("read_file").is_some());

        engine.clear();
        assert!(
            engine.check_fault("read_file").is_none(),
            "Should not inject after clear"
        );
    }

    #[test]
    fn drill_custom_scenario() {
        let engine = ChaosEngine::new();

        let scenario = DrillScenario {
            name: "custom_auth_test".to_string(),
            description: "Test auth failure recovery".to_string(),
            injections: vec![FaultInjection {
                fault_type: FaultType::AuthFailure,
                target_tool: "http_request".to_string(),
                probability: 1.0,
                max_injections: 3,
            }],
            expected_recoveries: vec!["reroute".to_string()],
            timeout_secs: 10,
        };

        let result = tokio::runtime::Runtime::new()
            .expect("create tokio runtime")
            .block_on(engine.run_drills(&scenario));

        assert!(result.passed);
        assert_eq!(
            result.injection_results[0].recovery_action,
            Some("reroute".to_string())
        );
    }
}
