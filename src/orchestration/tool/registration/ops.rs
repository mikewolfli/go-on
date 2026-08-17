//! Network, date/time, diagnostics, environment, game, utility, build, query,
//! template, metrics, security, docker, file-watch and tool-search tools.

use crate::orchestration::tool::{RetryPolicy, ToolCapabilityProfile, ToolRegistry, ToolRiskLevel};

pub(crate) fn register_ops(registry: &mut ToolRegistry) {
    // ── Network tools ─────────────────────────────────────────--
    registry.register_with_profile(
        crate::orchestration::tool_extended::DnsLookupTool,
        ToolCapabilityProfile {
            capability: "dns_lookup".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 10_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );
    registry.register_with_profile(
        crate::orchestration::tool_extended::PingTool,
        ToolCapabilityProfile {
            capability: "network_ping".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 30_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );
    registry.register_with_profile(
        crate::orchestration::tool_extended::PortScanTool,
        ToolCapabilityProfile {
            capability: "port_scan".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 300_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: false,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── Date/Time tools ─────────────────────────────────────────--
    registry.register_with_profile(
        crate::orchestration::tool_extended::DateTimeTool,
        ToolCapabilityProfile {
            capability: "date_time".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 5_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── Diagnostics tool (no feature gate) ──────────────────────────
    registry.register_with_profile(
        crate::orchestration::tool_extended::DiagnosticsTool,
        ToolCapabilityProfile {
            capability: "project_diagnostics".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 300_000,
            retry_policy: RetryPolicy {
                max_retries: 0,
                retry_on_failure: false,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── Environment info tool (no feature gate) ─────────────────────
    registry.register_with_profile(
        crate::orchestration::tool_extended::EnvironmentInfoTool,
        ToolCapabilityProfile {
            capability: "environment_discovery".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 10_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── Game tools (feature-gated) ───────────────────────────────
    #[cfg(any(
        feature = "game-online",
        feature = "game-process",
        feature = "game-screen",
        feature = "game-input",
        feature = "game-agent",
        feature = "game-state",
        feature = "game-modding"
    ))]
    crate::orchestration::tool::extended::game::register_game_tools(registry);

    // ── Utility tools (uuid, random_token, encode_decode, hash_file) ─
    registry.register_with_profile(
        crate::orchestration::tool_extended::UuidGenTool,
        ToolCapabilityProfile {
            capability: "uuid_gen".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 10_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );
    registry.register_with_profile(
        crate::orchestration::tool_extended::RandomTokenTool,
        ToolCapabilityProfile {
            capability: "random_token".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 10_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );
    registry.register_with_profile(
        crate::orchestration::tool_extended::EncodeDecodeTool,
        ToolCapabilityProfile {
            capability: "encode_decode".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 10_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );
    registry.register_with_profile(
        crate::orchestration::tool_extended::HashFileTool,
        ToolCapabilityProfile {
            capability: "hash_file".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 10_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── Build/lint/dependency tools (P1) ───────────────────
    registry.register_with_profile(
        crate::orchestration::tool_extended::RunBuildTool,
        ToolCapabilityProfile {
            capability: "build_run".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 300_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );
    registry.register_with_profile(
        crate::orchestration::tool_extended::LintCodeTool,
        ToolCapabilityProfile {
            capability: "lint_run".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 120_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );
    registry.register_with_profile(
        crate::orchestration::tool_extended::AddDependencyTool,
        ToolCapabilityProfile {
            capability: "dependency_add".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 30_000,
            retry_policy: RetryPolicy {
                max_retries: 0,
                retry_on_failure: false,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── Structured data query tools (P1) ────────────────────
    registry.register_with_profile(
        crate::orchestration::tool_extended::JsonQueryTool,
        ToolCapabilityProfile {
            capability: "json_query".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 10_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );
    #[cfg(feature = "data-export")]
    registry.register_with_profile(
        crate::orchestration::tool_extended::YamlQueryTool,
        ToolCapabilityProfile {
            capability: "yaml_query".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 10_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── Template rendering tool (P1) ────────────────────────
    registry.register_with_profile(
        crate::orchestration::tool_extended::TemplateRenderTool,
        ToolCapabilityProfile {
            capability: "template_render".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 10_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── Code metrics tool (P2) ──────────────────────────────
    registry.register_with_profile(
        crate::orchestration::tool_extended::CodeMetricsTool,
        ToolCapabilityProfile {
            capability: "code_metrics".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 60_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── Security scan tool (P2) ────────────────────────────
    registry.register_with_profile(
        crate::orchestration::tool_extended::SecurityScanTool,
        ToolCapabilityProfile {
            capability: "security_scan".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 120_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── Docker container tools (P2) ────────────────────────
    registry.register_with_profile(
        crate::orchestration::tool_extended::DockerPsTool,
        ToolCapabilityProfile {
            capability: "docker_ps".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 15_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );
    registry.register_with_profile(
        crate::orchestration::tool_extended::DockerExecTool,
        ToolCapabilityProfile {
            capability: "docker_exec".to_string(),
            risk_level: ToolRiskLevel::High,
            timeout_budget_ms: 60_000,
            retry_policy: RetryPolicy {
                max_retries: 0,
                retry_on_failure: false,
            },
            fallback_chain: Vec::new(),
        },
    );
    registry.register_with_profile(
        crate::orchestration::tool_extended::DockerLogsTool,
        ToolCapabilityProfile {
            capability: "docker_logs".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 15_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );
    registry.register_with_profile(
        crate::orchestration::tool_extended::DockerBuildTool,
        ToolCapabilityProfile {
            capability: "docker_build".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 300_000,
            retry_policy: RetryPolicy {
                max_retries: 0,
                retry_on_failure: false,
            },
            fallback_chain: Vec::new(),
        },
    );
    registry.register_with_profile(
        crate::orchestration::tool_extended::DockerPushTool,
        ToolCapabilityProfile {
            capability: "docker_push".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 300_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );
    registry.register_with_profile(
        crate::orchestration::tool_extended::DockerComposeTool,
        ToolCapabilityProfile {
            capability: "docker_compose".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 300_000,
            retry_policy: RetryPolicy {
                max_retries: 0,
                retry_on_failure: false,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── File watch tool (P2) ────────────────────────────────
    registry.register_with_profile(
        crate::orchestration::tool_extended::FileWatchTool,
        ToolCapabilityProfile {
            capability: "file_watch".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 30_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── Tool search / discovery tool ──────────────────────────
    // Direct-exposed so the model can discover Deferred tools.
    registry.register_with_profile(
        crate::orchestration::tool_extended::ToolSearchTool,
        ToolCapabilityProfile {
            capability: "tool_search".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 10_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── Cross-session memory search tool (SQLite/FTS5) ───────────
    #[cfg(feature = "backend-sqlite")]
    registry.register_with_profile(
        crate::orchestration::tool_extended::MemorySearchTool::new(),
        ToolCapabilityProfile {
            capability: "memory_search".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 30_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );
}
