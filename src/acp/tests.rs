//! ACP Tests - Test utilities and test cases
//!
//! This module contains test utilities and test cases for the ACP system.
//! It's only compiled when running tests.

#[cfg(test)]
mod test_suite {
    use std::collections::HashMap;

    use crate::acp::prelude::*;
    use crate::acp::server::ServerBuilder;
    use crate::config::{AgentConfig, AppConfig, FlowConfig, PhaseConfig};
    use crate::core::config::AdaptiveConfig;

    /// Create a vector config fixture for testing
    pub fn vector_config_fixture() -> crate::config::VectorConfig {
        crate::config::VectorConfig {
            enabled: true,
            auto_mode: false,
            path: "vector.sqlite3".to_string(),
            connection_string: None,
            dimensions: 192,
            min_query_chars: 140,
            top_k: 4,
            min_similarity: 0.91,
            max_snippet_chars: 640,
            max_entries: 1000,
            summary_enabled: false,
            summary_trigger_messages: 12,
            summary_max_chars: 2400,
            read_replica_connection_string: None,
        }
    }

    /// Create a phase inference server for testing
    pub fn phase_inference_server(
        default_phase: &str,
        phase_names: &[&str],
    ) -> crate::acp::server::AcpServer {
        let mut agents = HashMap::new();
        agents.insert(
            "copilot".to_string(),
            AgentConfig {
                agent_type: "copilot".to_string(),
                url: Some("http://127.0.0.1:8080".to_string()),
                chat_path: None,
                api_key_env: None,
                secret_key_env: None,
                anthropic_version: None,
                model: None,
                max_tokens: None,
                supports_system: None,
                supports_vision: None,
            },
        );

        let phases: HashMap<String, PhaseConfig> = phase_names
            .iter()
            .map(|name| {
                (
                    (*name).to_string(),
                    PhaseConfig {
                        description: format!("{} phase", name),
                        agents: vec!["copilot".to_string()],
                        fallback: None,
                        principles: Some(Vec::new()),
                        options: None,
                    },
                )
            })
            .collect();

        let flow = FlowConfig {
            name: default_phase.to_string(),
            phases: phases.keys().cloned().collect(),
            workflow_type: crate::config::WorkflowType::Auto,
        };

        let _config = AppConfig {
            schema_version: "1.0.0".to_string(),
            provider: crate::core::config::types::ProviderConfig {
                default_phase: default_phase.to_string(),
                agents,
                role_registry: HashMap::new(),
            },
            flow,
            phases,
            runtime: None,
            cache: None,
            vector: Some(vector_config_fixture()),
            autotune: None,
            security: crate::core::config::types::SecurityConfig::default(),
            feature: crate::core::config::types::FeatureConfig {
                model_selection_mode: "auto".to_string(),
                ..Default::default()
            },
            compliance: None,
            startup_context: None,
            protocol: None,
        };

        // Create server using builder
        let builder = ServerBuilder::new();

        // Note: In a real test, we would set up actual components
        // For now, we'll create a minimal server
        builder.build()
    }

    /// Test conversation state is accessible on startup
    #[tokio::test]
    async fn test_conversation_state_initial() {
        let server = phase_inference_server("coding", &["coding", "review"]);

        // Verify conversation state is initialized and empty
        let state = server.session.conversation_state.lock().await;
        assert!(state.checkpoints.is_empty());
        assert!(state.branch_heads.is_empty());
    }

    /// Test that conversation checkpoint creation via runtime_pack works (integration smoke test)
    #[tokio::test]
    async fn test_conversation_checkpoint_creation_via_runtime_pack() {
        use crate::acp::r#impl::request::create_checkpoint_record;
        use crate::agent::Message;

        let server = phase_inference_server("coding", &["coding", "review"]);
        let message = Message {
            role: "user".to_string(),
            content: "Test message".to_string(),
        };
        let checkpoint = create_checkpoint_record(
            &server,
            "conv-test",
            "main",
            vec![message],
            Some("checkpoint note".to_string()),
            None,
        )
        .await;

        assert_eq!(checkpoint.conversation_id, "conv-test");
        assert_eq!(checkpoint.branch_id, "main");
        assert_eq!(checkpoint.messages.len(), 1);
        assert!(!checkpoint.checkpoint_id.is_empty());

        let state = server.session.conversation_state.lock().await;
        assert_eq!(state.checkpoints.len(), 1);
        assert_eq!(state.checkpoints[0].checkpoint_id, checkpoint.checkpoint_id);
    }

    /// Test server status reporting
    #[tokio::test]
    async fn test_server_status() {
        let server = phase_inference_server("coding", &["coding", "review"]);

        let status = server.get_status();

        // Verify status structure. NOTE: `total_requests` inside `get_status()`
        // is merged with the process-wide performance monitor
        // (`global_metrics_snapshot()`), which parallel tests may already have
        // initialized — so assert on the per-server metric instead of the
        // merged value.
        assert_eq!(server.metrics().total_requests(), 0);
        assert!(status.metrics.avg_request_duration_ms >= 0.0);
        assert_eq!(status.lifecycle.current_phase, "running");
        assert!(status.lifecycle.is_healthy);
    }

    /// Test maintenance cycle
    #[tokio::test]
    async fn test_maintenance_cycle() {
        let server = phase_inference_server("coding", &["coding", "review"]);

        // Test that maintenance tracker is accessible and starts in the
        // idle state (the legacy `last_maintenance` field was removed — it
        // was never updated after construction).
        let maintenance_snapshot = server
            .maintenance()
            .read()
            .map(|guard| guard.snapshot())
            .unwrap_or_default();
        assert!(!maintenance_snapshot.running);
        assert_eq!(maintenance_snapshot.cycles_total, 0);
        assert!(maintenance_snapshot.last_started_at.is_none());
        assert!(maintenance_snapshot.last_completed_at.is_none());
    }

    /// Test circuit breaker functionality
    #[tokio::test]
    async fn test_circuit_breakers() {
        let server = phase_inference_server("coding", &["coding", "review"]);

        // Test circuit breaker registry
        let open_count = server
            .circuit_breakers()
            .lock()
            .map(|guard| guard.open_count())
            .unwrap_or(0);
        assert_eq!(open_count, 0);
    }

    /// Test metrics collection
    #[tokio::test]
    async fn test_metrics() {
        let server = phase_inference_server("coding", &["coding", "review"]);

        // Test basic metrics
        // Note: metrics() returns &Arc<RuntimeMetrics>
        let metrics = server.metrics();
        assert_eq!(metrics.successful_requests(), 0);
        assert_eq!(metrics.failed_requests(), 0);
        assert_eq!(metrics.active_requests(), 0);
    }

    /// Test lifecycle management
    #[tokio::test]
    async fn test_lifecycle() {
        let server = phase_inference_server("coding", &["coding", "review"]);

        // Test initial state
        assert!(server.is_healthy());
        assert!(!server.shutdown_requested());

        // Test shutdown request
        server.begin_shutdown();
        assert!(server.shutdown_requested());
    }

    /// Test server builder
    #[tokio::test]
    async fn test_server_builder() {
        // Adaptive auto-detection yields a minimal config with provider
        // auto-detection (the former `to_app_config` conversion was removed
        // with the un-wired adaptive-learning chain).
        let adaptive_config = AdaptiveConfig::auto_detect();
        assert!(adaptive_config.adaptive_mode);
        assert!(!adaptive_config
            .minimal_config
            .available_providers
            .is_empty());
        assert_eq!(adaptive_config.minimal_config.default_phase, "coding");

        let builder = ServerBuilder::new();
        let server = builder.build();

        // Note: The config field doesn't exist on the new AcpServer structure
        // This test is simplified for migration
        assert!(server.model_deps.flow_manager.is_none());
    }

    /// Test checkpoint capacity enforcement
    #[test]
    fn test_enforce_checkpoint_capacity() {
        let mut state = ConversationState::default();

        // Add some checkpoints
        for i in 0..300 {
            state.checkpoints.push(ConversationCheckpoint {
                checkpoint_id: format!("cp-{}", i),
                conversation_id: "test".to_string(),
                branch_id: "main".to_string(),
                parent_checkpoint_id: None,
                created_at: i as i64,
                note: None,
                metacognitive_loop: None,
                messages: Vec::new(),
            });
        }

        // Should be over capacity
        assert!(state.checkpoints.len() > MAX_CHECKPOINTS_PER_CONVERSATION);

        // Enforce capacity
        enforce_checkpoint_capacity(&mut state, 0, None);

        // Should be at or below capacity
        assert!(state.checkpoints.len() <= MAX_CHECKPOINTS_PER_CONVERSATION);
    }

    /// Test timestamp utilities
    #[test]
    fn test_timestamp_utilities() {
        let ts = now_ts();
        let ts_ms = crate::shared::timestamps::now_ts_ms();

        assert!(ts > 0);
        assert!(ts_ms > 0);
        assert!(ts_ms >= ts * 1000);
    }
}
