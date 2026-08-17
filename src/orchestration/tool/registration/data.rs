//! Archive, SQLite-query, data-serialization, compression, RSS, JSONL and
//! web-search tools.

use crate::orchestration::tool::{RetryPolicy, ToolCapabilityProfile, ToolRegistry, ToolRiskLevel};

pub(crate) fn register_data(registry: &mut ToolRegistry) {
    // ── Archive tools (no feature gate) ──────────────────
    registry.register_with_profile(
        crate::orchestration::tool_extended::ArchiveInspectTool,
        ToolCapabilityProfile {
            capability: "archive_inspect".to_string(),
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
        crate::orchestration::tool_extended::ArchiveExtractTool,
        ToolCapabilityProfile {
            capability: "archive_extract".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 60_000,
            retry_policy: RetryPolicy {
                max_retries: 0,
                retry_on_failure: false,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── SQLite query tools (feature-gated) ───────────────
    #[cfg(feature = "backend-sqlite")]
    registry.register_with_profile(
        crate::orchestration::tool_extended::SqliteQueryTool,
        ToolCapabilityProfile {
            capability: "sqlite_query".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 60_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── Data serialization tools (feature-gated) ─────────
    #[cfg(feature = "data-export")]
    registry.register_with_profile(
        crate::orchestration::tool_extended::CsvReadTool,
        ToolCapabilityProfile {
            capability: "csv_read".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 30_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );
    #[cfg(feature = "data-export")]
    registry.register_with_profile(
        crate::orchestration::tool_extended::CsvWriteTool,
        ToolCapabilityProfile {
            capability: "csv_write".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 30_000,
            retry_policy: RetryPolicy {
                max_retries: 0,
                retry_on_failure: false,
            },
            fallback_chain: Vec::new(),
        },
    );
    #[cfg(feature = "data-export")]
    registry.register_with_profile(
        crate::orchestration::tool_extended::CsvAnalyzeTool,
        ToolCapabilityProfile {
            capability: "csv_analyze".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 30_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );
    #[cfg(feature = "data-export")]
    registry.register_with_profile(
        crate::orchestration::tool_extended::CsvTransformTool,
        ToolCapabilityProfile {
            capability: "csv_transform".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 30_000,
            retry_policy: RetryPolicy {
                max_retries: 0,
                retry_on_failure: false,
            },
            fallback_chain: Vec::new(),
        },
    );
    #[cfg(feature = "data-export")]
    registry.register_with_profile(
        crate::orchestration::tool_extended::TomlReadTool,
        ToolCapabilityProfile {
            capability: "toml_read".to_string(),
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
        crate::orchestration::tool_extended::TomlWriteTool,
        ToolCapabilityProfile {
            capability: "toml_write".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 10_000,
            retry_policy: RetryPolicy {
                max_retries: 0,
                retry_on_failure: false,
            },
            fallback_chain: Vec::new(),
        },
    );
    #[cfg(feature = "data-export")]
    registry.register_with_profile(
        crate::orchestration::tool_extended::YamlReadTool,
        ToolCapabilityProfile {
            capability: "yaml_read".to_string(),
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
        crate::orchestration::tool_extended::YamlWriteTool,
        ToolCapabilityProfile {
            capability: "yaml_write".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 10_000,
            retry_policy: RetryPolicy {
                max_retries: 0,
                retry_on_failure: false,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── Compression tools (no feature gate) ───────────────
    registry.register_with_profile(
        crate::orchestration::tool_extended::CompressTool,
        ToolCapabilityProfile {
            capability: "compress".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 60_000,
            retry_policy: RetryPolicy {
                max_retries: 0,
                retry_on_failure: false,
            },
            fallback_chain: Vec::new(),
        },
    );
    registry.register_with_profile(
        crate::orchestration::tool_extended::DecompressTool,
        ToolCapabilityProfile {
            capability: "decompress".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 60_000,
            retry_policy: RetryPolicy {
                max_retries: 0,
                retry_on_failure: false,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── RSS feed reader tool (no feature gate) ────────────────
    registry.register_with_profile(
        crate::orchestration::tool_extended::RssReadTool,
        ToolCapabilityProfile {
            capability: "rss_read".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 30_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── 3D model (OBJ) reader tool — unified with cad-obj's ObjReadTool ─
    registry.register_with_profile(
        crate::orchestration::tool_extended::JsonlReadTool,
        ToolCapabilityProfile {
            capability: "jsonl_read".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 30_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── JSON Lines write tool (no feature gate) ───────────────────
    registry.register_with_profile(
        crate::orchestration::tool_extended::JsonlWriteTool,
        ToolCapabilityProfile {
            capability: "jsonl_write".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 30_000,
            retry_policy: RetryPolicy {
                max_retries: 0,
                retry_on_failure: false,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── Web search tool ─────────────────────────────────────────--
    registry.register_with_profile(
        crate::orchestration::tool_extended::WebSearchTool,
        ToolCapabilityProfile {
            capability: "web_search".to_string(),
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
