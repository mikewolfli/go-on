//! Unit tests for `chat.rs` — extracted to separate file for code organization.
//!
//! These tests validate the core `process_chat_request` pipeline and its
//! helper functions under various scenarios:
//!
//! - Vector context and checkpoint tree wiring
//! - HarnessBus / CapabilityBus closed-loop integration
//! - Empty agent output fallback and error handling
//! - Model-specific agent filtering fallback
//! - High-risk council deliberation and provenance
//! - Autonomy loop contract reporting
//! - Token economy estimation
//! - Phase summary generation
//! - Tool call extraction from various response formats

#[cfg(test)]
mod unit_tests {
    #[cfg(not(feature = "backend-postgres"))]
    use std::collections::HashMap;
    #[cfg(not(feature = "backend-postgres"))]
    use std::sync::{Arc, Mutex};

    #[cfg(not(feature = "backend-postgres"))]
    use async_trait::async_trait;
    #[cfg(not(feature = "backend-postgres"))]
    use serde_json::json;
    #[cfg(not(feature = "backend-postgres"))]
    use serde_json::Value;

    #[cfg(not(feature = "backend-postgres"))]
    use crate::acp::helpers::agent_preference::reset_agent_switch_state_for_test;
    #[cfg(not(feature = "backend-postgres"))]
    use crate::acp::r#impl::chat::{
        extract_tool_calls_from_response, process_chat_request, ChatParams,
    };
    #[cfg(not(feature = "backend-postgres"))]
    use crate::memory::agent_memory_bus::clear_agent_memory_bus;

    /// Global mutex to serialize chat tests that share global state.
    /// Prevents flaky failures when tests run in parallel and interfere
    /// with shared globals like AGENT_SWITCH_STATE and AGENT_MEMORY_BUS.
    #[cfg(not(feature = "backend-postgres"))]
    static CHAT_TEST_SERIAL: std::sync::LazyLock<std::sync::Mutex<()>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

    /// Reset all global state that can accumulate across test runs.
    /// Acquires the serialization lock to prevent parallel interference.
    #[cfg(not(feature = "backend-postgres"))]
    fn reset_global_state() {
        let _guard = CHAT_TEST_SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        reset_agent_switch_state_for_test();
        clear_agent_memory_bus();
    }
    #[cfg(not(feature = "backend-postgres"))]
    use crate::acp::server::ServerBuilder;
    #[cfg(not(feature = "backend-postgres"))]
    use crate::agent::AgentRegistry;
    #[cfg(not(feature = "backend-postgres"))]
    use crate::agent::Message;
    #[cfg(not(feature = "backend-postgres"))]
    use crate::agent::{Agent, StreamingSender};
    #[cfg(not(feature = "backend-postgres"))]
    use crate::config::{AppConfig, FlowConfig, PhaseConfig, PhaseOptions, VectorConfig};
    #[cfg(not(feature = "backend-postgres"))]
    use crate::flow::FlowManager;
    #[cfg(not(feature = "backend-postgres"))]
    use crate::rpc_protocol::chat_trace_context;
    #[cfg(not(feature = "backend-postgres"))]
    use crate::vector::VectorStore;

    #[cfg(not(feature = "backend-postgres"))]
    use crate::governance::hardening::TenantResourceQuota;

    #[cfg(not(feature = "backend-postgres"))]
    use crate::acp::r#impl::chat::build_phase_summary;

    /// Configure a default-tenant quota so agent budget checks pass in tests.
    #[cfg(not(feature = "backend-postgres"))]
    fn setup_test_tenant_budget(server: &crate::acp::server::AcpServer) {
        server
            .rate_limiting
            .tenant_budget
            .lock()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("tenant_budget lock poisoned, recovering");
                poisoned.into_inner()
            })
            .set_quota(TenantResourceQuota {
                tenant_id: "default-tenant".to_string(),
                daily_token_limit: 1_000_000,
                concurrent_tasks_limit: 100,
                daily_api_call_limit: 10_000,
            });
    }

    #[cfg(not(feature = "backend-postgres"))]
    struct RecordingAgent {
        seen_messages: Arc<Mutex<Vec<Message>>>,
        output: String,
    }

    #[cfg(not(feature = "backend-postgres"))]
    #[async_trait]
    impl Agent for RecordingAgent {
        async fn chat(
            &self,
            messages: Vec<Message>,
            _principles: Option<Vec<String>>,
            _options: Option<HashMap<String, Value>>,
            sender: StreamingSender,
        ) -> crate::core::error::Result<()> {
            if let Ok(mut guard) = self.seen_messages.lock() {
                *guard = messages;
            } else {
                tracing::error!("seen_messages lock poisoned, recovering");
                *self.seen_messages.lock().unwrap_or_else(|e| e.into_inner()) = messages;
            }
            sender
                .send(self.output.clone())
                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
            Ok(())
        }
    }

    #[cfg(not(feature = "backend-postgres"))]
    fn test_config() -> AppConfig {
        let mut phases = HashMap::new();
        phases.insert(
            "coding".to_string(),
            PhaseConfig {
                description: "coding".to_string(),
                agents: vec!["test-agent".to_string()],
                fallback: Some(true),
                principles: None,
                options: Some(PhaseOptions {
                    vector_enabled: Some(true),
                    vector_min_query_chars: Some(4),
                    vector_top_k: Some(2),
                    vector_min_similarity: Some(0.0),
                    vector_max_snippet_chars: Some(120),
                    summary_enabled: Some(true),
                    summary_trigger_messages: Some(1),
                    summary_max_chars: Some(240),
                    extra: std::iter::once(("llm_summary_enabled".to_string(), json!(false)))
                        .collect(),
                    ..PhaseOptions::default()
                }),
            },
        );

        AppConfig {
            schema_version: "1.0.0".to_string(),
            provider: crate::core::config::types::ProviderConfig {
                default_phase: "coding".to_string(),
                agents: HashMap::new(),
                role_registry: HashMap::new(),
            },
            flow: FlowConfig {
                name: "flow".to_string(),
                phases: vec!["coding".to_string()],
                workflow_type: crate::config::WorkflowType::Auto,
            },
            phases,
            runtime: None,
            cache: None,
            vector: Some(VectorConfig {
                enabled: true,
                auto_mode: false,
                path: "vector.sqlite3".to_string(),
                connection_string: None,
                dimensions: 32,
                min_query_chars: 4,
                top_k: 2,
                min_similarity: 0.0,
                max_snippet_chars: 120,
                max_entries: 128,
                summary_enabled: true,
                summary_trigger_messages: 1,
                summary_max_chars: 240,
            }),
            autotune: None,
            security: crate::core::config::types::SecurityConfig::default(),
            feature: crate::core::config::types::FeatureConfig {
                model_selection_mode: "adaptive".to_string(),
                ..Default::default()
            },
            compliance: None,
            startup_context: None,
            scheduler: None,
            reputation: None,
        }
    }

    #[cfg(not(feature = "backend-postgres"))]
    #[tokio::test]
    async fn process_chat_request_wires_vector_context_and_checkpoint_tree() {
        reset_global_state();
        let temp = tempfile::tempdir().expect("tempdir should exist");
        let vector_path = temp.path().join("vector.sqlite3");
        let vector_store = Arc::new(
            VectorStore::new(&vector_path, 32, 128).expect("vector store should initialize"),
        );
        vector_store
            .upsert(
                "coding",
                "rust stream notifications",
                "Use structured stream notifications for chunked output.",
            )
            .expect("seed vector entry");
        vector_store
            .upsert_phase_summary("coding", "Existing coding summary")
            .expect("seed phase summary");

        let seen_messages = Arc::new(Mutex::new(Vec::new()));
        let mut registry = AgentRegistry::new();
        registry.register_arc(
            "test-agent",
            Arc::new(RecordingAgent {
                seen_messages: Arc::clone(&seen_messages),
                output: "streamed answer".to_string(),
            }),
        );

        let config = Arc::new(test_config());
        let flow = Arc::new(FlowManager::new(Arc::clone(&config), None));

        let mut server = ServerBuilder::new().build();
        setup_test_tenant_budget(&server);
        server.model_deps.flow_manager = Some(flow);
        server.model_deps.agent_registry = Some(Arc::new(registry));
        server.cache_deps.cache.vector_store = Some(Arc::clone(&vector_store));
        server.cache_deps.vector_config = config.vector.clone();
        let config_path = temp.path().join("config.toml");
        std::fs::write(&config_path, "default_phase = \"coding\"\n").expect("config write");
        server.config_path = Some(config_path.display().to_string());
        if let Ok(mut ledger) = server.persistence.artifact_ledger.lock() {
            *ledger = crate::reinforcement::ArtifactLedger::new(Some(&config_path));
        }

        let params = ChatParams {
            mode: "ask".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: "Tell me about rust stream notifications".to_string(),
            }],
            conversation_id: Some("conv-chat".to_string()),
            branch_id: Some("feature-a".to_string()),
            phase: Some("coding".to_string()),
            options: None,
            requirement_contract: None,
            plan: None,
            vector_hits: None,
            execution_decision_candidate: None,
        };

        let trace = chat_trace_context(&Some(json!(1)), "chat.test");
        let result = process_chat_request(&server, &params, None, &trace, None, None)
            .await
            .expect("chat request should succeed");

        assert_eq!(result["branch_id"], "feature-a");
        assert_eq!(result["response"], "streamed answer");
        assert_eq!(
            result["vector_hits"].as_array().map(|items| items.len()),
            Some(1)
        );
        assert_eq!(result["checkpoint"]["branch_id"], "feature-a");
        assert!(
            result["metacognitive_loop"]["cycle_count"]
                .as_u64()
                .unwrap_or(0)
                >= 1
        );
        assert_eq!(
            result["checkpoint"]["metacognitive_loop"]["checkpoint_id"],
            result["checkpoint"]["checkpoint_id"]
        );
        assert!(result["token_economy"]["compression_ratio"].is_number());
        assert_eq!(result["knowledge"]["vector_memory_written"], true);
        assert!(result["knowledge"]["artifact_path"].is_string());
        assert_eq!(
            result["distillation"]["shared_epistemic_base_updated"],
            true
        );

        let captured = seen_messages.lock().expect("messages lock").clone();
        assert!(
            captured.iter().any(|msg| msg.role == "system"),
            "expected at least one system message"
        );
        let combined_system = captured
            .iter()
            .filter(|msg| msg.role == "system")
            .map(|msg| msg.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined_system.contains("Existing coding summary"));
        assert!(combined_system.contains("stream notifications"));

        let state = server.session.conversation_state.lock().await;
        assert_eq!(state.checkpoints.len(), 1);
        assert!(state.branch_heads.contains_key("conv-chat:feature-a"));

        assert_eq!(vector_store.memory_entry_count().expect("count"), 3);
        assert!(vector_store
            .get_phase_summary("coding")
            .expect("summary read")
            .expect("summary should exist")
            .contains("Intent:"));

        let artifact_path = result["knowledge"]["artifact_path"]
            .as_str()
            .expect("artifact path should be present");
        assert!(std::path::Path::new(artifact_path).exists());

        let distillation_path = result["distillation"]["artifact_path"]
            .as_str()
            .expect("distillation artifact path should be present");
        assert!(std::path::Path::new(distillation_path).exists());
    }

    #[cfg(not(feature = "backend-postgres"))]
    #[test]
    fn estimate_token_economy_reports_compression_ratio() {
        let payload = crate::acp::r#impl::chat::estimate_token_economy(
            &[Message {
                role: "user".to_string(),
                content: "Summarize this large body of implementation detail into one paragraph."
                    .to_string(),
            }],
            "Short summary.",
        );

        assert!(payload["input_tokens"].as_u64().unwrap_or(0) > 0);
        assert!(payload["output_tokens"].as_u64().unwrap_or(0) > 0);
        assert!(payload["compression_ratio"].as_f64().unwrap_or(2.0) <= 1.0);
    }

    #[cfg(not(feature = "backend-postgres"))]
    #[test]
    fn build_phase_summary_trims_to_requested_size() {
        let summary = build_phase_summary(
            &[Message {
                role: "user".to_string(),
                content: "0123456789abcdef".to_string(),
            }],
            "response",
            12,
        );

        assert!(summary.chars().count() <= 12);
        assert!(!summary.is_empty());
    }

    #[cfg(not(feature = "backend-postgres"))]
    #[test]
    fn extract_tool_calls_from_explicit_marker() {
        let response = "Here is the plan\n__tool_call__:read_file:{\"path\":\"src/main.rs\"}\n__tool_call__:apply_patch:{\"path\":\"src/lib.rs\"}";
        let calls = extract_tool_calls_from_response(response, 5);
        assert_eq!(
            calls,
            vec!["read_file".to_string(), "apply_patch".to_string()]
        );
    }

    #[cfg(not(feature = "backend-postgres"))]
    #[test]
    fn extract_tool_calls_from_json_fence() {
        let response = "```json\n{\"tool_calls\":[{\"name\":\"read_file\"},{\"tool\":\"apply_patch\"}],\"tool_call\":\"bash\"}\n```";
        let calls = extract_tool_calls_from_response(response, 5);
        assert_eq!(
            calls,
            vec![
                "bash".to_string(),
                "read_file".to_string(),
                "apply_patch".to_string()
            ]
        );
    }

    #[cfg(not(feature = "backend-postgres"))]
    #[test]
    fn extract_tool_calls_from_action_plan_alias() {
        let response = "```json\n{\"action_plan\":{\"actions\":[{\"action\":\"read_file\"},{\"tool\":\"apply_patch\"},{\"name\":\"bash\"}]}}\n```";
        let calls = extract_tool_calls_from_response(response, 5);
        assert_eq!(
            calls,
            vec![
                "read_file".to_string(),
                "apply_patch".to_string(),
                "bash".to_string()
            ]
        );
    }

    #[cfg(not(feature = "backend-postgres"))]
    #[tokio::test]
    async fn process_chat_request_wires_harness_and_capability_bus_closed_loop() {
        reset_global_state();
        let temp = tempfile::tempdir().expect("tempdir should exist");
        let vector_path = temp.path().join("e2e_vector.sqlite3");
        let vector_store = Arc::new(
            VectorStore::new(&vector_path, 32, 128).expect("vector store should initialize"),
        );
        vector_store
            .upsert("coding", "rust e2e test", "E2E dual bus integration test")
            .expect("seed vector entry");

        let seen_messages = Arc::new(Mutex::new(Vec::new()));
        let mut registry = AgentRegistry::new();
        registry.register_arc(
            "test-agent",
            Arc::new(RecordingAgent {
                seen_messages: Arc::clone(&seen_messages),
                output: "e2e dual bus answer".to_string(),
            }),
        );

        let mut config = test_config();
        config.reputation = Some(crate::config::ReputationConfig {
            enabled: true,
            ema_alpha: 0.3,
            exclusion_threshold: 0.1,
            degraded_threshold: 0.3,
        });
        let config = Arc::new(config);
        let flow = Arc::new(FlowManager::new(Arc::clone(&config), None));

        let harness_bus = Arc::new(crate::governance::harness_bus::default_harness_bus(None));
        let workflow_registry = Arc::new(std::sync::Mutex::new(
            crate::orchestration::workflow_registry::WorkflowRegistry::new(),
        ));
        let capability_bus = Arc::new(
            crate::intelligence::capability_bus::core::CapabilityBus::new_default(
                Arc::clone(&harness_bus),
                Some(Arc::clone(&workflow_registry)),
            ),
        );

        let mut server = ServerBuilder::new().build();
        setup_test_tenant_budget(&server);
        server.model_deps.flow_manager = Some(flow);
        server.model_deps.agent_registry = Some(Arc::new(registry));
        server.cache_deps.cache.vector_store = Some(Arc::clone(&vector_store));
        server.cache_deps.vector_config = config.vector.clone();
        server.governance_deps.harness_bus = Some(Arc::clone(&harness_bus));
        server.governance_deps.capability_bus = Some(Arc::clone(&capability_bus));
        let config_path = temp.path().join("e2e_config.toml");
        std::fs::write(&config_path, "default_phase = \"coding\"\n").expect("config write");
        server.config_path = Some(config_path.display().to_string());

        let params = ChatParams {
            mode: "ask".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: "Test dual bus closed loop integration".to_string(),
            }],
            conversation_id: Some("e2e-conv".to_string()),
            branch_id: Some("e2e-branch".to_string()),
            phase: Some("coding".to_string()),
            options: None,
            requirement_contract: None,
            plan: None,
            vector_hits: None,
            execution_decision_candidate: None,
        };

        let trace = chat_trace_context(&Some(json!(1)), "chat.e2e");
        let result = process_chat_request(&server, &params, None, &trace, None, None)
            .await
            .expect("e2e dual bus chat request should succeed");

        let hp = harness_bus.governance_profile();
        assert!(
            hp.total_evaluations >= 1,
            "HarnessBus evaluate() must be called"
        );
        assert!(
            hp.allow_count + hp.deny_count + hp.escalate_count + hp.review_count >= 1,
            "HarnessBus must produce at least one verdict"
        );

        let cp = capability_bus.capability_bus_profile();
        assert!(
            cp.routing_count >= 1,
            "CapabilityBus must route at least once"
        );

        assert_eq!(result["branch_id"], "e2e-branch");
        assert_eq!(result["response"], "e2e dual bus answer");

        let captured = seen_messages.lock().expect("messages lock").clone();
        assert!(
            captured.iter().any(|msg| msg.role == "system"),
            "expected at least one system message"
        );
        let combined_system = captured
            .iter()
            .filter(|msg| msg.role == "system")
            .map(|msg| msg.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            combined_system.contains("E2E dual bus integration test"),
            "vector context must be injected into system message"
        );
    }

    #[cfg(not(feature = "backend-postgres"))]
    #[tokio::test]
    async fn process_chat_request_skips_empty_agent_output_and_uses_next_agent() {
        reset_global_state();
        let temp = tempfile::tempdir().expect("tempdir should exist");

        let seen_messages = Arc::new(Mutex::new(Vec::new()));
        let mut registry = AgentRegistry::new();
        registry.register_arc(
            "empty-agent",
            Arc::new(RecordingAgent {
                seen_messages: Arc::new(Mutex::new(Vec::new())),
                output: "   ".to_string(),
            }),
        );
        registry.register_arc(
            "test-agent",
            Arc::new(RecordingAgent {
                seen_messages: Arc::clone(&seen_messages),
                output: "fallback answer".to_string(),
            }),
        );

        let mut config = test_config();
        config
            .phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .agents = vec!["empty-agent".to_string(), "test-agent".to_string()];
        let config = Arc::new(config);
        let flow = Arc::new(FlowManager::new(Arc::clone(&config), None));

        let mut server = ServerBuilder::new().build();
        setup_test_tenant_budget(&server);
        server.model_deps.flow_manager = Some(flow);
        server.model_deps.agent_registry = Some(Arc::new(registry));
        server.cache_deps.vector_config = config.vector.clone();
        let config_path = temp.path().join("config.toml");
        std::fs::write(&config_path, "default_phase = \"coding\"\n").expect("config write");
        server.config_path = Some(config_path.display().to_string());

        let params = ChatParams {
            mode: "ask".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: "Return a concise answer".to_string(),
            }],
            conversation_id: Some("empty-output-conv".to_string()),
            branch_id: Some("main".to_string()),
            phase: Some("coding".to_string()),
            options: None,
            requirement_contract: None,
            plan: None,
            vector_hits: None,
            execution_decision_candidate: None,
        };

        let trace = chat_trace_context(&Some(json!(1)), "chat.empty_output");
        let result = process_chat_request(&server, &params, None, &trace, None, None)
            .await
            .expect("chat request should succeed by trying next agent");

        assert_eq!(result["response"], "fallback answer");

        // The system should have produced a result where the fallback
        // kicked in due to empty output from the first agent.
        let attempts = result["agent_attempts"]
            .as_array()
            .expect("agent attempts should be an array");
        assert!(!attempts.is_empty(), "expected at least one agent attempt");
        // The final selected agent should be the fallback (test-agent)
        let _selected = result["routing_diagnostics"]
            .get("selected_agent")
            .or_else(|| result.get("selected_agent"));
        assert!(
            attempts
                .iter()
                .any(|attempt| attempt["agent"] == "test-agent"
                    && attempt.get("ok").and_then(|v| v.as_bool()) == Some(true)),
            "test-agent should be among successful attempts, attempts: {:?}",
            attempts
        );

        let captured = seen_messages.lock().expect("messages lock").clone();
        assert_eq!(
            captured.last().map(|msg| msg.role.as_str()),
            Some("user"),
            "second agent should receive the request"
        );
    }

    #[cfg(not(feature = "backend-postgres"))]
    #[tokio::test]
    async fn process_chat_request_all_empty_outputs_returns_specific_error() {
        reset_global_state();
        let temp = tempfile::tempdir().expect("tempdir should exist");

        let mut registry = AgentRegistry::new();
        registry.register_arc(
            "empty-agent",
            Arc::new(RecordingAgent {
                seen_messages: Arc::new(Mutex::new(Vec::new())),
                output: " ".to_string(),
            }),
        );

        let mut config = test_config();
        config
            .phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .agents = vec!["empty-agent".to_string()];
        let config = Arc::new(config);
        let flow = Arc::new(FlowManager::new(Arc::clone(&config), None));

        let mut server = ServerBuilder::new().build();
        setup_test_tenant_budget(&server);
        server.model_deps.flow_manager = Some(flow);
        server.model_deps.agent_registry = Some(Arc::new(registry));
        server.cache_deps.vector_config = config.vector.clone();
        let config_path = temp.path().join("config.toml");
        std::fs::write(&config_path, "default_phase = \"coding\"\n").expect("config write");
        server.config_path = Some(config_path.display().to_string());

        let params = ChatParams {
            mode: "ask".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: "Return a concise answer".to_string(),
            }],
            conversation_id: Some("all-empty-conv".to_string()),
            branch_id: Some("main".to_string()),
            phase: Some("coding".to_string()),
            options: None,
            requirement_contract: None,
            plan: None,
            vector_hits: None,
            execution_decision_candidate: None,
        };

        let trace = chat_trace_context(&Some(json!(1)), "chat.all_empty");
        let err = process_chat_request(&server, &params, None, &trace, None, None)
            .await
            .expect_err("all empty outputs should fail with a specific error");

        let err_msg = err.to_string();
        assert!(
            err_msg.contains("all candidate agents returned empty responses")
                || err_msg.starts_with("error.chat."),
            "error should explain empty responses, got: {}",
            err_msg
        );
    }

    #[cfg(not(feature = "backend-postgres"))]
    #[tokio::test]
    async fn process_chat_request_specific_model_without_match_keeps_phase_agents() {
        reset_global_state();
        let temp = tempfile::tempdir().expect("tempdir should exist");

        let seen_messages = Arc::new(Mutex::new(Vec::new()));
        let mut registry = AgentRegistry::new();
        registry.register_arc(
            "test-agent",
            Arc::new(RecordingAgent {
                seen_messages: Arc::clone(&seen_messages),
                output: "model fallback answer".to_string(),
            }),
        );

        let config = Arc::new(test_config());
        let flow = Arc::new(FlowManager::new(Arc::clone(&config), None));

        let mut server = ServerBuilder::new().build();
        setup_test_tenant_budget(&server);
        server.model_deps.flow_manager = Some(flow);
        server.model_deps.agent_registry = Some(Arc::new(registry));
        server.cache_deps.vector_config = config.vector.clone();
        let config_path = temp.path().join("config.toml");
        std::fs::write(&config_path, "default_phase = \"coding\"\n").expect("config write");
        server.config_path = Some(config_path.display().to_string());

        let params = ChatParams {
            mode: "ask".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: "Return a concise answer".to_string(),
            }],
            conversation_id: Some("model-fallback-conv".to_string()),
            branch_id: Some("main".to_string()),
            phase: Some("coding".to_string()),
            options: Some(PhaseOptions {
                extra: [(
                    "model".to_string(),
                    json!("gpt-4o-mini-not-mapped-to-agent-name"),
                )]
                .into_iter()
                .collect(),
                ..PhaseOptions::default()
            }),
            requirement_contract: None,
            plan: None,
            vector_hits: None,
            execution_decision_candidate: None,
        };

        let trace = chat_trace_context(&Some(json!(1)), "chat.model_filter_fallback");
        let result = process_chat_request(&server, &params, None, &trace, None, None)
            .await
            .expect("chat request should succeed by falling back to phase agents");

        assert_eq!(result["response"], "model fallback answer");
        let attempts = result["agent_attempts"]
            .as_array()
            .expect("agent attempts should be an array");
        assert!(attempts
            .iter()
            .any(|attempt| attempt["agent"] == "test-agent" && attempt["ok"] == true));
    }

    #[cfg(not(feature = "backend-postgres"))]
    #[tokio::test]
    async fn process_chat_request_high_risk_multi_candidate_emits_council_decision() {
        reset_global_state();
        let temp = tempfile::tempdir().expect("tempdir should exist");

        let mut registry = AgentRegistry::new();
        registry.register_arc(
            "agent-a",
            Arc::new(RecordingAgent {
                seen_messages: Arc::new(Mutex::new(Vec::new())),
                output: "agent-a answer".to_string(),
            }),
        );
        registry.register_arc(
            "agent-b",
            Arc::new(RecordingAgent {
                seen_messages: Arc::new(Mutex::new(Vec::new())),
                output: "agent-b answer".to_string(),
            }),
        );

        let mut config = test_config();
        config
            .phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .agents = vec!["agent-a".to_string(), "agent-b".to_string()];
        let config = Arc::new(config);
        let flow = Arc::new(FlowManager::new(Arc::clone(&config), None));

        let harness_bus = Arc::new(crate::governance::harness_bus::default_harness_bus(None));
        harness_bus.set_sandbox_level(crate::governance::hardening::SandboxLevel::Strict);
        let workflow_registry = Arc::new(std::sync::Mutex::new(
            crate::orchestration::workflow_registry::WorkflowRegistry::new(),
        ));
        let capability_bus = Arc::new(
            crate::intelligence::capability_bus::core::CapabilityBus::new_default(
                Arc::clone(&harness_bus),
                Some(Arc::clone(&workflow_registry)),
            ),
        );

        let mut server = ServerBuilder::new().build();
        setup_test_tenant_budget(&server);
        server.model_deps.flow_manager = Some(flow);
        server.model_deps.agent_registry = Some(Arc::new(registry));
        server.cache_deps.vector_config = config.vector.clone();
        server.governance_deps.harness_bus = Some(Arc::clone(&harness_bus));
        server.governance_deps.capability_bus = Some(Arc::clone(&capability_bus));
        let config_path = temp.path().join("config.toml");
        std::fs::write(&config_path, "default_phase = \"coding\"\n").expect("config write");
        server.config_path = Some(config_path.display().to_string());

        let params = ChatParams {
            mode: "ask".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: "Need medical diagnosis and prescription decision with legal compliance considerations"
                    .to_string(),
            }],
            conversation_id: Some("council-smoke-conv".to_string()),
            branch_id: Some("main".to_string()),
            phase: Some("coding".to_string()),
            options: None,
            requirement_contract: None,
            plan: None,
            vector_hits: None,
            execution_decision_candidate: None,
        };

        let trace = chat_trace_context(&Some(json!(1)), "chat.council_smoke");
        let result = process_chat_request(&server, &params, None, &trace, None, None)
            .await
            .expect("chat request should succeed");

        let decision = result["routing_diagnostics"]["council_decision"]
            .as_object()
            .expect("council decision should be present for high-risk multi-candidate request");
        assert!(decision.contains_key("proposal_id"));

        let provenance = result["routing_diagnostics"]["routing_provenance"]
            .as_array()
            .expect("routing provenance must be an array")
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>();
        assert!(
            provenance.contains(&"council_deliberation_selected_route"),
            "routing provenance should indicate council deliberation route selection"
        );

        let response_text = result["response"].as_str().unwrap_or_default();
        assert!(response_text == "agent-a answer" || response_text == "agent-b answer");
    }

    #[cfg(not(feature = "backend-postgres"))]
    #[tokio::test]
    async fn process_chat_request_execute_mode_exposes_stable_autonomy_contract() {
        reset_global_state();
        let temp = tempfile::tempdir().expect("tempdir should exist");

        let mut registry = AgentRegistry::new();
        registry.register_arc(
            "test-agent",
            Arc::new(RecordingAgent {
                seen_messages: Arc::new(Mutex::new(Vec::new())),
                output: "execute mode answer".to_string(),
            }),
        );

        let config = Arc::new(test_config());
        let flow = Arc::new(FlowManager::new(Arc::clone(&config), None));

        let mut server = ServerBuilder::new().build();
        setup_test_tenant_budget(&server);
        server.model_deps.flow_manager = Some(flow);
        server.model_deps.agent_registry = Some(Arc::new(registry));
        server.cache_deps.vector_config = config.vector.clone();
        let config_path = temp.path().join("config.toml");
        std::fs::write(&config_path, "default_phase = \"coding\"\n").expect("config write");
        server.config_path = Some(config_path.display().to_string());

        let params = ChatParams {
            mode: "execute".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: "Fix the build and return the result".to_string(),
            }],
            conversation_id: Some("autonomy-contract-conv".to_string()),
            branch_id: Some("main".to_string()),
            phase: Some("coding".to_string()),
            options: None,
            requirement_contract: None,
            plan: None,
            vector_hits: None,
            execution_decision_candidate: None,
        };

        let trace = chat_trace_context(&Some(json!(1)), "chat.autonomy_contract");
        let result = process_chat_request(&server, &params, None, &trace, None, None)
            .await
            .expect("chat request should succeed");

        // In execute mode, the autonomy loop runs and produces a response directly.
        // The result uses the streamlined response format (no full assembly).
        // The response is produced by the autonomy loop (brain loop), not raw agent output.
        assert!(
            result["response"]
                .as_str()
                .map(|s| !s.is_empty())
                .unwrap_or(false),
            "response should be non-empty"
        );
        assert_eq!(result["agent"], "test-agent");
        assert_eq!(result["mode"], "execute");
        assert_eq!(result["done"], true);
        assert_eq!(result["phase"], "coding");
    }
}
