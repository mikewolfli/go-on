//! Config loading sub-modules
//!
//! This module provides config parsing, validation, migration, and
//! environment variable override functionality for the go-on application.

pub mod env_override;
pub mod migrator;
pub mod parser;
pub mod validator;

// Re-export all public items from sub-modules to preserve the public API surface.
// Callers use `crate::core::config::load::some_function()`.

pub use env_override::{
    build_config_health_report, collect_config_warnings, collect_production_strict_violations,
    is_agent_env_ready, missing_env_vars, validate_external_secret_refs,
    validate_runtime_readiness,
};
pub use parser::{ConfigHealthReport, ConfigWarning, ConfigWarningSeverity};

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use crate::core::config::types::{
        AgentConfig, AppConfig, CacheConfig, FlowConfig, PhaseConfig, PhaseOptions, RuntimeConfig,
        VectorConfig, WorkflowType,
    };

    use super::*;

    fn base_agent() -> AgentConfig {
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
        }
    }

    fn valid_config() -> AppConfig {
        let mut agents = HashMap::new();
        agents.insert("copilot".to_string(), base_agent());
        agents.insert(
            "reviewer_a".to_string(),
            AgentConfig {
                agent_type: "claude".to_string(),
                url: Some("https://api.anthropic.com".to_string()),
                chat_path: None,
                api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
                secret_key_env: None,
                anthropic_version: Some("2023-06-01".to_string()),
                model: Some("claude-3-7-sonnet-latest".to_string()),
                max_tokens: Some(4096),
                supports_system: None,
                supports_vision: None,
            },
        );
        agents.insert(
            "reviewer_b".to_string(),
            AgentConfig {
                agent_type: "wenxin".to_string(),
                url: None,
                chat_path: None,
                api_key_env: Some("WENXIN_API_KEY".to_string()),
                secret_key_env: Some("WENXIN_SECRET_KEY".to_string()),
                anthropic_version: None,
                model: None,
                max_tokens: None,
                supports_system: None,
                supports_vision: None,
            },
        );

        let mut phases = HashMap::new();
        phases.insert(
            "coding".to_string(),
            PhaseConfig {
                description: "coding".to_string(),
                agents: vec!["copilot".to_string()],
                fallback: Some(true),
                principles: None,
                options: None,
            },
        );
        phases.insert(
            "review".to_string(),
            PhaseConfig {
                description: "review".to_string(),
                agents: vec!["reviewer_a".to_string(), "reviewer_b".to_string()],
                fallback: Some(true),
                principles: None,
                options: None,
            },
        );

        AppConfig {
            schema_version: "1.0.0".to_string(),
            provider: crate::core::config::types::ProviderConfig {
                default_phase: "coding".to_string(),
                agents,
                role_registry: HashMap::new(),
            },
            flow: FlowConfig {
                name: "flow".to_string(),
                phases: vec!["coding".to_string(), "review".to_string()],
                workflow_type: WorkflowType::Auto,
            },
            phases,
            runtime: Some(RuntimeConfig::default()),
            cache: None,
            vector: None,
            autotune: None,
            security: crate::core::config::types::SecurityConfig::default(),
            feature: crate::core::config::types::FeatureConfig {
                model_selection_mode: "adaptive".to_string(),
                ..Default::default()
            },
            compliance: None,
            startup_context: None,
            reputation: None,
            protocol: None,
        }
    }

    #[test]
    fn validate_accepts_valid_configuration() {
        let cfg = valid_config();
        cfg.validate().expect("valid config should pass");
    }

    #[test]
    fn validate_rejects_default_phase_not_in_flow() {
        let mut cfg = valid_config();
        cfg.provider.default_phase = "missing".to_string();
        let err = cfg
            .validate()
            .expect_err("default phase outside flow must fail");
        assert!(
            err.to_string().contains("error.default_phase_not_in_list"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_phase_with_unknown_agent() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .agents = vec!["missing".to_string()];

        let err = cfg
            .validate()
            .expect_err("phase referencing undefined agent must fail");
        assert!(
            err.to_string()
                .contains("error.phase_references_undefined_agent"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_accepts_phase_with_no_agents() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .agents = vec![];

        cfg.validate()
            .expect("phase without agents should be allowed for AI-optional templates");
    }

    #[test]
    fn validate_rejects_autotune_threshold_order() {
        let mut cfg = valid_config();
        cfg.autotune = Some(crate::core::config::autotune::AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.8,
            high_precision_threshold: 0.5,
            state_path: "state.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 3,
            summary_trigger_max: 20,
        });

        let err = cfg
            .validate()
            .expect_err("invalid autotune threshold order must fail");
        assert!(
            err.to_string().contains("error.autotune_min_le_max"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_zero_runtime_maintenance_interval() {
        let mut cfg = valid_config();
        cfg.runtime = Some(RuntimeConfig {
            maintenance_interval_seconds: 0,
            health_interval_seconds: 30,
            shutdown_drain_seconds: 10,
            protocol_mode: None,
            platform_mode: Some("phase_compat".to_string()),
            pua_report: false,
            deployment_target: None,
            acp_http_bind_addr: None,
            entry_auth_enabled: false,
            entry_auth_api_key_env: "GO_ON_ENTRY_API_KEY".to_string(),
            entry_rate_limit_rpm: 240,
            entry_rate_limit_burst: 60,
            production_strict: false,
            otel_enabled: false,
            otel_exporter: "otlp".to_string(),
            otel_endpoint: None,
            otel_service_name: "go-on".to_string(),
            otel_sample_ratio: 1.0,
            trace_slow_top_n: 20,
            skills_enabled: true,
            skills_import_enabled: false,
            skills_allowed_sources: Vec::new(),
            skills_require_sha256: true,
            skills_allow_floating_ref: false,
            skills_cache_dir: "skills_cache".to_string(),
            cors_allowed_origins: Vec::new(),
            user_auth_enabled: false,
            user_auth_token_secret: String::new(),
            user_auth_token_secret_env: "GO_ON_USER_AUTH_TOKEN_SECRET".to_string(),
            user_auth_token_ttl_seconds: 86400,
            tenant_default_daily_token_limit: 1_000_000,
            tenant_default_concurrent_tasks: 10,
            tenant_default_daily_api_calls: 10_000,
            i18n_default_language: "en".to_string(),
            enable_dag_execution: false,
            enable_agent_reroute: true,
            enable_metacognitive_feedback: true,
            enable_delphi_debate: false,
            governance_enabled: true,
            governance_policy_mode: String::new(),
            request_signing_enabled: false,
            request_signing_public_key: String::new(),
            request_signing_hmac_secret: String::new(),
            mtls_enabled: false,
            mtls_ca_cert_path: String::new(),
            mtls_server_cert_path: String::new(),
            mtls_server_key_path: String::new(),
            mtls_require_client_cert: false,
            mtls_allowed_cns: String::new(),
        });
        let err = cfg
            .validate()
            .expect_err("0 maintenance interval must fail");
        assert!(
            err.to_string().contains("error.runtime_must_be_positive"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_autotune_summary_range() {
        let mut cfg = valid_config();
        cfg.autotune = Some(crate::core::config::autotune::AutoTuneConfig {
            enabled: true,
            evaluate_interval: 20,
            min_query_chars_step: 20,
            min_query_chars_min: 40,
            min_query_chars_max: 300,
            max_top_k: 4,
            low_precision_threshold: 0.35,
            high_precision_threshold: 0.75,
            state_path: "state.json".to_string(),
            cooldown_windows: 2,
            min_vector_searches: 5,
            summary_trigger_min: 9,
            summary_trigger_max: 6,
        });

        let err = cfg
            .validate()
            .expect_err("invalid autotune summary range must fail");
        assert!(
            err.to_string().contains("error.autotune_min_le_max"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_complex_autopilot_without_two_reviewers() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            autopilot_complexity: Some("complex".to_string()),
            full_auto_review_agents: Some(vec!["reviewer_a".to_string()]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("complex autopilot with one reviewer must fail");
        assert!(
            err.to_string()
                .contains("error.complex_autopilot_min_review_agents"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_complex_autopilot_when_reviewer_not_in_review_phase() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("review")
            .expect("review phase must exist")
            .agents = vec!["reviewer_a".to_string(), "copilot".to_string()];
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            autopilot_complexity: Some("complex".to_string()),
            full_auto_review_agents: Some(vec!["reviewer_a".to_string(), "reviewer_b".to_string()]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("missing reviewer in review phase must fail");
        assert!(
            err.to_string()
                .contains("error.review_agent_must_be_in_phases"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_zero_phase_timeout() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            request_timeout_seconds: Some(0),
            ..PhaseOptions::default()
        });

        let err = cfg.validate().expect_err("zero request timeout must fail");
        assert!(
            err.to_string().contains("error.phase_field_positive"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_required_approvals_exceeding_min_reviewers() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            extra: HashMap::from([
                ("min_reviewers".to_string(), serde_json::Value::from(1_u64)),
                (
                    "required_approvals".to_string(),
                    serde_json::Value::from(2_u64),
                ),
            ]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("required approvals above min reviewers must fail");
        assert!(
            err.to_string()
                .contains("error.phase_option_must_be_number"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_min_reviewers_above_two() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            extra: HashMap::from([("min_reviewers".to_string(), serde_json::Value::from(3_u64))]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("min_reviewers above two must fail");
        assert!(
            err.to_string()
                .contains("error.phase_option_must_be_number"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_complex_autopilot_with_more_than_two_reviewers() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("review")
            .expect("review phase must exist")
            .agents = vec![
            "reviewer_a".to_string(),
            "reviewer_b".to_string(),
            "copilot".to_string(),
        ];
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            autopilot_complexity: Some("complex".to_string()),
            full_auto_review_agents: Some(vec![
                "reviewer_a".to_string(),
                "reviewer_b".to_string(),
                "copilot".to_string(),
            ]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("complex autopilot with >2 reviewers must fail");
        assert!(
            err.to_string()
                .contains("error.complex_autopilot_max_review_agents"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_invalid_rate_limit_type() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            extra: HashMap::from([(
                "rate_limit_rpm".to_string(),
                serde_json::Value::from("fast"),
            )]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("non-numeric rate_limit_rpm must fail");
        assert!(
            err.to_string()
                .contains("error.phase_option_must_be_number"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_invalid_burst_multiplier_range() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            extra: HashMap::from([(
                "rate_limit_burst_multiplier".to_string(),
                serde_json::Value::from(100.0_f64),
            )]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("burst multiplier out of range must fail");
        assert!(
            err.to_string()
                .contains("error.phase_option_must_be_number"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_zero_breaker_open_seconds() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            extra: HashMap::from([(
                "circuit_breaker_open_seconds".to_string(),
                serde_json::Value::from(0_u64),
            )]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("zero breaker open seconds must fail");
        assert!(
            err.to_string()
                .contains("error.phase_option_must_be_number"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_invalid_review_timeout_policy() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            extra: HashMap::from([(
                "review_timeout_policy".to_string(),
                serde_json::Value::from("maybe"),
            )]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("invalid review timeout policy must fail");
        assert!(
            err.to_string().contains("error.phase_option_must_be_bool"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_non_boolean_auto_attach() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            extra: HashMap::from([("auto_attach".to_string(), serde_json::Value::from("yes"))]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("non-boolean auto_attach must fail");
        assert!(
            err.to_string().contains("error.phase_option_must_be_bool"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_rejects_unsupported_optimization_module() {
        let mut cfg = valid_config();
        cfg.phases
            .get_mut("coding")
            .expect("coding phase must exist")
            .options = Some(PhaseOptions {
            extra: HashMap::from([(
                "optimization_modules".to_string(),
                serde_json::Value::from(vec!["unknown_module"]),
            )]),
            ..PhaseOptions::default()
        });

        let err = cfg
            .validate()
            .expect_err("unsupported optimization module must fail");
        assert!(
            err.to_string()
                .contains("error.phase_option_must_be_number"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn missing_env_vars_detects_agent_requirements() {
        let cfg = valid_config();
        let missing = missing_env_vars(&cfg);

        assert!(missing.iter().any(|value| value == "ANTHROPIC_API_KEY"));
        assert!(missing.iter().any(|value| value == "WENXIN_API_KEY"));
        assert!(missing.iter().any(|value| value == "WENXIN_SECRET_KEY"));
    }

    #[test]
    fn runtime_readiness_allows_when_at_least_one_agent_ready() {
        let cfg = valid_config();
        let dir = tempdir().expect("tempdir should be created");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "# test").expect("config marker should be written");

        validate_runtime_readiness(&config_path, &cfg)
            .expect("runtime readiness should pass when at least one agent is env-ready");
    }

    #[test]
    fn runtime_readiness_allows_degraded_when_all_agents_are_env_blocked() {
        let mut cfg = valid_config();
        cfg.provider.agents.remove("copilot");
        cfg.phases
            .get_mut("coding")
            .expect("coding phase should exist")
            .agents = vec!["reviewer_a".to_string()];
        if let Some(agent) = cfg.provider.agents.get_mut("reviewer_a") {
            agent.api_key_env = Some("UNITTEST_MISSING_REVIEWER_A_KEY".to_string());
        }
        if let Some(agent) = cfg.provider.agents.get_mut("reviewer_b") {
            agent.api_key_env = Some("UNITTEST_MISSING_REVIEWER_B_KEY".to_string());
            agent.secret_key_env = Some("UNITTEST_MISSING_REVIEWER_B_SECRET".to_string());
        }

        let dir = tempdir().expect("tempdir should be created");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "# test").expect("config marker should be written");

        validate_runtime_readiness(&config_path, &cfg)
            .expect("runtime readiness should allow degraded startup in non-strict mode");
    }

    #[test]
    fn runtime_readiness_strict_mode_fails_when_agent_secrets_missing() {
        let mut cfg = valid_config();
        cfg.runtime = Some(RuntimeConfig {
            production_strict: true,
            ..RuntimeConfig::default()
        });

        let dir = tempdir().expect("tempdir should be created");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "# test").expect("config marker should be written");

        let err = validate_runtime_readiness(&config_path, &cfg)
            .expect_err("strict mode should fail when any configured agent is missing secrets");
        assert!(
            err.to_string().contains("error.missing_field"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn runtime_readiness_strict_mode_fails_when_entry_auth_disabled_for_http_bind() {
        let mut cfg = valid_config();
        if let Some(agent) = cfg.provider.agents.get_mut("copilot") {
            agent.url = None;
        }
        if let Some(agent) = cfg.provider.agents.get_mut("reviewer_a") {
            agent.api_key_env = None;
        }
        if let Some(agent) = cfg.provider.agents.get_mut("reviewer_b") {
            agent.api_key_env = None;
            agent.secret_key_env = None;
        }
        cfg.runtime = Some(RuntimeConfig {
            production_strict: true,
            acp_http_bind_addr: Some("127.0.0.1:8090".to_string()),
            entry_auth_enabled: false,
            ..RuntimeConfig::default()
        });

        let dir = tempdir().expect("tempdir should be created");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "# test").expect("config marker should be written");

        let err = validate_runtime_readiness(&config_path, &cfg).expect_err(
            "strict mode should fail when entry auth is disabled for exposed HTTP endpoint",
        );
        assert!(
            err.to_string().contains("error.missing_field"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn runtime_readiness_strict_mode_passes_with_safe_configuration() {
        let mut cfg = valid_config();
        if let Some(agent) = cfg.provider.agents.get_mut("copilot") {
            agent.url = None;
        }
        if let Some(agent) = cfg.provider.agents.get_mut("reviewer_a") {
            agent.api_key_env = None;
        }
        if let Some(agent) = cfg.provider.agents.get_mut("reviewer_b") {
            agent.api_key_env = None;
            agent.secret_key_env = None;
        }
        cfg.runtime = Some(RuntimeConfig {
            production_strict: true,
            entry_auth_enabled: true,
            ..RuntimeConfig::default()
        });

        let dir = tempdir().expect("tempdir should be created");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "# test").expect("config marker should be written");

        validate_runtime_readiness(&config_path, &cfg)
            .expect("strict mode should pass when all strict checks are satisfied");
    }

    #[test]
    fn adaptive_template_loads_and_validates() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config/config.toml");
        let cfg = AppConfig::load(&path).expect("config.toml should parse");

        cfg.validate()
            .expect("config.toml should be internally consistent");
    }

    #[test]
    fn build_config_health_report_recommends_minimal_on_clean_config() {
        let dir = tempdir().expect("tempdir should be created");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "# test").expect("config marker should be written");

        let cfg = valid_config();
        let report = build_config_health_report(&config_path, &cfg);

        assert_eq!(report.total, 1);
        assert_eq!(report.info_count, 0);
        assert_eq!(report.warn_count, 1);
        assert_eq!(report.profile_recommendation, "balanced");
        assert!(!report.recommendations.is_empty());
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.code == "PRODUCTION_STRICT_RECOMMENDED"));
    }

    #[test]
    fn build_config_health_report_flags_suspicious_combo_and_recommendations() {
        let dir = tempdir().expect("tempdir should be created");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "# test").expect("config marker should be written");

        let mut cfg = valid_config();
        cfg.cache = Some(CacheConfig {
            enabled: false,
            path: "cache.sqlite3".to_string(),
            default_ttl_seconds: 30,
            max_entries: 20_000,
            connection_string: None,
            read_replica_connection_string: None,
            persist_enabled: true,
        });
        cfg.vector = Some(VectorConfig {
            enabled: false,
            auto_mode: true,
            path: "vector.sqlite3".to_string(),
            connection_string: None,
            dimensions: 192,
            min_query_chars: 80,
            top_k: 2,
            min_similarity: 0.82,
            max_snippet_chars: 800,
            max_entries: 10_000,
            summary_enabled: true,
            summary_trigger_messages: 8,
            summary_max_chars: 1200,
            read_replica_connection_string: None,
        });
        cfg.runtime = Some(RuntimeConfig {
            maintenance_interval_seconds: 20,
            health_interval_seconds: 120,
            shutdown_drain_seconds: 30,
            protocol_mode: None,
            platform_mode: Some("phase_compat".to_string()),
            pua_report: false,
            deployment_target: None,
            acp_http_bind_addr: None,
            entry_auth_enabled: false,
            entry_auth_api_key_env: "GO_ON_ENTRY_API_KEY".to_string(),
            entry_rate_limit_rpm: 240,
            entry_rate_limit_burst: 60,
            production_strict: false,
            otel_enabled: true,
            otel_exporter: "otlp".to_string(),
            otel_endpoint: None,
            otel_service_name: "go-on".to_string(),
            otel_sample_ratio: 1.0,
            trace_slow_top_n: 20,
            skills_enabled: true,
            skills_import_enabled: false,
            skills_allowed_sources: Vec::new(),
            skills_require_sha256: true,
            skills_allow_floating_ref: false,
            skills_cache_dir: "skills_cache".to_string(),
            cors_allowed_origins: Vec::new(),
            user_auth_enabled: false,
            user_auth_token_secret: String::new(),
            user_auth_token_secret_env: "GO_ON_USER_AUTH_TOKEN_SECRET".to_string(),
            user_auth_token_ttl_seconds: 86400,
            tenant_default_daily_token_limit: 1_000_000,
            tenant_default_concurrent_tasks: 10,
            tenant_default_daily_api_calls: 10_000,
            i18n_default_language: "en".to_string(),
            enable_dag_execution: false,
            enable_agent_reroute: true,
            enable_metacognitive_feedback: true,
            enable_delphi_debate: false,
            governance_enabled: true,
            governance_policy_mode: String::new(),
            request_signing_enabled: false,
            request_signing_public_key: String::new(),
            request_signing_hmac_secret: String::new(),
            mtls_enabled: false,
            mtls_ca_cert_path: String::new(),
            mtls_server_cert_path: String::new(),
            mtls_server_key_path: String::new(),
            mtls_require_client_cert: false,
            mtls_allowed_cns: String::new(),
        });

        let report = build_config_health_report(&config_path, &cfg);
        let codes = report
            .warnings
            .iter()
            .map(|w| w.code.clone())
            .collect::<Vec<_>>();

        assert!(codes.iter().any(|code| code == "MEMORY_LAYERS_DISABLED"));
        assert!(codes
            .iter()
            .any(|code| code == "RUNTIME_OBSERVABILITY_OVERHEAD_RISK"));
        assert!(codes
            .iter()
            .any(|code| code == "PRODUCTION_STRICT_RECOMMENDED"));
        assert_eq!(report.warn_count, 3);
        assert_eq!(report.profile_recommendation, "full");
        assert!(report
            .recommendations
            .iter()
            .any(|text| text.contains("enable either cache or vector memory")));
    }

    #[test]
    fn load_auto_rules_from_rules_directory_and_phase_files() {
        let dir = tempdir().expect("tempdir should be created");
        let config_path = dir.path().join("config.toml");
        let rules_dir = dir.path().join("RULES");
        fs::create_dir_all(&rules_dir).expect("rules directory should be created");

        fs::write(
            &config_path,
            r#"default_phase = "coding"

[flow]
name = "test"
phases = ["coding", "review"]

[agents.copilot]
type = "copilot"
url = "http://127.0.0.1:8080"

[phases.coding]
description = "coding"
agents = ["copilot"]
fallback = true
principles = ["inline principle"]

[phases.review]
description = "review"
agents = ["copilot"]
fallback = true
"#,
        )
        .expect("config should be written");

        fs::write(
            dir.path().join("RULES.md"),
            "# Shared\n- shared one\n- shared two\n",
        )
        .expect("shared rules should be written");
        fs::write(
            rules_dir.join("coding.md"),
            "## Coding\n1. coding phase rule\n* extra coding rule\n",
        )
        .expect("phase rules should be written");

        let cfg = AppConfig::load(&config_path).expect("config should load");
        let coding = cfg
            .phases
            .get("coding")
            .and_then(|phase| phase.principles.as_ref())
            .expect("coding principles should exist");
        assert!(coding.iter().any(|v| v == "inline principle"));
        assert!(coding.iter().any(|v| v == "shared one"));
        assert!(coding.iter().any(|v| v == "shared two"));
        assert!(coding.iter().any(|v| v == "coding phase rule"));
        assert!(coding.iter().any(|v| v == "extra coding rule"));

        let review = cfg
            .phases
            .get("review")
            .and_then(|phase| phase.principles.as_ref())
            .expect("review principles should exist");
        assert!(review.iter().any(|v| v == "shared one"));
        assert!(review.iter().any(|v| v == "shared two"));
    }

    #[test]
    fn load_auto_rules_from_sidecar_phase_file() {
        let dir = tempdir().expect("tempdir should be created");
        let config_path = dir.path().join("config.toml");

        fs::write(
            &config_path,
            r#"default_phase = "coding"

[flow]
name = "test"
phases = ["coding"]

[agents.copilot]
type = "copilot"
url = "http://127.0.0.1:8080"

[phases.coding]
description = "coding"
agents = ["copilot"]
fallback = true
"#,
        )
        .expect("config should be written");

        fs::write(
            dir.path().join("coding.rules.md"),
            "- keep functions short\n- add tests\n",
        )
        .expect("sidecar rules should be written");

        let cfg = AppConfig::load(&config_path).expect("config should load");
        let coding = cfg
            .phases
            .get("coding")
            .and_then(|phase| phase.principles.as_ref())
            .expect("coding principles should exist");

        assert!(coding.iter().any(|v| v == "keep functions short"));
        assert!(coding.iter().any(|v| v == "add tests"));
    }
}
