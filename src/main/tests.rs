use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::json;

use super::*;
use crate::config::{
    AgentConfig, AppConfig, CacheConfig, FlowConfig, PhaseConfig, PhaseOptions, RuntimeConfig,
    VectorConfig,
};
use crate::reinforcement::{CheckStatus, ComponentReport, RuntimeHealthcheckReport};

fn openai_config_with_inflight(
    phase_max_inflight: Option<i64>,
    global_max_inflight: Option<i64>,
) -> AppConfig {
    let mut agents = HashMap::new();
    agents.insert(
        "primary".to_string(),
        AgentConfig {
            agent_type: "openai".to_string(),
            url: Some("https://api.openai.com/v1".to_string()),
            chat_path: None,
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            model: Some("gpt-4o-mini".to_string()),
            max_tokens: None,
            supports_system: Some(true),
            supports_vision: None,
        },
    );

    let mut coding_extra = HashMap::new();
    if let Some(value) = phase_max_inflight {
        coding_extra.insert("phase_max_inflight".to_string(), json!(value));
    }
    if let Some(value) = global_max_inflight {
        coding_extra.insert("global_max_inflight".to_string(), json!(value));
    }

    let mut phases = HashMap::new();
    phases.insert(
        "planning".to_string(),
        PhaseConfig {
            description: "planning".to_string(),
            agents: vec!["primary".to_string()],
            fallback: Some(true),
            principles: None,
            options: Some(PhaseOptions {
                request_timeout_seconds: Some(120),
                ..PhaseOptions::default()
            }),
        },
    );
    phases.insert(
        "coding".to_string(),
        PhaseConfig {
            description: "coding".to_string(),
            agents: vec!["primary".to_string()],
            fallback: Some(true),
            principles: None,
            options: Some(PhaseOptions {
                request_timeout_seconds: Some(150),
                review_timeout_seconds: Some(60),
                extra: coding_extra,
                ..PhaseOptions::default()
            }),
        },
    );
    phases.insert(
        "review".to_string(),
        PhaseConfig {
            description: "review".to_string(),
            agents: vec!["primary".to_string()],
            fallback: Some(true),
            principles: None,
            options: Some(PhaseOptions {
                request_timeout_seconds: Some(60),
                ..PhaseOptions::default()
            }),
        },
    );
    phases.insert(
        "delivery".to_string(),
        PhaseConfig {
            description: "delivery".to_string(),
            agents: vec!["primary".to_string()],
            fallback: Some(false),
            principles: None,
            options: Some(PhaseOptions {
                request_timeout_seconds: Some(90),
                ..PhaseOptions::default()
            }),
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
            phases: vec![
                "planning".to_string(),
                "coding".to_string(),
                "review".to_string(),
                "delivery".to_string(),
            ],
            workflow_type: crate::config::WorkflowType::Auto,
        },
        phases,
        runtime: Some(RuntimeConfig::default()),
        cache: Some(CacheConfig {
            enabled: true,
            path: "acp_cache.sqlite3".to_string(),
            default_ttl_seconds: 3600,
            max_entries: 5000,
            connection_string: None,
            read_replica_connection_string: None,
        }),
        vector: Some(VectorConfig {
            enabled: true,
            auto_mode: true,
            path: "acp_vector.sqlite3".to_string(),
            connection_string: None,
            dimensions: 192,
            min_query_chars: 80,
            top_k: 2,
            min_similarity: 0.82,
            max_snippet_chars: 800,
            max_entries: 10000,
            summary_enabled: true,
            summary_trigger_messages: 8,
            summary_max_chars: 1200,
            read_replica_connection_string: None,
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
        protocol: None,
    }
}

fn ready_report() -> RuntimeHealthcheckReport {
    RuntimeHealthcheckReport {
        generated_at: 0,
        overall_status: CheckStatus::Healthy,
        components: vec![ComponentReport {
            name: "provider_dependencies".to_string(),
            status: CheckStatus::Healthy,
            message: "ok".to_string(),
            details: json!({"ready": 1, "total": 1, "agents": []}),
        }],
    }
}

#[test]
fn completeness_reports_inflight_recommendation_mismatch() {
    let cfg = openai_config_with_inflight(Some(8), Some(32));
    let report = build_completeness_report(&cfg, &ready_report());

    assert!(report.recommended.iter().any(|item| item
        .message
        .contains("phases.coding.options.phase_max_inflight recommended=")));
    assert!(report
        .recommended
        .iter()
        .any(|item| item.level == RecommendationLevel::Warning));
    assert!(report.recommended.iter().any(|item| item
        .message
        .contains("phases.coding.options.global_max_inflight recommended=")));
}

#[test]
fn completeness_reports_missing_inflight_keys() {
    let cfg = openai_config_with_inflight(None, None);
    let report = build_completeness_report(&cfg, &ready_report());

    assert!(report
        .missing
        .iter()
        .any(|item| item == "phases.coding.options.phase_max_inflight"));
    assert!(report
        .missing
        .iter()
        .any(|item| item == "phases.coding.options.global_max_inflight"));
}

#[test]
fn cli_protocol_mode_overrides_config() {
    let mut runtime_config = RuntimeConfig {
        protocol_mode: Some("adaptive".to_string()),
        ..RuntimeConfig::default()
    };

    if let Some(mode) = validate_cli_protocol_mode(Some("mcp_http")).unwrap() {
        runtime_config.protocol_mode = Some(mode);
    }

    assert_eq!(runtime_config.protocol_mode.as_deref(), Some("mcp_http"));
}

#[test]
fn cli_protocol_mode_accepts_all_valid_values() {
    for mode in [
        "adaptive",
        "adap",
        "acp_stdio",
        "acp_http",
        "acp-http",
        "mcp_stdio",
        "mcp_http",
        "mcp-http",
        "auto",
        "acp",
        "mcp",
        "acp+http",
        "mcp+stdio",
    ] {
        assert!(
            validate_cli_protocol_mode(Some(mode)).is_ok(),
            "mode={mode}"
        );
    }
}

#[test]
fn cli_protocol_mode_rejects_invalid_value() {
    let err = validate_cli_protocol_mode(Some("invalid_mode")).unwrap_err();
    assert!(err
        .to_string()
        .contains("invalid --protocol-mode 'invalid_mode'"));
}

#[test]
fn cli_protocol_mode_rejects_ambiguous_prefix() {
    let err = validate_cli_protocol_mode(Some("acp_")).unwrap_err();
    assert!(err.to_string().contains("ambiguous --protocol-mode prefix"));
}

#[test]
fn preferred_config_root_prefers_xdg_on_unix() {
    let root = preferred_config_root("linux", |key| match key {
        "XDG_CONFIG_HOME" => Some(PathBuf::from("/tmp/xdg")),
        "HOME" => Some(PathBuf::from("/home/user")),
        _ => None,
    });
    assert_eq!(root, Some(PathBuf::from("/tmp/xdg")));
}

#[test]
fn preferred_config_root_falls_back_to_home_dot_config_on_unix() {
    let root = preferred_config_root("macos", |key| match key {
        "HOME" => Some(PathBuf::from("/Users/alice")),
        _ => None,
    });
    assert_eq!(root, Some(PathBuf::from("/Users/alice/.config")));
}

#[test]
fn preferred_config_root_prefers_appdata_on_windows() {
    let root = preferred_config_root("windows", |key| match key {
        "APPDATA" => Some(PathBuf::from("C:/Users/alice/AppData/Roaming")),
        "USERPROFILE" => Some(PathBuf::from("C:/Users/alice")),
        _ => None,
    });
    assert_eq!(root, Some(PathBuf::from("C:/Users/alice/AppData/Roaming")));
}

#[test]
fn preferred_config_root_falls_back_to_userprofile_on_windows() {
    let root = preferred_config_root("windows", |key| match key {
        "USERPROFILE" => Some(PathBuf::from("C:/Users/bob")),
        _ => None,
    });
    assert_eq!(root, Some(PathBuf::from("C:/Users/bob/AppData/Roaming")));
}
