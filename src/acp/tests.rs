#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use super::*;
    use crate::config::{AgentConfig, AppConfig, FlowConfig, PhaseConfig};

    fn vector_config_fixture() -> VectorConfig {
        VectorConfig {
            enabled: true,
            auto_mode: false,
            path: "vector.sqlite3".to_string(),
            dimensions: 192,
            min_query_chars: 140,
            top_k: 4,
            min_similarity: 0.91,
            max_snippet_chars: 640,
            max_entries: 1000,
            summary_enabled: false,
            summary_trigger_messages: 12,
            summary_max_chars: 1500,
        }
    }

    fn phase_inference_server(default_phase: &str, phase_names: &[&str]) -> AcpServer {
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
            },
        );

        let phases = phase_names
            .iter()
            .map(|name| {
                (
                    (*name).to_string(),
                    PhaseConfig {
                        description: format!("{} phase", name),
                        agents: vec!["copilot".to_string()],
                        fallback: Some(true),
                        principles: None,
                        options: None,
                    },
                )
            })
            .collect::<HashMap<_, _>>();

        let config = Arc::new(AppConfig {
            default_phase: default_phase.to_string(),
            agents,
            flow: FlowConfig {
                name: "test-flow".to_string(),
                phases: phase_names.iter().map(|name| (*name).to_string()).collect(),
            },
            phases,
            runtime: Some(RuntimeConfig::default()),
            cache: None,
            vector: None,
            autotune: None,
            model_selection_mode: "adaptive".to_string(),
        });

        let flow = Arc::new(FlowManager::new(Arc::clone(&config), None));
        let registry = Arc::new(
            AgentRegistry::from_config(Arc::clone(&config), reqwest::Client::new())
                .expect("test registry should build"),
        );

        AcpServer::new(
            flow,
            registry,
            None,
            None,
            None,
            None,
            None,
            None,
            RuntimeConfig::default(),
            None,
            None,
            None,
            false,
        )
    }

    fn phase_inference_flow(default_phase: &str, phase_names: &[&str]) -> FlowManager {
        let phases = phase_names
            .iter()
            .map(|name| {
                (
                    (*name).to_string(),
                    PhaseConfig {
                        description: format!("{} phase", name),
                        agents: vec!["copilot".to_string()],
                        fallback: Some(true),
                        principles: None,
                        options: None,
                    },
                )
            })
            .collect::<HashMap<_, _>>();

        FlowManager::new(
            Arc::new(AppConfig {
                default_phase: default_phase.to_string(),
                agents: HashMap::from([(
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
                    },
                )]),
                flow: FlowConfig {
                    name: "test-flow".to_string(),
                    phases: phase_names.iter().map(|name| (*name).to_string()).collect(),
                },
                phases,
                runtime: Some(RuntimeConfig::default()),
                cache: None,
                vector: None,
                autotune: None,
                model_selection_mode: "adaptive".to_string(),
            }),
            None,
        )
    }

    #[test]
    fn chat_mode_parsing() {
        assert_eq!(ChatMode::parse(Some("ask")), Some(ChatMode::Ask));
        assert_eq!(ChatMode::parse(Some("edit")), Some(ChatMode::Edit));
        assert_eq!(ChatMode::parse(Some("agent")), Some(ChatMode::Agent));
        assert_eq!(ChatMode::parse(Some("full_auto")), Some(ChatMode::FullAuto));
        assert_eq!(ChatMode::parse(Some("FULL-AUTO")), Some(ChatMode::FullAuto));
        assert_eq!(ChatMode::parse(Some("unknown")), None);
        assert_eq!(ChatMode::parse(None), None);
    }

    #[test]
    fn autopilot_complexity_parsing() {
        assert_eq!(
            AutopilotComplexity::from_str("simple"),
            Some(AutopilotComplexity::Simple)
        );
        assert_eq!(
            AutopilotComplexity::from_str("complex"),
            Some(AutopilotComplexity::Complex)
        );
        assert_eq!(
            AutopilotComplexity::from_str("SIMPLE"),
            Some(AutopilotComplexity::Simple)
        );
        assert_eq!(AutopilotComplexity::from_str("unknown"), None);
    }

    #[test]
    fn mode_to_strategy_mapping() {
        assert_eq!(
            mode_to_approval_strategy(Some(ChatMode::Ask), None),
            ApprovalStrategy::DefaultApprovals
        );
        assert_eq!(
            mode_to_approval_strategy(Some(ChatMode::Edit), None),
            ApprovalStrategy::ByPassApproval
        );
        assert_eq!(
            mode_to_approval_strategy(Some(ChatMode::Agent), None),
            ApprovalStrategy::ByPassApproval
        );
        assert_eq!(
            mode_to_approval_strategy(Some(ChatMode::FullAuto), Some(AutopilotComplexity::Simple)),
            ApprovalStrategy::AutoPilotSimple
        );
        assert_eq!(
            mode_to_approval_strategy(Some(ChatMode::FullAuto), Some(AutopilotComplexity::Complex)),
            ApprovalStrategy::AutoPilotComplex
        );
        assert_eq!(
            mode_to_approval_strategy(Some(ChatMode::FullAuto), None),
            ApprovalStrategy::AutoPilotSimple
        );
        assert_eq!(
            mode_to_approval_strategy(None, None),
            ApprovalStrategy::DefaultApprovals
        );
    }

    #[test]
    fn conversation_checkpoint_roundtrip_and_rollback() {
        let server = phase_inference_server("coding", &["coding", "review"]);
        let first_messages = vec![Message {
            role: "user".to_string(),
            content: "draft plan".to_string(),
        }];

        let first = server
            .create_conversation_checkpoint(
                "conv-a",
                "main",
                first_messages.clone(),
                Some("initial".to_string()),
            )
            .expect("first checkpoint should be created");
        let second = server
            .create_conversation_checkpoint(
                "conv-a",
                "main",
                vec![Message {
                    role: "assistant".to_string(),
                    content: "second response".to_string(),
                }],
                Some("second".to_string()),
            )
            .expect("second checkpoint should be created");

        let listed = server
            .list_conversation_checkpoints("conv-a", Some("main"), 10)
            .expect("list should succeed");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].checkpoint_id, second.checkpoint_id);
        assert_eq!(listed[1].checkpoint_id, first.checkpoint_id);

        let restored = server
            .rollback_conversation_checkpoint("conv-a", &first.checkpoint_id, Some("hotfix"))
            .expect("rollback should locate target checkpoint");
        assert_eq!(restored.branch_id, "hotfix");
        assert_ne!(restored.checkpoint_id, first.checkpoint_id);
        assert_eq!(
            restored.parent_checkpoint_id.as_deref(),
            Some(first.checkpoint_id.as_str())
        );
        assert_eq!(restored.messages.len(), first_messages.len());
        assert_eq!(restored.messages[0].content, first_messages[0].content);

        let prune = server.prune_conversation_checkpoints("conv-a", Some("main"), 1);
        assert_eq!(prune.removed, 1);

        let hotfix_checkpoint = server
            .create_conversation_checkpoint(
                "conv-a",
                "hotfix",
                vec![Message {
                    role: "assistant".to_string(),
                    content: "hotfix follow-up".to_string(),
                }],
                Some("hotfix checkpoint".to_string()),
            )
            .expect("hotfix checkpoint should be created");
        assert_eq!(
            hotfix_checkpoint.parent_checkpoint_id.as_deref(),
            Some(restored.checkpoint_id.as_str())
        );
    }

    #[test]
    fn rollback_preserves_target_checkpoint_under_capacity_pressure() {
        let server = phase_inference_server("coding", &["coding", "review"]);
        let mut first_checkpoint_id = None;

        for idx in 0..MAX_CHECKPOINTS_PER_CONVERSATION {
            let cp = server
                .create_conversation_checkpoint(
                    "conv-cap",
                    "main",
                    vec![Message {
                        role: "user".to_string(),
                        content: format!("message-{idx}"),
                    }],
                    None,
                )
                .expect("checkpoint creation should succeed");
            if idx == 0 {
                first_checkpoint_id = Some(cp.checkpoint_id);
            }
        }

        let target = first_checkpoint_id.expect("first checkpoint id should be captured");
        let restored = server
            .rollback_conversation_checkpoint("conv-cap", &target, Some("hotfix"))
            .expect("rollback should succeed");

        assert_eq!(
            restored.parent_checkpoint_id.as_deref(),
            Some(target.as_str())
        );

        let store = server
            .conversation_store
            .lock()
            .expect("conversation store lock should succeed");
        let state = store
            .get("conv-cap")
            .expect("conversation state should exist");
        assert_eq!(state.checkpoints.len(), MAX_CHECKPOINTS_PER_CONVERSATION);
        assert!(state
            .checkpoints
            .iter()
            .any(|cp| cp.checkpoint_id == target));
    }

    #[test]
    fn stream_limits_reject_next_token_before_append() {
        assert!(stream_would_exceed_limits(0, MAX_STREAM_CHARS, 1));
        assert!(stream_would_exceed_limits(MAX_STREAM_CHUNKS, 0, 1));
        assert!(!stream_would_exceed_limits(
            MAX_STREAM_CHUNKS.saturating_sub(1),
            MAX_STREAM_CHARS.saturating_sub(1),
            1
        ));
    }

    #[test]
    fn infer_phase_prefers_explicit_phase_over_mode_default() {
        let server = phase_inference_server("planning", &["planning", "review", "coding"]);
        let flow = phase_inference_flow("planning", &["planning", "review", "coding"]);

        assert_eq!(
            server.infer_phase_name_with_flow(&flow, Some("delivery"), Some(ChatMode::Ask)),
            "delivery"
        );
    }

    #[test]
    fn infer_phase_uses_review_for_ask_when_available() {
        let server = phase_inference_server("planning", &["planning", "review"]);
        let flow = phase_inference_flow("planning", &["planning", "review"]);

        assert_eq!(
            server.infer_phase_name_with_flow(&flow, None, Some(ChatMode::Ask)),
            "review"
        );
    }

    #[test]
    fn infer_phase_uses_coding_for_edit_agent_and_full_auto() {
        let server = phase_inference_server("planning", &["planning", "coding"]);
        let flow = phase_inference_flow("planning", &["planning", "coding"]);

        assert_eq!(
            server.infer_phase_name_with_flow(&flow, None, Some(ChatMode::Edit)),
            "coding"
        );
        assert_eq!(
            server.infer_phase_name_with_flow(&flow, None, Some(ChatMode::Agent)),
            "coding"
        );
        assert_eq!(
            server.infer_phase_name_with_flow(&flow, None, Some(ChatMode::FullAuto)),
            "coding"
        );
    }

    #[test]
    fn infer_phase_falls_back_to_default_when_mode_phase_missing() {
        let server = phase_inference_server("planning", &["planning"]);
        let flow = phase_inference_flow("planning", &["planning"]);

        assert_eq!(
            server.infer_phase_name_with_flow(&flow, None, Some(ChatMode::Ask)),
            "planning"
        );
        assert_eq!(
            server.infer_phase_name_with_flow(&flow, None, Some(ChatMode::FullAuto)),
            "planning"
        );
    }

    #[test]
    fn approval_strategy_dual_review_check() {
        assert!(!ApprovalStrategy::DefaultApprovals.needs_dual_review());
        assert!(!ApprovalStrategy::ByPassApproval.needs_dual_review());
        assert!(!ApprovalStrategy::AutoPilotSimple.needs_dual_review());
        assert!(ApprovalStrategy::AutoPilotComplex.needs_dual_review());
    }

    #[test]
    fn optimize_messages_respects_limits() {
        let options = PhaseOptions {
            max_history_messages: Some(2),
            max_history_chars: Some(10),
            ..PhaseOptions::default()
        };
        let messages = vec![
            Message {
                role: "user".to_string(),
                content: "12345".to_string(),
            },
            Message {
                role: "assistant".to_string(),
                content: "67890".to_string(),
            },
            Message {
                role: "user".to_string(),
                content: "abc".to_string(),
            },
        ];

        let optimized = optimize_messages(&messages, Some(&options));
        assert_eq!(optimized.len(), 2);
        assert_eq!(optimized[0].content, "67890");
        assert_eq!(optimized[1].content, "abc");
    }

    #[test]
    fn append_recent_summary_keeps_recent_tail() {
        let summary =
            append_recent_summary(Some("old summary"), Some("new question"), "new answer", 24);

        assert!(summary.contains("new answer"));
    }

    #[test]
    fn review_verdict_requires_approve_first_line() {
        assert_eq!(
            review_verdict("APPROVE\nLooks safe.", 8),
            ReviewVerdict::Approve
        );
        assert_eq!(
            review_verdict("REJECT\nMissing tests.", 8),
            ReviewVerdict::Reject
        );
        assert_eq!(
            review_verdict("Looks fine, APPROVE", 8),
            ReviewVerdict::Invalid
        );
        assert_eq!(review_verdict("OK", 8), ReviewVerdict::Invalid);
    }

    #[test]
    fn review_timeout_prefers_review_phase_override() {
        let review_options = PhaseOptions {
            review_timeout_seconds: Some(15),
            request_timeout_seconds: Some(30),
            ..PhaseOptions::default()
        };
        let primary_options = PhaseOptions {
            review_timeout_seconds: Some(20),
            request_timeout_seconds: Some(40),
            ..PhaseOptions::default()
        };

        let timeout = review_timeout(Some(&review_options), Some(&primary_options));
        assert_eq!(timeout.map(|value| value.as_secs()), Some(15));
    }

    #[test]
    fn vector_defaults_fall_back_to_global_config() {
        let vector_config = vector_config_fixture();

        assert!(!effective_vector_auto(None, Some(&vector_config)));
        assert_eq!(
            effective_vector_min_query_chars(None, Some(&vector_config), None),
            140
        );
        assert_eq!(effective_vector_top_k(None, Some(&vector_config), None), 4);
        assert_eq!(
            effective_vector_min_similarity(None, Some(&vector_config)),
            0.91
        );
        assert_eq!(
            effective_vector_max_snippet_chars(None, Some(&vector_config)),
            640
        );
        assert!(!effective_summary_enabled(None, Some(&vector_config)));
        assert_eq!(
            effective_summary_trigger_messages(None, Some(&vector_config)),
            12
        );
    }

    #[test]
    fn autotune_thresholds_override_static_vector_defaults() {
        let vector_config = vector_config_fixture();
        let tuned_state = AutoTuneState {
            current_min_query_chars: 95,
            current_top_k: 3,
            window_phase: 0,
            high_precision_count: 0,
            low_precision_count: 0,
            vector_search_count: 0,
            cooldown_remaining: 0,
        };

        assert_eq!(
            effective_vector_min_query_chars(None, Some(&vector_config), Some(&tuned_state)),
            95
        );
        assert_eq!(
            effective_vector_top_k(None, Some(&vector_config), Some(&tuned_state)),
            3
        );
    }

    #[test]
    fn autotune_snapshot_includes_all_fields() {
        let config = AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "test.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        };

        let mut state = AutoTuneState::new(&config);
        state.current_min_query_chars = 120;
        state.current_top_k = 3;
        state.window_phase = 5;
        state.high_precision_count = 12;
        state.low_precision_count = 2;
        state.vector_search_count = 18;
        state.cooldown_remaining = 1;

        let snapshot = state.snapshot();
        assert_eq!(snapshot["current_min_query_chars"], 120);
        assert_eq!(snapshot["current_top_k"], 3);
        assert_eq!(snapshot["window_phase"], 5);
        assert_eq!(snapshot["high_precision_count"], 12);
        assert_eq!(snapshot["low_precision_count"], 2);
        assert_eq!(snapshot["vector_search_count"], 18);
        assert_eq!(snapshot["cooldown_remaining"], 1);
    }

    // Integration tests for full ACP protocol flow
    #[test]
    fn initialize_request_returns_server_capabilities() {
        let request_json = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let request: JsonRpcRequest = serde_json::from_str(request_json).unwrap();

        assert_eq!(request.jsonrpc, "2.0");
        assert_eq!(request.id, Some(Value::Number(1.into())));
        assert_eq!(request.method, "initialize");
    }

    #[test]
    fn metrics_snapshot_structure() {
        let metrics = RuntimeMetrics::default();
        metrics.inc_chat_requests();
        metrics.inc_cache_lookup();
        metrics.inc_cache_hit();
        metrics.inc_vector_search();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.chat_requests_total, 1);
        assert_eq!(snapshot.cache_lookup_total, 1);
        assert_eq!(snapshot.cache_hit_total, 1);
        assert_eq!(snapshot.vector_search_total, 1);
    }

    #[test]
    fn jsonrpc_response_serialization() {
        let response = JsonRpcResponse {
            jsonrpc: "2.0",
            id: Some(Value::Number(1.into())),
            result: Some(json!({"status": "ok"})),
            error: None,
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 1);
        assert_eq!(json["result"]["status"], "ok");
        assert!(json.get("error").is_none());
    }

    #[test]
    fn jsonrpc_error_response_serialization() {
        let response = JsonRpcResponse {
            jsonrpc: "2.0",
            id: Some(Value::Number(2.into())),
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: "Method not found".to_string(),
                data: None,
            }),
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 2);
        assert_eq!(json["error"]["code"], -32601);
        assert!(json.get("result").is_none());
    }

    // Cache hit integration test
    #[test]
    fn cache_hit_increments_metrics() {
        let metrics = RuntimeMetrics::default();
        metrics.inc_cache_lookup();
        metrics.inc_cache_hit();
        metrics.inc_cache_lookup();
        metrics.inc_cache_hit();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.cache_lookup_total, 2);
        assert_eq!(snapshot.cache_hit_total, 2);
    }

    #[test]
    fn cache_miss_tracked_correctly() {
        let metrics = RuntimeMetrics::default();
        metrics.inc_cache_lookup();
        // No hit incremented
        metrics.inc_cache_lookup();
        metrics.inc_cache_hit();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.cache_lookup_total, 2);
        assert_eq!(snapshot.cache_hit_total, 1);
    }

    // Dual review integration test
    #[test]
    fn autopilot_complex_requires_dual_review() {
        let mode = ChatMode::FullAuto;
        let complexity = AutopilotComplexity::Complex;
        let strategy = mode_to_approval_strategy(Some(mode), Some(complexity));

        assert_eq!(strategy, ApprovalStrategy::AutoPilotComplex);
        assert!(strategy.needs_dual_review());
    }

    #[test]
    fn autopilot_simple_bypasses_dual_review() {
        let mode = ChatMode::FullAuto;
        let complexity = AutopilotComplexity::Simple;
        let strategy = mode_to_approval_strategy(Some(mode), Some(complexity));

        assert_eq!(strategy, ApprovalStrategy::AutoPilotSimple);
        assert!(!strategy.needs_dual_review());
    }

    #[test]
    fn edit_mode_bypasses_approvals() {
        let mode = ChatMode::Edit;
        let strategy = mode_to_approval_strategy(Some(mode), None);

        assert!(!strategy.needs_dual_review());
        assert_eq!(strategy.as_str(), "by_pass_approval");
    }

    // Fallback chain integration test
    #[test]
    fn approval_strategy_fallback_chain() {
        // Test: Ask mode (default) requires approval
        let strategy_ask = mode_to_approval_strategy(Some(ChatMode::Ask), None);
        assert_eq!(strategy_ask, ApprovalStrategy::DefaultApprovals);

        // Test: No mode defaults to Ask behavior
        let strategy_none = mode_to_approval_strategy(None, None);
        assert_eq!(strategy_none, ApprovalStrategy::DefaultApprovals);

        // Test: FullAuto without complexity defaults to Simple
        let strategy_auto = mode_to_approval_strategy(Some(ChatMode::FullAuto), None);
        assert_eq!(strategy_auto, ApprovalStrategy::AutoPilotSimple);
    }

    #[test]
    fn strategy_string_representations() {
        let strategies = vec![
            (ApprovalStrategy::DefaultApprovals, "default_approvals"),
            (ApprovalStrategy::ByPassApproval, "by_pass_approval"),
            (ApprovalStrategy::AutoPilotSimple, "autopilot_simple"),
            (ApprovalStrategy::AutoPilotComplex, "autopilot_complex"),
        ];

        for (strategy, expected) in strategies {
            assert_eq!(strategy.as_str(), expected);
        }
    }

    #[test]
    fn resolve_primary_secondary_policy_defaults_to_single_primary_and_ranked_secondary() {
        let agents = vec![
            "primary-a".to_string(),
            "secondary-b".to_string(),
            "secondary-c".to_string(),
        ];
        let params = json!({});

        let policy = resolve_primary_secondary_policy(&agents, &params, None).unwrap();

        assert_eq!(policy.primary_agent, "primary-a");
        assert_eq!(
            policy.secondary_agents,
            vec!["secondary-b".to_string(), "secondary-c".to_string()]
        );
        assert_eq!(policy.failover_policy, "first_secondary");
        assert_eq!(policy.policy_version, "blue5.v1");
    }

    #[test]
    fn resolve_primary_secondary_policy_rejects_non_candidate_primary() {
        let agents = vec!["agent-a".to_string(), "agent-b".to_string()];
        let params = json!({"primary_agent": "agent-x"});

        let err = resolve_primary_secondary_policy(&agents, &params, None).unwrap_err();
        // Check for translation key since i18n system may not be initialized in tests
        assert!(err.to_string().contains("error.primary_agent_not_found"));
    }

    #[test]
    fn online_controller_ranks_agents_by_live_phase_outcomes() {
        let mut state = OnlineControllerState::default();

        for _ in 0..6 {
            state.record_agent_outcome("coding", "copilot", false, 10_000);
            state.record_agent_outcome("coding", "deepseek", true, 1_200);
        }

        let ranked = state
            .rank_agent_names_for_phase("coding", &["copilot".to_string(), "deepseek".to_string()]);

        assert_eq!(ranked[0].0, "deepseek");
        assert_eq!(ranked[1].0, "copilot");
        assert!(ranked[0].1 > ranked[1].1);
    }

    #[test]
    fn online_controller_keeps_original_order_without_enough_samples() {
        let mut state = OnlineControllerState::default();
        state.record_agent_outcome("coding", "copilot", true, 1_100);
        state.record_agent_outcome("coding", "deepseek", false, 1_100);

        let ranked = state
            .rank_agent_names_for_phase("coding", &["copilot".to_string(), "deepseek".to_string()]);

        assert_eq!(ranked[0].0, "copilot");
        assert_eq!(ranked[1].0, "deepseek");
    }

    #[test]
    fn circuit_breaker_transitions_to_half_open_and_closes_on_success() {
        let breaker = CircuitBreakerRegistry::default();

        breaker.record_failure_with_config("copilot", 2, 1);
        assert!(matches!(
            breaker.allow_request("copilot"),
            CircuitBreakerAdmission::Closed
        ));

        breaker.record_failure_with_config("copilot", 2, 1);
        let snapshot = breaker.snapshot();
        assert_eq!(snapshot["copilot"].state, "open");

        assert!(matches!(
            breaker.allow_request("copilot"),
            CircuitBreakerAdmission::Rejected {
                state: "open",
                retry_after_seconds: Some(_)
            }
        ));

        {
            let mut guard = breaker.inner.lock().unwrap();
            let state = guard.get_mut("copilot").unwrap();
            state.open_until = Some(now_ts() - 1);
        }

        assert!(matches!(
            breaker.allow_request("copilot"),
            CircuitBreakerAdmission::HalfOpenProbe
        ));
        assert!(matches!(
            breaker.allow_request("copilot"),
            CircuitBreakerAdmission::Rejected {
                state: "half_open",
                retry_after_seconds: None
            }
        ));

        breaker.record_success("copilot");
        let snapshot = breaker.snapshot();
        assert_eq!(snapshot["copilot"].state, "closed");
        assert_eq!(snapshot["copilot"].consecutive_failures, 0);
        assert!(!snapshot["copilot"].probe_in_flight);
    }

    #[test]
    fn circuit_breaker_half_open_failure_reopens_breaker() {
        let breaker = CircuitBreakerRegistry::default();

        breaker.record_failure_with_config("claude", 1, 1);
        {
            let mut guard = breaker.inner.lock().unwrap();
            let state = guard.get_mut("claude").unwrap();
            state.open_until = Some(now_ts() - 1);
        }

        assert!(matches!(
            breaker.allow_request("claude"),
            CircuitBreakerAdmission::HalfOpenProbe
        ));

        breaker.record_failure_with_config("claude", 1, 3);
        let snapshot = breaker.snapshot();
        assert_eq!(snapshot["claude"].state, "open");
        assert_eq!(snapshot["claude"].consecutive_failures, 1);
        assert!(!snapshot["claude"].probe_in_flight);
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn prometheus_export_includes_headers_and_runtime_labels() {
        let mut snapshot = MetricsSnapshot::default();
        snapshot.chat_requests_total = 3;
        snapshot.cache_hit_total = 2;
        snapshot.review_gate_timeout_total = 1;
        snapshot.review_gate_degraded_total = 1;
        snapshot.review_gate_invalid_response_total = 1;
        snapshot.chat_latency_count = 1;
        snapshot.chat_latency_sum_seconds = 0.25;
        snapshot.chat_latency_bucket_counts[1] = 1;

        let gauges = RuntimeGaugeSnapshot {
            memory_cache_entries: 4,
            sqlite_cache_entries: 6,
            vector_memory_entries: 8,
            vector_summary_entries: 2,
            circuit_open_agents: 1,
            circuit_half_open_agents: 1,
            circuit_tracked_agents: 2,
            rate_limiter_tracked_phases: 1,
        };

        let breaker_snapshot = HashMap::from([(
            "copilot-main".to_string(),
            CircuitBreakerSnapshot {
                consecutive_failures: 3,
                state: "half_open_ready".to_string(),
                open_until: Some(now_ts() + 5),
                probe_in_flight: false,
            },
        )]);
        let phase_limiter_snapshot = HashMap::from([("coding".to_string(), (4.5, 12.0))]);
        let inflight_snapshot = (2_usize, HashMap::from([("coding".to_string(), 1_usize)]));
        let lifecycle = LifecycleSnapshot {
            shutting_down: true,
            shutdown_started_at: Some(now_ts()),
            shutdown_reason: Some("unit-test".to_string()),
        };
        let maintenance = MaintenanceSnapshot {
            running: true,
            cycles_total: 7,
            last_started_at: Some(now_ts()),
            last_completed_at: Some(now_ts()),
            last_memory_expired_removed: 3,
            last_sqlite_expired_removed: 5,
            last_cache_vacuumed: false,
            last_vector_vacuumed: false,
            last_error: None,
        };

        let rendered = build_prometheus_metrics(
            &snapshot,
            &gauges,
            &breaker_snapshot,
            &phase_limiter_snapshot,
            &inflight_snapshot,
            &lifecycle,
            &maintenance,
        );

        assert!(rendered.contains("# HELP acp_chat_requests_total Total ACP chat requests handled"));
        assert!(rendered.contains("# TYPE acp_chat_requests_total counter"));
        assert!(rendered.contains("acp_review_gate_timeout_total 1"));
        assert!(rendered.contains("acp_review_gate_degraded_total 1"));
        assert!(rendered.contains("acp_review_gate_invalid_response_total 1"));
        assert!(rendered.contains("acp_inflight_requests{scope=\"global\"} 2"));
        assert!(rendered.contains("acp_inflight_requests{scope=\"phase\",phase=\"coding\"} 1"));
        assert!(rendered.contains(
            "acp_circuit_breaker_state{agent=\"copilot-main\",state=\"half_open_ready\"} 1"
        ));
        assert!(rendered.contains("acp_lifecycle_shutting_down 1"));
        assert!(rendered.contains("acp_maintenance_cycles_total 7"));
        assert!(rendered.contains("acp_lazy_blue5_doc_lookup_total 0"));
        assert!(rendered.contains("acp_chat_latency_seconds_bucket{le=\"0.25\"} 1"));
    }

    #[test]
    fn metrics_reset_clears_all_counters() {
        let metrics = RuntimeMetrics::default();
        metrics.inc_chat_requests();
        metrics.inc_cache_hit();
        metrics.inc_vector_search();

        let snapshot1 = metrics.snapshot();
        assert!(snapshot1.chat_requests_total > 0);

        metrics.reset();
        let snapshot2 = metrics.snapshot();
        assert_eq!(snapshot2.chat_requests_total, 0);
        assert_eq!(snapshot2.cache_hit_total, 0);
        assert_eq!(snapshot2.vector_search_total, 0);
    }

    #[test]
    fn record_agent_failure_metrics_tracks_timeout_bucket() {
        let metrics = RuntimeMetrics::default();
        let err = anyhow::anyhow!("agent timed out after 15s");

        record_agent_failure_metrics(&metrics, &err);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.agent_failures_total, 1);
        assert_eq!(snapshot.agent_timeout_failures_total, 1);
        assert_eq!(snapshot.agent_panic_failures_total, 0);
        assert_eq!(snapshot.agent_other_failures_total, 0);
    }

    #[test]
    fn record_agent_failure_metrics_tracks_panic_and_other_buckets() {
        let metrics = RuntimeMetrics::default();
        let panic_err = anyhow::anyhow!("agent panic: task join error");
        let other_err = anyhow::anyhow!("remote provider returned malformed payload");

        record_agent_failure_metrics(&metrics, &panic_err);
        record_agent_failure_metrics(&metrics, &other_err);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.agent_failures_total, 2);
        assert_eq!(snapshot.agent_timeout_failures_total, 0);
        assert_eq!(snapshot.agent_panic_failures_total, 1);
        assert_eq!(snapshot.agent_other_failures_total, 1);
    }

    // === ACP Runtime RPC Integration Tests ===
    // These tests verify the JSON-RPC protocol contract for ACP server endpoints.

    #[test]
    fn rpc_initialize_response_includes_server_name_and_capabilities() {
        let server = phase_inference_server("planning", &["planning", "coding"]);

        // Verify request parsing
        let request_json = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let request: JsonRpcRequest = serde_json::from_str(request_json).unwrap();

        assert_eq!(request.method, "initialize");
        assert_eq!(request.id, Some(Value::Number(1.into())));

        // Runtime defaults are injected when no explicit runtime block is provided.
        assert!(server.runtime_config_snapshot().shutdown_drain_seconds > 0);
    }

    #[test]
    fn rpc_metrics_snapshot_includes_all_metric_types() {
        let metrics = RuntimeMetrics::default();
        metrics.inc_chat_requests();
        metrics.inc_cache_lookup();
        metrics.inc_cache_hit();
        metrics.inc_vector_search();
        metrics.inc_vector_hit();
        metrics.inc_summary_read();
        metrics.inc_summary_hit();
        metrics.inc_agent_failures();
        metrics.inc_agent_timeout_failures();
        metrics.inc_review_gate();
        metrics.inc_review_gate_approved();
        metrics.inc_review_gate_timeout();
        metrics.inc_review_gate_degraded();
        metrics.inc_review_gate_invalid_response();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.chat_requests_total, 1);
        assert_eq!(snapshot.cache_lookup_total, 1);
        assert_eq!(snapshot.cache_hit_total, 1);
        assert_eq!(snapshot.vector_search_total, 1);
        assert_eq!(snapshot.vector_hit_total, 1);
        assert_eq!(snapshot.summary_read_total, 1);
        assert_eq!(snapshot.summary_hit_total, 1);
        assert_eq!(snapshot.agent_failures_total, 1);
        assert_eq!(snapshot.agent_timeout_failures_total, 1);
        assert_eq!(snapshot.review_gate_total, 1);
        assert_eq!(snapshot.review_gate_approved_total, 1);
        assert_eq!(snapshot.review_gate_timeout_total, 1);
        assert_eq!(snapshot.review_gate_degraded_total, 1);
        assert_eq!(snapshot.review_gate_invalid_response_total, 1);
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn rpc_prometheus_metrics_serializes_to_valid_format() {
        let mut snapshot = MetricsSnapshot::default();
        snapshot.chat_requests_total = 42;
        snapshot.cache_hit_total = 15;
        snapshot.agent_failures_total = 2;
        snapshot.agent_timeout_failures_total = 1;
        snapshot.agent_panic_failures_total = 1;
        snapshot.review_gate_total = 3;
        snapshot.review_gate_approved_total = 2;
        snapshot.review_gate_timeout_total = 1;
        snapshot.review_gate_degraded_total = 1;
        snapshot.review_gate_invalid_response_total = 1;
        snapshot.lazy_blue5_doc_lookup_total = 4;

        let gauges = RuntimeGaugeSnapshot {
            memory_cache_entries: 12,
            sqlite_cache_entries: 45,
            vector_memory_entries: 8,
            vector_summary_entries: 3,
            circuit_open_agents: 1,
            circuit_half_open_agents: 0,
            circuit_tracked_agents: 2,
            rate_limiter_tracked_phases: 4,
        };

        let prometheus = build_prometheus_metrics(
            &snapshot,
            &gauges,
            &HashMap::new(),
            &HashMap::new(),
            &(0, HashMap::new()),
            &LifecycleSnapshot::default(),
            &MaintenanceSnapshot::default(),
        );

        assert!(prometheus.contains("acp_chat_requests_total 42"));
        assert!(prometheus.contains("acp_cache_hit_total 15"));
        assert!(prometheus.contains("acp_agent_failures_total 2"));
        assert!(prometheus.contains("acp_agent_timeout_failures_total 1"));
        assert!(prometheus.contains("acp_agent_panic_failures_total 1"));
        assert!(prometheus.contains("acp_review_gate_total 3"));
        assert!(prometheus.contains("acp_review_gate_approved_total 2"));
        assert!(prometheus.contains("acp_review_gate_timeout_total 1"));
        assert!(prometheus.contains("acp_review_gate_degraded_total 1"));
        assert!(prometheus.contains("acp_review_gate_invalid_response_total 1"));
        assert!(prometheus.contains("acp_lazy_blue5_doc_lookup_total 4"));
        assert!(prometheus.contains("acp_memory_cache_entries 12"));
        assert!(prometheus.contains("acp_circuit_tracked_agents 2"));
        assert!(prometheus.contains("acp_rate_limiter_tracked_phases 4"));
    }

    #[test]
    fn rpc_runtime_health_includes_all_subsystems() {
        let server = phase_inference_server("planning", &["planning", "coding"]);
        let memory_cache = &server.memory_cache;
        let circuit_breakers = &server.circuit_breakers;
        let phase_rate_limiter = &server.phase_rate_limiter;
        let inflight_limiter = &server.inflight_limiter;

        // Verify cache is accessible
        assert_eq!(memory_cache.active_entries(), 0);

        // Verify circuit breaker state
        let cb_snapshot = circuit_breakers.snapshot();
        assert!(cb_snapshot.is_empty());
        assert_eq!(circuit_breakers.tracked_agents(), 0);

        // Verify rate limiter
        assert_eq!(phase_rate_limiter.tracked_phases(), 0);

        // Verify inflight tracking
        let (global, phases) = inflight_limiter.snapshot();
        assert_eq!(global, 0);
        assert!(phases.is_empty());
    }

    #[test]
    fn rpc_phase_status_tracks_rate_limiter_state() {
        let phase_limiter = PhaseRateLimiter::default();

        // Test token bucket state tracking
        assert!(phase_limiter.allow("planning", 60, None));
        assert_eq!(phase_limiter.tracked_phases(), 1);

        let snapshot = phase_limiter.snapshot();
        assert!(snapshot.contains_key("planning"));
        let (tokens, capacity) = snapshot["planning"];
        assert!(tokens < capacity);
        assert_eq!(capacity, 60.0);
    }

    #[test]
    fn rpc_phase_status_burst_capacity_respected() {
        let phase_limiter = PhaseRateLimiter::default();

        // Allow requests up to burst capacity
        for _ in 0..5 {
            assert!(phase_limiter.allow("coding", 60, Some(5)));
        }

        // 6th request should fail
        assert!(!phase_limiter.allow("coding", 60, Some(5)));

        // Verify capacity constraint
        let snapshot = phase_limiter.snapshot();
        assert!(snapshot.contains_key("coding"));
        let (tokens, _) = snapshot["coding"];
        // Tokens should be less than 1.0 (since we just consumed one)
        assert!(tokens < 1.0);
    }

    #[test]
    fn rpc_inflight_limiter_enforces_phase_and_global_limits() {
        let limiter = Arc::new(InflightLimiter::default());

        // Test phase limit
        let guard1 = limiter.clone().try_enter("planning", Some(2), Some(5));
        assert!(guard1.is_some());

        let guard2 = limiter.clone().try_enter("planning", Some(2), Some(5));
        assert!(guard2.is_some());

        let guard3 = limiter.clone().try_enter("planning", Some(2), Some(5));
        assert!(guard3.is_none());

        drop(guard1);
        let guard4 = limiter.clone().try_enter("planning", Some(2), Some(5));
        assert!(guard4.is_some());

        let (global, _) = limiter.snapshot();
        assert_eq!(global, 2);
    }

    #[test]
    fn rpc_inflight_limiter_global_limit_respected() {
        let limiter = Arc::new(InflightLimiter::default());

        let mut guards = Vec::new();
        for _ in 0..3 {
            let guard = limiter.clone().try_enter("planning", None, Some(3));
            assert!(guard.is_some());
            guards.push(guard);
        }

        let guard4 = limiter.clone().try_enter("coding", None, Some(3));
        assert!(guard4.is_none());

        drop(guards.pop());
        let guard5 = limiter.clone().try_enter("coding", None, Some(3));
        assert!(guard5.is_some());
    }

    #[test]
    fn rpc_lifecycle_state_tracks_shutdown() {
        let lifecycle = LifecycleState::default();

        assert!(!lifecycle.is_shutting_down());
        assert!(lifecycle.start_shutdown("test shutdown"));
        assert!(lifecycle.is_shutting_down());

        // Second call should fail
        assert!(!lifecycle.start_shutdown("already shutting down"));

        let snapshot = lifecycle.snapshot();
        assert!(snapshot.shutting_down);
        assert_eq!(snapshot.shutdown_reason, Some("test shutdown".to_string()));
        assert!(snapshot.shutdown_started_at.is_some());
    }

    #[test]
    fn rpc_maintenance_tracker_records_cycle_metrics() {
        let maintenance = MaintenanceTracker::default();

        maintenance.note_started();
        let snapshot1 = maintenance.snapshot();
        assert!(snapshot1.running);
        assert_eq!(snapshot1.cycles_total, 1);

        maintenance.note_completed(5, 3, true, false);
        let snapshot2 = maintenance.snapshot();
        assert!(!snapshot2.running);
        assert_eq!(snapshot2.last_memory_expired_removed, 5);
        assert_eq!(snapshot2.last_sqlite_expired_removed, 3);
        assert!(snapshot2.last_cache_vacuumed);
        assert!(!snapshot2.last_vector_vacuumed);
        assert_eq!(snapshot2.cycles_total, 1);
    }

    #[test]
    fn rpc_maintenance_tracker_records_failures() {
        let maintenance = MaintenanceTracker::default();

        maintenance.note_started();
        maintenance.note_failed("connection timeout");

        let snapshot = maintenance.snapshot();
        assert!(!snapshot.running);
        assert_eq!(snapshot.last_error, Some("connection timeout".to_string()));
    }

    #[test]
    fn rpc_circuit_breaker_snapshot_complete() {
        let breaker = CircuitBreakerRegistry::default();

        breaker.record_failure_with_config("agent-a", 2, 10);
        breaker.record_failure_with_config("agent-a", 2, 10);
        breaker.record_failure_with_config("agent-b", 1, 10);

        let snapshot = breaker.snapshot();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot["agent-a"].state, "open");
        assert_eq!(snapshot["agent-a"].consecutive_failures, 2);
        assert_eq!(snapshot["agent-b"].state, "open");
        assert_eq!(snapshot["agent-b"].consecutive_failures, 1);
    }

    #[test]
    fn rpc_metrics_reset_integration() {
        let metrics = RuntimeMetrics::default();

        metrics.inc_chat_requests();
        metrics.inc_cache_hit();
        metrics.inc_agent_failures();
        metrics.observe_chat_latency(Duration::from_secs_f64(0.25));

        let snapshot1 = metrics.snapshot();
        assert_eq!(snapshot1.chat_requests_total, 1);
        assert_eq!(snapshot1.cache_hit_total, 1);
        assert_eq!(snapshot1.agent_failures_total, 1);
        assert!(snapshot1.chat_latency_count > 0);

        metrics.reset();
        let snapshot2 = metrics.snapshot();
        assert_eq!(snapshot2.chat_requests_total, 0);
        assert_eq!(snapshot2.cache_hit_total, 0);
        assert_eq!(snapshot2.agent_failures_total, 0);
        assert_eq!(snapshot2.chat_latency_count, 0);
    }

    #[test]
    fn rpc_jsonrpc_error_codes_reserved() {
        // Verify standard JSON-RPC error codes
        assert_eq!(-32700, -32700); // Parse error
        assert_eq!(-32600, -32600); // Invalid request
        assert_eq!(-32601, -32601); // Method not found
        assert_eq!(-32602, -32602); // Invalid params
        assert_eq!(-32603, -32603); // Internal error
        assert_eq!(-32031, -32031); // Server state error (custom)
    }

    #[test]
    fn rpc_request_parsing_handles_missing_fields() {
        let request_json = r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        let request: JsonRpcRequest = serde_json::from_str(request_json).unwrap();

        assert_eq!(request.method, "initialize");
        assert_eq!(request.id, Some(Value::Number(1.into())));
        assert_eq!(request.params, None);
    }

    #[test]
    fn rpc_response_with_result_omits_error() {
        let response = JsonRpcResponse {
            jsonrpc: "2.0",
            id: Some(Value::Number(1.into())),
            result: Some(json!({"status": "ok"})),
            error: None,
        };

        let serialized = serde_json::to_string(&response).unwrap();
        assert!(serialized.contains("\"result\""));
        assert!(!serialized.contains("\"error\""));
    }

    #[test]
    fn rpc_response_with_error_omits_result() {
        let response = JsonRpcResponse {
            jsonrpc: "2.0",
            id: Some(Value::Number(2.into())),
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: "Method not found".to_string(),
                data: None,
            }),
        };

        let serialized = serde_json::to_string(&response).unwrap();
        assert!(serialized.contains("\"error\""));
        assert!(!serialized.contains("\"result\""));
    }

    #[test]
    fn rpc_notification_has_no_id() {
        let response = JsonRpcResponse {
            jsonrpc: "2.0",
            id: None,
            result: Some(json!({"type": "notification"})),
            error: None,
        };

        let serialized = serde_json::to_string(&response).unwrap();
        assert!(!serialized.contains("\"id\""));
    }

    #[test]
    fn stream_chunk_notification_includes_progress_and_context() {
        let payload = stream_chunk_notification(
            &Some(json!(123)),
            "copilot",
            "hello",
            2,
            11,
            Some("memory"),
            Some("coding"),
            Some("trace-abc"),
        );

        assert_eq!(payload["id"], 123);
        assert_eq!(payload["agent"], "copilot");
        assert_eq!(payload["token"], "hello");
        assert_eq!(payload["chunk_index"], 2);
        assert_eq!(payload["total_chars"], 11);
        assert_eq!(payload["cached"], true);
        assert_eq!(payload["cache_level"], "memory");
        assert_eq!(payload["phase"], "coding");
        assert_eq!(payload["trace_id"], "trace-abc");
    }

    #[test]
    fn stream_done_notification_marks_done_with_totals() {
        let payload = stream_done_notification(
            &Some(json!("req-7")),
            "deepseek",
            4,
            128,
            None,
            Some("review"),
            Some("trace-xyz"),
            530,
        );

        assert_eq!(payload["id"], "req-7");
        assert_eq!(payload["agent"], "deepseek");
        assert_eq!(payload["done"], true);
        assert_eq!(payload["chunks"], 4);
        assert_eq!(payload["total_chars"], 128);
        assert_eq!(payload["duration_ms"], 530);
        assert_eq!(payload["phase"], "review");
        assert_eq!(payload["trace_id"], "trace-xyz");
        assert!(payload.get("cache_level").is_none());
    }
}
