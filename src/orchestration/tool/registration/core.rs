//! Core tools: the six built-in file/shell/verification tools, the four skill
//! tools, and the backward-compatibility alias block (kept last, matching the
//! original registration order in `ToolRegistry::new`).

use crate::orchestration::tool::builtin_tools::{
    ApplyPatchTool, InspectGitDiffTool, ReadFileTool, RunTestsTool, SearchFilesTool,
    SkillCreateTool, SkillExecuteTool, SkillListTool, SkillReloadTool, WriteFileTool,
};
use crate::orchestration::tool::{RetryPolicy, ToolCapabilityProfile, ToolRegistry, ToolRiskLevel};

pub(crate) fn register_core(registry: &mut ToolRegistry) {
    registry.register_with_profile(
        ReadFileTool,
        ToolCapabilityProfile {
            capability: "filesystem_read".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 10_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: vec!["search_files".to_string()],
        },
    );
    registry.register_with_profile(
        WriteFileTool,
        ToolCapabilityProfile {
            capability: "filesystem_write".to_string(),
            risk_level: ToolRiskLevel::High,
            timeout_budget_ms: 15_000,
            retry_policy: RetryPolicy {
                max_retries: 0,
                retry_on_failure: false,
            },
            fallback_chain: Vec::new(),
        },
    );
    registry.register_with_profile(
        SearchFilesTool,
        ToolCapabilityProfile {
            capability: "filesystem_search".to_string(),
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
        ApplyPatchTool,
        ToolCapabilityProfile {
            capability: "patch_apply".to_string(),
            risk_level: ToolRiskLevel::High,
            timeout_budget_ms: 20_000,
            retry_policy: RetryPolicy {
                max_retries: 0,
                retry_on_failure: false,
            },
            fallback_chain: vec!["inspect_git_diff".to_string()],
        },
    );
    registry.register_with_profile(
        RunTestsTool,
        ToolCapabilityProfile {
            capability: "verification_execute".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 300_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: vec!["inspect_git_diff".to_string()],
        },
    );
    registry.register_with_profile(
        InspectGitDiffTool,
        ToolCapabilityProfile {
            capability: "scm_diff".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 8_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── Skill listing tool (always compiled, no feature gate) ────
    registry.register_with_profile(
        SkillListTool,
        ToolCapabilityProfile {
            capability: "skill_list".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 5_000,
            retry_policy: RetryPolicy {
                max_retries: 0,
                retry_on_failure: false,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── Skill execution tool (always compiled, no feature gate) ────
    registry.register_with_profile(
        SkillExecuteTool,
        ToolCapabilityProfile {
            capability: "skill_execute".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 120_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── Skill creation tool (always compiled, no feature gate) ────
    registry.register_with_profile(
        SkillCreateTool,
        ToolCapabilityProfile {
            capability: "skill_create".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 30_000,
            retry_policy: RetryPolicy {
                max_retries: 0,
                retry_on_failure: false,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── Skill reload tool (always compiled, no feature gate) ────
    registry.register_with_profile(
        SkillReloadTool,
        ToolCapabilityProfile {
            capability: "skill_reload".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 30_000,
            retry_policy: RetryPolicy {
                max_retries: 0,
                retry_on_failure: false,
            },
            fallback_chain: Vec::new(),
        },
    );

    // ── Backward-compatibility aliases ───────────────────────
    // These names exist in the governance evaluator's allowlist.
    // Some now have their own Tool implementations; others alias
    // to existing tools with the same functionality.
    // create_directory and copy_path have dedicated implementations above.
    // file_move/file_delete are the old primary names; now they alias to the new canonical names.
    registry.register_alias("file_move", "move_path");
    registry.register_alias("file_delete", "delete_path");
    registry.register_alias("execute_command", "shell_exec");
    registry.register_alias("terminal", "shell_exec");
    registry.register_alias("bash", "shell_exec");
    registry.register_alias("find_path", "search_files");
    registry.register_alias("semantic_search", "code_index_search");
    registry.register_alias("cargo_test", "run_tests");
    registry.register_alias("find_files", "search_files");
}
