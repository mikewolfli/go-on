//! Centralized tool-name-to-capability registry.
//!
//! Unifies the previously independent tool-classification mappings into a single
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
//! All three methods look up the **same** [`TOOL_CLASSIFICATIONS`] table, so a
//! tool's classification can never disagree between consumers, plus a
//! **keyword-based fallback** as a safety net for tools that follow standard
//! naming conventions but have not yet been added to the table. Tools that
//! match no keyword and no table entry receive a safe default
//! (Read / Read / LowRiskWrite respectively).

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

    /// Return the sandbox operation for a tool name.
    ///
    /// Looks up the single [`TOOL_CLASSIFICATIONS`] table; tools not listed
    /// there fall back to keyword classification.
    pub fn operation(tool: &str) -> ToolOperation {
        lookup_tool(tool)
            .map(|c| c.1)
            .unwrap_or_else(|| classify_operation_by_keyword(tool))
    }

    // ── Governance action (for permission checks) ──────────────────────────

    /// Return the governance action for a tool name.
    ///
    /// This is the canonical tool→action mapping, consolidating what was
    /// previously duplicated in `pipeline_tool_to_action`. All sandbox
    /// governance paths should use this single source of truth. Looks up the
    /// single [`TOOL_CLASSIFICATIONS`] table; tools not listed there fall back
    /// to keyword classification.
    pub fn action(tool: &str) -> GovernanceAction {
        lookup_tool(tool)
            .map(|c| c.2)
            .unwrap_or_else(|| classify_action_by_keyword(tool))
    }

    // ── Risk class (for default governance policy) ─────────────────────────

    /// Return the risk class for a tool name.
    ///
    /// The `goon_skill_*` prefix is the highest-priority admin signal (it
    /// applies even to tools like `goon_skill_version_list` that the table
    /// classifies as read-only). Looks up the single [`TOOL_CLASSIFICATIONS`]
    /// table; tools without an explicit risk entry (or not listed at all)
    /// fall back to keyword classification.
    pub fn risk_class(tool: &str) -> ToolRiskClass {
        // Admin prefix check (highest priority)
        if tool.starts_with("goon_skill_") {
            return ToolRiskClass::Admin;
        }

        lookup_tool(tool)
            .and_then(|c| c.3)
            .unwrap_or_else(|| classify_risk_by_keyword(tool))
    }

    // ── Introspection ──────────────────────────────────────────────────────

    /// Names of every tool that has an explicit classification-table entry.
    ///
    /// Exposed for introspection (e.g. `governance::status::known_tool_names`)
    /// so tool-name reporting stays derived from the single classification
    /// table instead of maintaining a parallel hardcoded list that drifts
    /// whenever a new tool is classified.
    pub fn known_names() -> impl Iterator<Item = &'static str> {
        TOOL_CLASSIFICATIONS.iter().map(|(name, ..)| *name)
    }
}

/// Single row of the unified tool-classification table.
///
/// `(name, operation, action, risk)` — `risk: None` means the tool has no
/// explicit risk class and falls back to keyword classification.
type ToolClassification = (
    &'static str,
    ToolOperation,
    GovernanceAction,
    Option<ToolRiskClass>,
);

/// Single source of truth for tool classification: name → sandbox operation,
/// governance action and default risk class.
///
/// The three public methods (`operation` / `action` / `risk_class`) all look
/// up this one table, so a tool's classification can never disagree between
/// consumers. Previously the three explicit match arms drifted from each
/// other — e.g. `game_auto_grind` was Shell in `operation` but Write in
/// `action`, `cad_convert` was Read in `operation` but Write in `action`,
/// and the `docker_*` tools were only listed in one of the three arms. The
/// conflicting entries below were unified to the more accurate value:
///
/// - game input tools (`game_auto_grind`, `game_keyboard_input`,
///   `game_mouse_input`) execute in-game actions → Shell for both op and action
/// - `cad_convert` produces output files → Write for both op and action
/// - `search_files` / `search_packages` / `web_search` are search/network ops
/// - `run` / `build_run` / `docker_logs` / `format_code` / `dependency_add`
///   now get an explicit action matching their operation instead of the
///   generic keyword default
const TOOL_CLASSIFICATIONS: &[ToolClassification] = &[
    // ── Read / query tools ────────────────────────────────────────────
    (
        "read_file",
        ToolOperation::Read,
        GovernanceAction::Read,
        Some(ToolRiskClass::ReadOnly),
    ),
    (
        "read_file_lines",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    ("read", ToolOperation::Read, GovernanceAction::Read, None),
    (
        "list_directory",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "list_files",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    ("ls", ToolOperation::Read, GovernanceAction::Read, None),
    (
        "date_time",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "skill_list",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "skill-finder",
        ToolOperation::Read,
        GovernanceAction::Read,
        Some(ToolRiskClass::ReadOnly),
    ),
    (
        "skill_reload",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "chat.execute",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "acp_trace_get",
        ToolOperation::Read,
        GovernanceAction::Read,
        Some(ToolRiskClass::ReadOnly),
    ),
    (
        "acp_debug_panel_get",
        ToolOperation::Read,
        GovernanceAction::Read,
        Some(ToolRiskClass::ReadOnly),
    ),
    (
        "goon_workflow_run_list",
        ToolOperation::Read,
        GovernanceAction::Read,
        Some(ToolRiskClass::ReadOnly),
    ),
    (
        "goon_workflow_run_get",
        ToolOperation::Read,
        GovernanceAction::Read,
        Some(ToolRiskClass::ReadOnly),
    ),
    (
        "goon_metrics_window_query",
        ToolOperation::Read,
        GovernanceAction::Read,
        Some(ToolRiskClass::ReadOnly),
    ),
    (
        "goon_metrics_errors_summary",
        ToolOperation::Read,
        GovernanceAction::Read,
        Some(ToolRiskClass::ReadOnly),
    ),
    (
        "goon_provider_capabilities",
        ToolOperation::Read,
        GovernanceAction::Read,
        Some(ToolRiskClass::ReadOnly),
    ),
    (
        "prompts_list",
        ToolOperation::Read,
        GovernanceAction::Read,
        Some(ToolRiskClass::ReadOnly),
    ),
    (
        "prompts_get",
        ToolOperation::Read,
        GovernanceAction::Read,
        Some(ToolRiskClass::ReadOnly),
    ),
    (
        "workflow_execute",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "workflow_ask",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "workflow_generate",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "import_skill",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "archive_inspect",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "jsonl_read",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "environment_info",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "echo_skill",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "builtin.echo",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    // Risk resolved to Admin by the `goon_skill_*` prefix rule, not here.
    (
        "goon_skill_version_list",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    // ── Document readers ──────────────────────────────────────────────
    (
        "read_pdf",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "read_docx",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "read_excel",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "read_ppt",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "email_parse",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "invoice_parse",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "web_scrape",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    // ── CAD / 3D readers ──────────────────────────────────────────────
    (
        "dxf_read",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "step_read",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "obj_read",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "obj_model_read",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "stl_read",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "gltf_read",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "iges_read",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "ply_read",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "geo_util",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "gcode_read",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "gpx_read",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "svg_read",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    // ── Image / data readers ──────────────────────────────────────────
    (
        "image_analyze",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "csv_read",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "csv_analyze",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "toml_read",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "yaml_read",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "rss_read",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "sqlite_query",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    // ── Game readers ──────────────────────────────────────────────────
    (
        "game_server_query",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "game_price_tracker",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "game_matchmaking",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "game_achievements",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "game_mod_list",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "game_coaching_assistant",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    // ── Compilation / diagnostics ─────────────────────────────────────
    (
        "cargo_check",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "diagnostics",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    // ── Code analysis ─────────────────────────────────────────────────
    (
        "code_metrics",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "encode_decode",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "file_diff",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "file_watch",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "hash_file",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "lint_run",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "random_token",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "security_scan",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "template_render",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "uuid_gen",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    (
        "inspect_git_diff",
        ToolOperation::Read,
        GovernanceAction::Read,
        Some(ToolRiskClass::ReadOnly),
    ),
    (
        "docker_logs",
        ToolOperation::Read,
        GovernanceAction::Read,
        None,
    ),
    // ── Search / discovery tools ──────────────────────────────────────
    (
        "grep",
        ToolOperation::Search,
        GovernanceAction::Search,
        None,
    ),
    (
        "find_path",
        ToolOperation::Search,
        GovernanceAction::Search,
        None,
    ),
    (
        "semantic_search",
        ToolOperation::Search,
        GovernanceAction::Search,
        None,
    ),
    (
        "code_index_search",
        ToolOperation::Search,
        GovernanceAction::Search,
        None,
    ),
    (
        "find_files",
        ToolOperation::Search,
        GovernanceAction::Search,
        None,
    ),
    (
        "search",
        ToolOperation::Search,
        GovernanceAction::Search,
        None,
    ),
    (
        "find",
        ToolOperation::Search,
        GovernanceAction::Search,
        None,
    ),
    (
        "search_files",
        ToolOperation::Search,
        GovernanceAction::Search,
        Some(ToolRiskClass::ReadOnly),
    ),
    (
        "search_packages",
        ToolOperation::Search,
        GovernanceAction::Search,
        None,
    ),
    // ── Network / outbound tools ──────────────────────────────────────
    (
        "http_request",
        ToolOperation::Network,
        GovernanceAction::Network,
        None,
    ),
    (
        "web_search",
        ToolOperation::Network,
        GovernanceAction::Network,
        None,
    ),
    (
        "dns_lookup",
        ToolOperation::Network,
        GovernanceAction::Network,
        None,
    ),
    (
        "ping",
        ToolOperation::Network,
        GovernanceAction::Network,
        None,
    ),
    (
        "port_scan",
        ToolOperation::Network,
        GovernanceAction::Network,
        None,
    ),
    (
        "git",
        ToolOperation::Network,
        GovernanceAction::Network,
        None,
    ),
    (
        "github_search_skills",
        ToolOperation::Network,
        GovernanceAction::Network,
        None,
    ),
    (
        "game_monitor",
        ToolOperation::Network,
        GovernanceAction::Network,
        None,
    ),
    (
        "game_online_status",
        ToolOperation::Network,
        GovernanceAction::Network,
        None,
    ),
    (
        "goon_provider_test_connection",
        ToolOperation::Network,
        GovernanceAction::Network,
        Some(ToolRiskClass::HighRiskExecute),
    ),
    (
        "goon_provider_test_completion",
        ToolOperation::Network,
        GovernanceAction::Network,
        Some(ToolRiskClass::HighRiskExecute),
    ),
    // ── Shell / execution tools ───────────────────────────────────────
    (
        "run_tests",
        ToolOperation::Shell,
        GovernanceAction::Shell,
        Some(ToolRiskClass::HighRiskExecute),
    ),
    (
        "execute_command",
        ToolOperation::Shell,
        GovernanceAction::Shell,
        Some(ToolRiskClass::HighRiskExecute),
    ),
    (
        "terminal",
        ToolOperation::Shell,
        GovernanceAction::Shell,
        None,
    ),
    (
        "bash",
        ToolOperation::Shell,
        GovernanceAction::Shell,
        Some(ToolRiskClass::HighRiskExecute),
    ),
    (
        "cargo_test",
        ToolOperation::Shell,
        GovernanceAction::Shell,
        None,
    ),
    (
        "shell_exec",
        ToolOperation::Shell,
        GovernanceAction::Shell,
        Some(ToolRiskClass::HighRiskExecute),
    ),
    ("run", ToolOperation::Shell, GovernanceAction::Shell, None),
    (
        "build_run",
        ToolOperation::Shell,
        GovernanceAction::Shell,
        None,
    ),
    (
        "docker_build",
        ToolOperation::Shell,
        GovernanceAction::Shell,
        None,
    ),
    (
        "docker_compose",
        ToolOperation::Shell,
        GovernanceAction::Shell,
        None,
    ),
    (
        "docker_exec",
        ToolOperation::Shell,
        GovernanceAction::Shell,
        None,
    ),
    (
        "docker_push",
        ToolOperation::Shell,
        GovernanceAction::Shell,
        None,
    ),
    (
        "spawn_agent",
        ToolOperation::Shell,
        GovernanceAction::Shell,
        None,
    ),
    (
        "game_launch",
        ToolOperation::Shell,
        GovernanceAction::Shell,
        None,
    ),
    (
        "game_keyboard_input",
        ToolOperation::Shell,
        GovernanceAction::Shell,
        None,
    ),
    (
        "game_mouse_input",
        ToolOperation::Shell,
        GovernanceAction::Shell,
        None,
    ),
    (
        "game_auto_grind",
        ToolOperation::Shell,
        GovernanceAction::Shell,
        None,
    ),
    (
        "skill_execute",
        ToolOperation::Shell,
        GovernanceAction::Shell,
        None,
    ),
    // ── Write / admin tools ───────────────────────────────────────────
    (
        "write_file",
        ToolOperation::Write,
        GovernanceAction::Write,
        Some(ToolRiskClass::LowRiskWrite),
    ),
    ("write", ToolOperation::Write, GovernanceAction::Write, None),
    (
        "create",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "apply_patch",
        ToolOperation::Write,
        GovernanceAction::Write,
        Some(ToolRiskClass::LowRiskWrite),
    ),
    (
        "apply_code_action",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "create_directory",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "dependency_add",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "delete_path",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "move_path",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "copy_path",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "file_move",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "file_delete",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "edit_file",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "format_code",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "compress",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "decompress",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "archive_extract",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "jsonl_write",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "csv_write",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "csv_transform",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "toml_write",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "yaml_write",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "write_docx",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "write_excel",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "write_ppt",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "svg_generate",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "svg_export",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "stl_generate",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "qrcode_generate",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "image_generate",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "image_resize",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "image_convert",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "skill_create",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "skill-creator",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "goon_skill_update",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "goon_skill_version_rollback",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "goon_workflow_run_cancel",
        ToolOperation::Write,
        GovernanceAction::Write,
        Some(ToolRiskClass::Admin),
    ),
    (
        "goon_workflow_run_pause",
        ToolOperation::Write,
        GovernanceAction::Write,
        Some(ToolRiskClass::Admin),
    ),
    (
        "goon_workflow_run_resume",
        ToolOperation::Write,
        GovernanceAction::Write,
        Some(ToolRiskClass::Admin),
    ),
    (
        "game_screen_capture",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "game_replay_recorder",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "game_save_manager",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "game_mod_install",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "game_state_modify",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "pdf_merge",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "pdf_split",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
    (
        "cad_convert",
        ToolOperation::Write,
        GovernanceAction::Write,
        None,
    ),
];

/// Look up a tool in the unified [`TOOL_CLASSIFICATIONS`] table.
fn lookup_tool(
    tool: &str,
) -> Option<(
    &'static str,
    ToolOperation,
    GovernanceAction,
    Option<ToolRiskClass>,
)> {
    TOOL_CLASSIFICATIONS
        .iter()
        .find(|(name, ..)| *name == tool)
        .copied()
}

// ── Keyword-based helpers ──────────────────────────────────────────────────

/// Classify a tool by keyword patterns into a sandbox operation.
fn classify_operation_by_keyword(tool: &str) -> ToolOperation {
    let lower = tool.to_ascii_lowercase();

    // Synthetic phase-dispatch actions (`phase.full_auto.execute` etc.) are
    // internal orchestration routing signals, not shell operations. Without
    // this guard the `exec` keyword below would classify them as Shell,
    // denying every phase request under Basic+ sandbox levels.
    if tool.starts_with("phase.") {
        return ToolOperation::Read;
    }

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

    // Synthetic phase-dispatch actions are read-only orchestration signals
    // (see `classify_operation_by_keyword` for the same guard).
    if tool.starts_with("phase.") {
        return GovernanceAction::Read;
    }

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
        // skill_execute runs skill code, so it is a Shell operation
        // (consistency with GovernanceAction::Shell).
        assert_eq!(
            ToolCapabilityRegistry::operation("skill_execute"),
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
