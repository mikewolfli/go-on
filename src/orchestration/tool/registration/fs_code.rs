//! Filesystem, shell, git, code-intelligence and code-workflow tools.

use crate::orchestration::tool::{RetryPolicy, ToolCapabilityProfile, ToolRegistry, ToolRiskLevel};

pub(crate) fn register_fs_code(registry: &mut ToolRegistry) {
    // ── Extended tools ───────────────────────────────────────────
    registry.register_with_profile(
        crate::orchestration::tool_extended::ShellExecTool,
        ToolCapabilityProfile {
            capability: "shell_execution".to_string(),
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
        crate::orchestration::tool_extended::HttpRequestTool,
        ToolCapabilityProfile {
            capability: "http_request".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 30_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );
    registry.register_with_profile(
        crate::orchestration::tool_extended::GrepTool,
        ToolCapabilityProfile {
            capability: "content_search".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 15_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: vec!["search_files".to_string()],
        },
    );

    registry.register_with_profile(
        crate::orchestration::tool_extended::GitTool,
        ToolCapabilityProfile {
            capability: "version_control".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 15_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: vec!["inspect_git_diff".to_string()],
        },
    );
    registry.register_with_profile(
        crate::orchestration::tool_extended::ListDirectoryTool,
        ToolCapabilityProfile {
            capability: "directory_listing".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 5_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );
    registry.register_with_profile(
        crate::orchestration::tool_extended::CargoCheckTool,
        ToolCapabilityProfile {
            capability: "compilation_check".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 120_000,
            retry_policy: RetryPolicy {
                max_retries: 0,
                retry_on_failure: false,
            },
            fallback_chain: Vec::new(),
        },
    );
    // cargo_test is an alias for run_tests (registered via register_alias below).
    // The canonical tool RunTestsTool is registered at line ~134 above.
    registry.register_with_profile(
        crate::orchestration::tool_extended::FileMoveTool,
        ToolCapabilityProfile {
            capability: "filesystem_move".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 10_000,
            retry_policy: RetryPolicy {
                max_retries: 0,
                retry_on_failure: false,
            },
            fallback_chain: Vec::new(),
        },
    );
    registry.register_with_profile(
        crate::orchestration::tool_extended::FileDeleteTool,
        ToolCapabilityProfile {
            capability: "filesystem_delete".to_string(),
            risk_level: ToolRiskLevel::High,
            timeout_budget_ms: 10_000,
            retry_policy: RetryPolicy {
                max_retries: 0,
                retry_on_failure: false,
            },
            fallback_chain: Vec::new(),
        },
    );
    registry.register_with_profile(
        crate::orchestration::tool_extended::CreateDirectoryTool,
        ToolCapabilityProfile {
            capability: "filesystem_create_directory".to_string(),
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
        crate::orchestration::tool_extended::CopyPathTool,
        ToolCapabilityProfile {
            capability: "filesystem_copy".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 30_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: vec!["move_path".to_string()],
        },
    );

    registry.register_with_profile(
        crate::orchestration::tool_extended::EditFileTool,
        ToolCapabilityProfile {
            capability: "filesystem_edit".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 15_000,
            retry_policy: RetryPolicy {
                max_retries: 0,
                retry_on_failure: false,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── Code index / semantic search tool ───────────────────
    // Provides workspace-wide code symbol indexing and ranked semantic
    // search across multiple programming languages.
    registry.register_with_profile(
        crate::orchestration::tool_extended::CodeIndexTool,
        ToolCapabilityProfile {
            capability: "code_index_search".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 300_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: vec!["grep".to_string()],
        },
    );

    // ── File diff tool ───────────────────────────────────────
    registry.register_with_profile(
        crate::orchestration::tool_extended::DiffTool,
        ToolCapabilityProfile {
            capability: "file_diff".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 15_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── LSP-like code intelligence tools ─────────────────────
    registry.register_with_profile(
        crate::orchestration::tool_extended::GoToDefinitionTool,
        ToolCapabilityProfile {
            capability: "go_to_definition".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 30_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: vec!["grep".to_string()],
        },
    );
    registry.register_with_profile(
        crate::orchestration::tool_extended::FindReferencesTool,
        ToolCapabilityProfile {
            capability: "find_references".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 30_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: vec!["grep".to_string()],
        },
    );
    registry.register_with_profile(
        crate::orchestration::tool_extended::ApplyCodeActionTool,
        ToolCapabilityProfile {
            capability: "apply_code_action".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 60_000,
            retry_policy: RetryPolicy {
                max_retries: 0,
                retry_on_failure: false,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── Read file lines tool ────────────────────────────────
    registry.register_with_profile(
        crate::orchestration::tool_extended::ReadFileLinesTool,
        ToolCapabilityProfile {
            capability: "file_read_lines".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 10_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: vec!["read_file".to_string()],
        },
    );

    // ── Spawn sub-agent tool ───────────────────────────────
    registry.register_with_profile(
        crate::orchestration::tool_extended::SpawnAgentTool,
        ToolCapabilityProfile {
            capability: "agent_spawn".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 300_000,
            retry_policy: RetryPolicy {
                max_retries: 0,
                retry_on_failure: false,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── Code format tool ───────────────────────────────────
    registry.register_with_profile(
        crate::orchestration::tool_extended::FormatCodeTool,
        ToolCapabilityProfile {
            capability: "format_code".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 30_000,
            retry_policy: RetryPolicy {
                max_retries: 0,
                retry_on_failure: false,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── Package search tool ─────────────────────────────────
    registry.register_with_profile(
        crate::orchestration::tool_extended::SearchPackagesTool,
        ToolCapabilityProfile {
            capability: "package_search".to_string(),
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
