//! Centralized tool-name-to-capability registry.
//!
//! Unifies 4 previously independent tool-classification mappings into a single
//! source of truth so that new tools only need to be added once.
//!
//! # Design
//!
//! Each of the three classification dimensions is exposed as a static method on
//! `ToolCapabilityRegistry`:
//!
//! * `operation` — sandbox operation type  (Read / Write / Shell / Network / Search / Unknown)
//! * `action`    — governance action        (Read / Write / Shell / Search / Network)
//! * `risk_class` — default-policy risk class (ReadOnly / LowRiskWrite / HighRiskExecute / Admin)
//!
//! All methods use **explicit match arms** for tools that have been registered in
//! any of the four original mappings, plus a **keyword-based fallback** as a
//! safety net for tools that follow standard naming conventions but have not yet
//! been added to the explicit list.  Tools that match no keyword and no explicit
//! arm receive a safe default (Read / Read / LowRiskWrite respectively).

use crate::governance::hardening::GovernanceAction;
use serde::{Deserialize, Serialize};

/// The sandbox operation type for a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOperation {
    Read,
    Write,
    Shell,
    Network,
    Search,
    /// Not recognized — requires user review.
    Unknown,
}

/// The risk class for governance defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolRiskClass {
    ReadOnly,
    LowRiskWrite,
    HighRiskExecute,
    Admin,
}

/// Central registry of tool capabilities.
pub struct ToolCapabilityRegistry;

impl ToolCapabilityRegistry {
    // ── Sandbox operation ──────────────────────────────────────────────────
    //
    // Derived from evaluator.rs sandbox match and status.rs tool_category().

    /// Return the sandbox operation for a tool name.
    pub fn operation(tool: &str) -> ToolOperation {
        match tool {
            // ── Read / Query tools ──────────────────────────────────
            "read_file"
            | "read_file_lines"
            | "read"
            | "search_files"
            | "inspect_git_diff"
            | "list_directory"
            | "list_files"
            | "ls"
            | "date_time"
            | "skill_list"
            | "skill-finder"
            | "skill_reload"
            | "chat.execute"
            | "acp_trace_get"
            | "acp_debug_panel_get"
            | "goon_workflow_run_list"
            | "goon_workflow_run_get"
            | "goon_metrics_window_query"
            | "goon_metrics_errors_summary"
            | "goon_provider_capabilities"
            | "prompts_list"
            | "prompts_get"
            | "workflow_execute"
            | "workflow_ask"
            | "workflow_generate"
            | "import_skill"
            | "archive_inspect"
            | "jsonl_read"
            | "environment_info"
            | "echo_skill"
            | "builtin.echo"
            | "goon_skill_version_list"
            // ── Document readers ──────────────────────────────
            | "read_pdf"
            | "pdf_merge"
            | "pdf_split"
            | "read_docx"
            | "read_excel"
            | "read_ppt"
            | "email_parse"
            | "invoice_parse"
            | "web_scrape"
            // ── CAD / 3D readers ──────────────────────────────
            | "dxf_read"
            | "cad_convert"
            | "step_read"
            | "obj_read"
            | "obj_model_read"
            | "stl_read"
            | "gltf_read"
            | "iges_read"
            | "ply_read"
            | "geo_util"
            | "gcode_read"
            | "gpx_read"
            | "svg_read"
            // ── Image readers ────────────────────────────────
            | "image_analyze"
            // ── Data readers ──────────────────────────────────
            | "csv_read"
            | "csv_analyze"
            | "toml_read"
            | "yaml_read"
            | "rss_read"
            // ── Database ───────────────────────────────────────
            | "sqlite_query"
            // ── Game readers ───────────────────────────────────
            | "game_server_query"
            | "game_price_tracker"
            | "game_matchmaking"
            | "game_achievements"
            | "game_mod_list"
            | "game_coaching_assistant"
            // ── Compilation / diagnostics ─────────────────────
            | "cargo_check"
            | "diagnostics"
            // ── Code analysis ──────────────────────────────────
            | "code_metrics"
            | "encode_decode"
            | "file_diff"
            | "file_watch"
            | "hash_file"
            | "lint_run"
            | "random_token"
            | "search_packages"
            | "security_scan"
            | "template_render"
            | "uuid_gen"
            | "docker_logs"
            // ── Skill execution ──────────────────────────────────
            | "skill_execute" => ToolOperation::Read,

            // ── Search / Discovery tools ──────────────────────────
            | "grep" | "find_path" | "semantic_search" | "code_index_search" | "find_files"
            | "search" | "find" => ToolOperation::Search,

            // ── Network / Outbound tools ─────────────────────────
            "http_request"
            | "dns_lookup"
            | "ping"
            | "port_scan"
            | "git"
            | "github_search_skills"
            | "game_monitor"
            | "game_online_status"
            | "goon_provider_test_connection"
            | "goon_provider_test_completion" => ToolOperation::Network,

            // ── Shell / Execution tools ──────────────────────────
            "run_tests"
            | "execute_command"
            | "terminal"
            | "bash"
            | "cargo_test"
            | "shell_exec"
            | "run"
            | "build_run"
            | "docker_build"
            | "docker_compose"
            | "docker_exec"
            | "docker_push"
            | "spawn_agent"
            | "game_launch"
            | "game_keyboard_input"
            | "game_mouse_input"
            | "game_auto_grind" => ToolOperation::Shell,

            // ── Write / Admin tools ───────────────────────────────
            "write_file"
            | "write"
            | "create"
            | "apply_patch"
            | "apply_code_action"
            | "create_directory"
            | "dependency_add"
            | "delete_path"
            | "move_path"
            | "copy_path"
            | "file_move"
            | "file_delete"
            | "edit_file"
            | "format_code"
            | "compress"
            | "decompress"
            | "archive_extract"
            | "jsonl_write"
            | "csv_write"
            | "csv_transform"
            | "toml_write"
            | "yaml_write"
            | "write_docx"
            | "write_excel"
            | "write_ppt"
            | "svg_generate"
            | "svg_export"
            | "stl_generate"
            | "qrcode_generate"
            | "image_generate"
            | "image_resize"
            | "image_convert"
            | "skill_create"
            | "skill-creator"
            | "goon_skill_update"
            | "goon_skill_version_rollback"
            | "goon_workflow_run_cancel"
            | "goon_workflow_run_pause"
            | "goon_workflow_run_resume"
            | "game_screen_capture"
            | "game_replay_recorder"
            | "game_save_manager"
            | "game_mod_install"
            | "game_state_modify" => ToolOperation::Write,

            // ── Keyword-based fallback ─────────────────────────
            _ => classify_operation_by_keyword(tool),
        }
    }

    // ── Governance action (for permission checks) ──────────────────────────
    //
    // Derived from evaluator.rs check_permission() + tools_pack.rs
    // governance_action_for_tool().

    /// Return the governance action for a tool name.
    ///
    /// This is the canonical tool→action mapping, consolidating what was
    /// previously duplicated in `pipeline_tool_to_action`. All sandbox
    /// governance paths should use this single source of truth.
    pub fn action(tool: &str) -> GovernanceAction {
        match tool {
            // ── Read operations (read-only file/content access) ──
            "read_file" | "inspect_git_diff" | "list_directory" | "date_time"
            | "skill_list" | "archive_inspect" | "jsonl_read" | "diagnostics" | "environment_info"
            | "echo_skill" | "builtin.echo" | "goon_skill_version_list"
            | "skill-finder" | "chat.execute"
            | "acp_trace_get" | "acp_debug_panel_get"
            | "goon_workflow_run_list" | "goon_workflow_run_get"
            | "goon_metrics_window_query" | "goon_metrics_errors_summary"
            | "goon_provider_capabilities" | "prompts_list" | "prompts_get"
            | "workflow_execute" | "workflow_ask" | "workflow_generate"
            | "import_skill" | "skill_reload"
            // ── CAD read tools (read-only 3d/2d format parsing) ──
            | "dxf_read" | "stl_read" | "obj_read" | "step_read" | "ply_read" | "iges_read"
            | "gltf_read" | "svg_read" | "obj_model_read" | "gcode_read" | "gpx_read" | "geo_util"
            // ── Image read/analyze tools ──
            | "image_analyze"
            // ── Document read tools ──
            | "read_docx" | "read_excel" | "read_pdf" | "read_ppt"
            | "email_parse" | "csv_read" | "csv_analyze" | "toml_read" | "yaml_read"
            | "web_scrape" | "invoice_parse" | "rss_read" | "sqlite_query" => GovernanceAction::Read,

            // ── Search operations ──
            "grep" | "search_files" | "find_path" | "find_files" | "code_index_search" | "semantic_search"
            | "search" | "find" => GovernanceAction::Search,

            // ── Write operations (file creation/modification) ──
            "write_file"
            | "apply_patch"
            | "create_directory"
            | "delete_path"
            | "move_path"
            | "copy_path"
            | "file_move"
            | "file_delete"
            | "compress"
            | "decompress"
            | "archive_extract"
            | "jsonl_write"
            | "csv_write"
            | "csv_transform"
            | "toml_write"
            | "yaml_write"
            | "game_mod_install"
            | "game_replay_recorder"
            | "game_save_manager"
            | "game_screen_capture"
            | "goon_skill_update"
            | "goon_skill_version_rollback"
            | "goon_workflow_run_cancel"
            | "goon_workflow_run_pause"
            | "goon_workflow_run_resume"
            | "image_generate"
            | "image_resize"
            | "image_convert"
            | "skill-creator" | "skill_create"
            | "stl_generate"
            | "svg_export"
            | "svg_generate"
            | "qrcode_generate"
            | "write_docx"
            | "write_excel"
            | "write_ppt"
            | "pdf_merge" | "pdf_split"
            | "cad_convert"
            | "game_auto_grind"
            | "game_keyboard_input"
            | "game_mouse_input"
            | "game_state_modify"
            | "spawn_agent" => GovernanceAction::Write,

            // ── Shell operations (command/code execution) ──
            "run_tests"
            | "execute_command"
            | "terminal"
            | "bash"
            | "cargo_test"
            | "shell_exec"
            | "cargo_check"
            | "game_launch"
            | "skill_execute" => GovernanceAction::Shell,

            // ── Network operations (outbound) ──
            "http_request"
            | "web_search"
            | "dns_lookup"
            | "ping"
            | "port_scan"
            | "git"
            | "github_search_skills"
            | "game_monitor"
            | "game_online_status"
            | "goon_provider_test_completion"
            | "goon_provider_test_connection" => GovernanceAction::Network,

            // ── Keyword-based fallback ─────────────────────────
            _ => classify_action_by_keyword(tool),
        }
    }

    // ── Risk class (for default governance policy) ─────────────────────────
    //
    // Derived from tool_governance_defaults.rs classify_tool_risk().

    /// Return the risk class for a tool name.
    pub fn risk_class(tool: &str) -> ToolRiskClass {
        // Admin prefix check (highest priority)
        if tool.starts_with("goon_skill_") {
            return ToolRiskClass::Admin;
        }

        match tool {
            // Admin tools (workflow control)
            "goon_workflow_run_cancel" | "goon_workflow_run_pause" | "goon_workflow_run_resume" => {
                ToolRiskClass::Admin
            }

            // Read-only tools (explicit from tool_governance_defaults.rs)
            "search_files"
            | "read_file"
            | "inspect_git_diff"
            | "skill-finder"
            | "prompts_list"
            | "prompts_get"
            | "acp_trace_get"
            | "acp_debug_panel_get"
            | "goon_workflow_run_list"
            | "goon_workflow_run_get"
            | "goon_metrics_window_query"
            | "goon_metrics_errors_summary"
            | "goon_provider_capabilities"
            | "goon_skill_version_list" => ToolRiskClass::ReadOnly,

            // Low-risk write tools (explicit from tool_governance_defaults.rs)
            "write_file" | "apply_patch" => ToolRiskClass::LowRiskWrite,

            // High-risk execution tools (explicit from tool_governance_defaults.rs)
            "run_tests"
            | "bash"
            | "execute_command"
            | "shell_exec"
            | "goon_provider_test_connection"
            | "goon_provider_test_completion" => ToolRiskClass::HighRiskExecute,

            // ── Keyword-based fallback (matches tool_governance_defaults.rs) ──
            _ => classify_risk_by_keyword(tool),
        }
    }
}

// ── Keyword-based helpers ──────────────────────────────────────────────────

/// Classify a tool by keyword patterns into a sandbox operation.
fn classify_operation_by_keyword(tool: &str) -> ToolOperation {
    let lower = tool.to_ascii_lowercase();

    // Shell / execution keywords
    if lower.contains("shell")
        || lower.contains("command")
        || lower.contains("docker")
        || lower.contains("bash")
        || lower.contains("terminal")
        || lower.contains("exec")
    {
        return ToolOperation::Shell;
    }

    // Network keywords
    if lower.contains("http")
        || lower.contains("request")
        || lower.contains("network")
        || lower.contains("dns")
        || lower.contains("ping")
        || lower.contains("git")
        || lower.contains("port_scan")
    {
        return ToolOperation::Network;
    }

    // Search keywords
    if lower.contains("search") || lower.contains("find") || lower.contains("grep") {
        return ToolOperation::Search;
    }

    // Write / mutation keywords
    if lower.contains("write")
        || lower.contains("edit")
        || lower.contains("create")
        || lower.contains("delete")
        || lower.contains("move")
        || lower.contains("rename")
        || lower.contains("patch")
        || lower.contains("apply")
        || lower.contains("copy")
        || lower.contains("remove")
        || lower.contains("compress")
        || lower.contains("decompress")
        || lower.contains("extract")
        || lower.contains("generate")
        || lower.contains("convert")
        || lower.contains("resize")
        || lower.contains("transform")
        || lower.contains("install")
    {
        return ToolOperation::Write;
    }

    // Read / query keywords
    if lower.contains("read")
        || lower.contains("list")
        || lower.contains("get")
        || lower.contains("query")
        || lower.contains("inspect")
        || lower.contains("view")
        || lower.contains("show")
        || lower.contains("check")
        || lower.contains("diff")
        || lower.contains("analyze")
        || lower.contains("parse")
        || lower.contains("scrape")
    {
        return ToolOperation::Read;
    }

    // Unknown — no keyword matched
    ToolOperation::Unknown
}

/// Classify a tool by keyword patterns into a governance action.
fn classify_action_by_keyword(tool: &str) -> GovernanceAction {
    let lower = tool.to_ascii_lowercase();

    if lower.contains("shell") || lower.contains("command") || lower.contains("docker") {
        return GovernanceAction::Shell;
    }

    if lower.contains("write")
        || lower.contains("edit")
        || lower.contains("create")
        || lower.contains("delete")
        || lower.contains("move")
        || lower.contains("rename")
        || lower.contains("patch")
        || lower.contains("apply")
    {
        return GovernanceAction::Write;
    }

    if lower.contains("search") || lower.contains("find") {
        return GovernanceAction::Search;
    }

    if lower.contains("http") || lower.contains("request") || lower.contains("network") {
        return GovernanceAction::Network;
    }

    GovernanceAction::Read
}

/// Classify a tool by keyword patterns into a risk class.
///
/// Mirrors the fallback logic in `tool_governance_defaults.rs`.
fn classify_risk_by_keyword(tool: &str) -> ToolRiskClass {
    let lower = tool.to_ascii_lowercase();

    // High-risk keywords (destructive / execution operations)
    let high_risk_keywords = [
        "delete",
        "remove",
        "drop",
        "rm",
        "shutdown",
        "rollback",
        "revert",
        "reset",
        "force",
        "truncate",
        "uninstall",
    ];
    for kw in &high_risk_keywords {
        if lower.contains(kw) {
            return ToolRiskClass::HighRiskExecute;
        }
    }

    // Medium-risk keywords (write / edit / update operations)
    let medium_risk_keywords = [
        "write", "edit", "modify", "update", "create", "patch", "rename", "move", "copy",
    ];
    for kw in &medium_risk_keywords {
        if lower.contains(kw) {
            return ToolRiskClass::LowRiskWrite;
        }
    }

    // Low-risk keywords (read / search / query operations)
    let low_risk_keywords = [
        "read", "list", "search", "grep", "find", "view", "show", "get", "check", "test",
    ];
    for kw in &low_risk_keywords {
        if lower.contains(kw) {
            return ToolRiskClass::ReadOnly;
        }
    }

    // Unknown tools default to LowRiskWrite (conservative, requires policy)
    ToolRiskClass::LowRiskWrite
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governance::hardening::GovernanceAction;

    // ── operation() tests ───────────────────────────────────────────────

    #[test]
    fn operation_read() {
        assert_eq!(
            ToolCapabilityRegistry::operation("read_file"),
            ToolOperation::Read
        );
        assert_eq!(
            ToolCapabilityRegistry::operation("list_directory"),
            ToolOperation::Read
        );
        assert_eq!(
            ToolCapabilityRegistry::operation("diagnostics"),
            ToolOperation::Read
        );
        assert_eq!(
            ToolCapabilityRegistry::operation("skill_execute"),
            ToolOperation::Read
        );
    }

    #[test]
    fn operation_write() {
        assert_eq!(
            ToolCapabilityRegistry::operation("write_file"),
            ToolOperation::Write
        );
        assert_eq!(
            ToolCapabilityRegistry::operation("apply_patch"),
            ToolOperation::Write
        );
        assert_eq!(
            ToolCapabilityRegistry::operation("create_directory"),
            ToolOperation::Write
        );
        assert_eq!(
            ToolCapabilityRegistry::operation("delete_path"),
            ToolOperation::Write
        );
        assert_eq!(
            ToolCapabilityRegistry::operation("move_path"),
            ToolOperation::Write
        );
        assert_eq!(
            ToolCapabilityRegistry::operation("image_generate"),
            ToolOperation::Write
        );
    }

    #[test]
    fn operation_shell() {
        assert_eq!(
            ToolCapabilityRegistry::operation("bash"),
            ToolOperation::Shell
        );
        assert_eq!(
            ToolCapabilityRegistry::operation("terminal"),
            ToolOperation::Shell
        );
        assert_eq!(
            ToolCapabilityRegistry::operation("run_tests"),
            ToolOperation::Shell
        );
    }

    #[test]
    fn operation_network() {
        assert_eq!(
            ToolCapabilityRegistry::operation("http_request"),
            ToolOperation::Network
        );
        assert_eq!(
            ToolCapabilityRegistry::operation("git"),
            ToolOperation::Network
        );
        assert_eq!(
            ToolCapabilityRegistry::operation("dns_lookup"),
            ToolOperation::Network
        );
    }

    #[test]
    fn operation_search() {
        assert_eq!(
            ToolCapabilityRegistry::operation("grep"),
            ToolOperation::Search
        );
        assert_eq!(
            ToolCapabilityRegistry::operation("find_path"),
            ToolOperation::Search
        );
        assert_eq!(
            ToolCapabilityRegistry::operation("semantic_search"),
            ToolOperation::Search
        );
    }

    #[test]
    fn operation_unknown() {
        assert_eq!(
            ToolCapabilityRegistry::operation("completely_unknown_tool_xyz"),
            ToolOperation::Unknown
        );
    }

    #[test]
    fn operation_keyword_fallback_shell() {
        // Tools that follow naming conventions but aren't explicitly listed
        assert_eq!(
            ToolCapabilityRegistry::operation("custom_shell_tool"),
            ToolOperation::Shell
        );
        assert_eq!(
            ToolCapabilityRegistry::operation("docker_run"),
            ToolOperation::Shell
        );
    }

    #[test]
    fn operation_keyword_fallback_write() {
        assert_eq!(
            ToolCapabilityRegistry::operation("custom_edit"),
            ToolOperation::Write
        );
        assert_eq!(
            ToolCapabilityRegistry::operation("custom_remove"),
            ToolOperation::Write
        );
    }

    #[test]
    fn operation_keyword_fallback_read() {
        assert_eq!(
            ToolCapabilityRegistry::operation("custom_reader"),
            ToolOperation::Read
        );
        assert_eq!(
            ToolCapabilityRegistry::operation("custom_query_tool"),
            ToolOperation::Read
        );
    }

    // ── action() tests ─────────────────────────────────────────────────

    #[test]
    fn action_write() {
        assert_eq!(
            ToolCapabilityRegistry::action("write_file"),
            GovernanceAction::Write
        );
        assert_eq!(
            ToolCapabilityRegistry::action("apply_patch"),
            GovernanceAction::Write
        );
        assert_eq!(
            ToolCapabilityRegistry::action("delete_path"),
            GovernanceAction::Write
        );
    }

    #[test]
    fn action_shell() {
        assert_eq!(
            ToolCapabilityRegistry::action("shell_exec"),
            GovernanceAction::Shell
        );
        assert_eq!(
            ToolCapabilityRegistry::action("bash"),
            GovernanceAction::Shell
        );
        assert_eq!(
            ToolCapabilityRegistry::action("run_tests"),
            GovernanceAction::Shell
        );
    }

    #[test]
    fn action_search() {
        assert_eq!(
            ToolCapabilityRegistry::action("search_files"),
            GovernanceAction::Search
        );
        assert_eq!(
            ToolCapabilityRegistry::action("grep"),
            GovernanceAction::Search
        );
    }

    #[test]
    fn action_default_read() {
        assert_eq!(
            ToolCapabilityRegistry::action("read_file"),
            GovernanceAction::Read
        );
        assert_eq!(
            ToolCapabilityRegistry::action("list_directory"),
            GovernanceAction::Read
        );
    }

    #[test]
    fn action_keyword_network() {
        assert_eq!(
            ToolCapabilityRegistry::action("http_request"),
            GovernanceAction::Network
        );
    }

    #[test]
    fn action_keyword_shell_via_docker() {
        assert_eq!(
            ToolCapabilityRegistry::action("docker_build"),
            GovernanceAction::Shell
        );
    }

    // ── risk_class() tests ──────────────────────────────────────────────

    #[test]
    fn risk_class_admin() {
        assert_eq!(
            ToolCapabilityRegistry::risk_class("goon_skill_update"),
            ToolRiskClass::Admin
        );
        assert_eq!(
            ToolCapabilityRegistry::risk_class("goon_skill_version_rollback"),
            ToolRiskClass::Admin
        );
        assert_eq!(
            ToolCapabilityRegistry::risk_class("goon_workflow_run_cancel"),
            ToolRiskClass::Admin
        );
    }

    #[test]
    fn risk_class_read_only() {
        assert_eq!(
            ToolCapabilityRegistry::risk_class("read_file"),
            ToolRiskClass::ReadOnly
        );
        assert_eq!(
            ToolCapabilityRegistry::risk_class("search_files"),
            ToolRiskClass::ReadOnly
        );
        assert_eq!(
            ToolCapabilityRegistry::risk_class("prompts_list"),
            ToolRiskClass::ReadOnly
        );
    }

    #[test]
    fn risk_class_low_risk_write() {
        assert_eq!(
            ToolCapabilityRegistry::risk_class("write_file"),
            ToolRiskClass::LowRiskWrite
        );
        assert_eq!(
            ToolCapabilityRegistry::risk_class("apply_patch"),
            ToolRiskClass::LowRiskWrite
        );
    }

    #[test]
    fn risk_class_high_risk_execute() {
        assert_eq!(
            ToolCapabilityRegistry::risk_class("bash"),
            ToolRiskClass::HighRiskExecute
        );
        assert_eq!(
            ToolCapabilityRegistry::risk_class("run_tests"),
            ToolRiskClass::HighRiskExecute
        );
        assert_eq!(
            ToolCapabilityRegistry::risk_class("shell_exec"),
            ToolRiskClass::HighRiskExecute
        );
    }

    #[test]
    fn risk_class_keyword_fallback() {
        // High-risk via keyword
        assert_eq!(
            ToolCapabilityRegistry::risk_class("custom_delete_tool"),
            ToolRiskClass::HighRiskExecute
        );
        // Low-risk via keyword
        assert_eq!(
            ToolCapabilityRegistry::risk_class("custom_edit_tool"),
            ToolRiskClass::LowRiskWrite
        );
        // Read-only via keyword
        assert_eq!(
            ToolCapabilityRegistry::risk_class("custom_view_tool"),
            ToolRiskClass::ReadOnly
        );
        // Unknown defaults to LowRiskWrite
        assert_eq!(
            ToolCapabilityRegistry::risk_class("unknown_foobar"),
            ToolRiskClass::LowRiskWrite
        );
    }
}
