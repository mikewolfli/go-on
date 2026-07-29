//! Unified governance gate for tool execution.
//!
//! Consolidates the governance checks that were duplicated across
//! `executor.rs` and `pipeline.rs` into a single location.
//!
//! Contains:
//! - Low-risk tool classification and audit logging (used by the executor)
//! - Sandbox-based pipeline governance checks (used by the pipeline)
//! - [`ShardedGovernanceCache`] for caching ACP permission results per session
//!   with sharded locking for high-concurrency scenarios

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::SystemTime;

use crate::governance::hardening::SandboxLevel;
use crate::orchestration::cache_layer::{CacheLayer, CacheStats};

// ---------------------------------------------------------------------------
// ShardedGovernanceCache — sharded per-session ACP permission cache
// ---------------------------------------------------------------------------

/// Number of cache shards. Must be a power of 2 (16) for fast modular hashing.
const SHARD_COUNT: usize = 16;

/// Per-shard LRU cache used internally by [`ShardedGovernanceCache`].
struct GovernanceCache {
    cache: HashMap<String, bool>,
    order: VecDeque<String>,
    max_entries: usize,
}

impl GovernanceCache {
    fn new(max_entries: usize) -> Self {
        Self {
            cache: HashMap::new(),
            order: VecDeque::new(),
            max_entries,
        }
    }

    fn get(&self, key: &str) -> Option<bool> {
        self.cache.get(key).copied()
    }

    fn insert(&mut self, key: String, value: bool) {
        // If the key already exists, don't change insertion order.
        if self.cache.contains_key(&key) {
            return;
        }
        self.cache.insert(key.clone(), value);
        self.order.push_back(key);

        // Evict oldest entries once we exceed the max.
        while self.order.len() > self.max_entries {
            if let Some(oldest) = self.order.pop_front() {
                self.cache.remove(&oldest);
            }
        }
    }
}

/// A sharded, thread-safe cache for tool governance permission results.
///
/// Caches the result of `request_client_permission` for each
/// `"{session_id}:{tool_name}"` key to avoid redundant network round-trips
/// to the ACP client within the same session.
///
/// # Concurrency
///
/// Instead of a single `Mutex<GovernanceCache>`, this uses 16 shards each
/// behind their own `Mutex`.  Keys are distributed by hash, so concurrent
/// accesses on different shards do not contend.  This is critical for the
/// multi-users-server profile where many sessions may check permissions
/// simultaneously.
pub struct ShardedGovernanceCache {
    shards: Vec<Mutex<GovernanceCache>>,
    #[allow(dead_code)]
    total_max_entries: usize,
    hits: AtomicU64,
    misses: AtomicU64,
}

/// Fast FNV-1a hash for distributing keys across shards.
///
/// Intentionally simple and deterministic — no dependency required.
fn hash_key(key: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;
    let mut hash = FNV_OFFSET;
    for byte in key.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

impl ShardedGovernanceCache {
    /// Create a new sharded cache with `total_max_entries` spread evenly
    /// across all shards.
    fn new(total_max_entries: usize) -> Self {
        let per_shard = total_max_entries / SHARD_COUNT;
        let shards = (0..SHARD_COUNT)
            .map(|_| Mutex::new(GovernanceCache::new(per_shard)))
            .collect();
        Self {
            shards,
            total_max_entries,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// Pick the shard index for a key via its hash.
    fn shard_index(&self, key: &str) -> usize {
        (hash_key(key) as usize) % SHARD_COUNT
    }

    /// Look up a cached permission result.
    ///
    /// Returns `Some(true)` (approved), `Some(false)` (denied), or `None`
    /// (not cached yet).  Only the targeted shard is locked.
    pub fn get(&self, key: &str) -> Option<bool> {
        let idx = self.shard_index(key);
        let guard = self.shards[idx].lock().unwrap_or_else(|e| e.into_inner());
        let result = guard.get(key);
        if result.is_some() {
            self.hits.fetch_add(1, Ordering::Relaxed);
        } else {
            self.misses.fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    /// Insert a permission result into the cache.
    ///
    /// Only the targeted shard is locked.
    pub fn insert(&self, key: String, value: bool) {
        let idx = self.shard_index(&key);
        let mut guard = self.shards[idx].lock().unwrap_or_else(|e| e.into_inner());
        guard.insert(key, value);
    }

    /// Return the number of live entries across all shards.
    #[allow(dead_code)]
    fn entry_count(&self) -> usize {
        self.shards
            .iter()
            .filter_map(|m| m.lock().ok())
            .map(|g| g.cache.len())
            .sum()
    }
}

/// Global governance cache, lazily initialized on first use.
static GOVERNANCE_CACHE: OnceLock<ShardedGovernanceCache> = OnceLock::new();

/// Access the global governance cache singleton.
pub fn governance_cache() -> &'static ShardedGovernanceCache {
    GOVERNANCE_CACHE.get_or_init(|| ShardedGovernanceCache::new(1000))
}

// ---------------------------------------------------------------------------
// CacheLayer implementation for ShardedGovernanceCache
// ---------------------------------------------------------------------------

impl CacheLayer for ShardedGovernanceCache {
    fn name(&self) -> &str {
        "governance"
    }

    fn stats(&self) -> CacheStats {
        let entry_count = self.entry_count();
        // Rough estimate: each entry stores a String key plus a bool, plus
        // VecDeque node overhead.  Assume ~48 bytes per entry on average.
        let estimated_size_bytes = entry_count.saturating_mul(48);
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            entries: entry_count,
            max_entries: self.total_max_entries,
            estimated_size_bytes,
        }
    }

    fn clear(&mut self) {
        for shard in &self.shards {
            if let Ok(mut guard) = shard.lock() {
                guard.cache.clear();
                guard.order.clear();
            }
        }
        self.hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// Low-risk tool classification
// ---------------------------------------------------------------------------

/// Determine whether a tool is low-risk and can skip the blocking governance gate.
///
/// Low-risk tools are read-only, informational, or utility tools that pose
/// minimal security or safety concern. For these tools, synchronous governance
/// approval can be replaced by async audit logging.
pub fn is_low_risk_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "read_file"
            | "search_files"
            | "list_directory"
            | "grep"
            | "environment_info"
            | "time_util"
            | "uuid_gen"
            | "random_token"
            | "encode_decode"
            | "hash_file"
            | "diagnostics"
            | "diff"
            | "format_code"
            | "code_metrics"
            | "svg_export"
            | "rss_feed"
            | "date_time"
            | "dns_lookup"
    )
}

/// Record an audit log entry for a low-risk tool access.
///
/// This replaces the blocking governance gate for low-risk tools,
/// ensuring observability without blocking execution.
pub fn low_risk_audit_log(tool_name: &str, operation_mode: &str) {
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    tracing::debug!(
        target: "governance::low_risk",
        timestamp = timestamp,
        tool_name = tool_name,
        operation_mode = operation_mode,
        "low_risk_tool_access: governance gate skipped"
    );
}

// ---------------------------------------------------------------------------
// Pipeline sandbox governance
// ---------------------------------------------------------------------------

/// Map a tool name to a governance action for pipeline sandbox checks.
/// This mirrors the evaluator's tool-to-action mapping in a simplified form.
///
/// # Security audit
/// All tools registered in `ToolRegistry::new()` must be mapped here.
/// Unknown tools default to "read" (lowest risk) but log a warning.
fn pipeline_tool_to_action(tool_name: &str) -> &'static str {
    match tool_name {
        // ── Read operations (read-only file/content access) ──
        "read_file" | "search_files" | "inspect_git_diff" | "list_directory" | "date_time"
        | "skill_list" | "archive_inspect" | "jsonl_read" | "diagnostics" | "environment_info"
        | "echo_skill" | "builtin.echo" | "goon_skill_version_list"
        | "skill-finder" | "chat.execute"
        | "acp_trace_get" | "acp_debug_panel_get"
        | "goon_workflow_run_list" | "goon_workflow_run_get"
        | "goon_metrics_window_query" | "goon_metrics_errors_summary"
        | "goon_provider_capabilities" | "prompts_list" | "prompts_get"
        | "workflow_execute" | "workflow_ask" | "workflow_generate"
        | "import_skill" | "skill_reload"
        | "semantic_search"
        // ── CAD read tools (read-only 3d/2d format parsing) ──
        | "dxf_read" | "stl_read" | "obj_read" | "step_read" | "ply_read" | "iges_read"
        | "gltf_read" | "svg_read" | "obj_model_read" | "gcode_read" | "gpx_read" | "geo_util"
        // ── Image read/analyze tools ──
        | "image_analyze"
        // ── Document read tools ──
        | "read_docx" | "read_excel" | "read_pdf" | "read_ppt"
        | "email_parse" | "csv_read" | "csv_analyze" | "toml_read" | "yaml_read"
        | "web_scrape" | "invoice_parse" | "rss_read" | "sqlite_query" => "read",

        // ── Search operations ──
        "grep" | "find_path" | "find_files" | "code_index_search" => "search",

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
        | "spawn_agent" => "write",

        // ── Shell operations (command/code execution) ──
        "run_tests"
        | "execute_command"
        | "terminal"
        | "bash"
        | "cargo_test"
        | "shell_exec"
        | "cargo_check"
        | "game_launch"
        | "skill_execute" => "shell",

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
        | "goon_provider_test_connection" => "network",

        // Unknown — default to read (lowest risk), log warning for security audit
        _ => {
            tracing::warn!(
                target: "tool_pipeline",
                tool = %tool_name,
                "pipeline_tool_to_action: unknown tool '{}', defaulting to 'read' action — audit needed",
                tool_name,
            );
            "read"
        }
    }
}

/// Check if a tool is allowed at the given sandbox level.
///
/// This is the unified sandbox governance check used by the pipeline
/// to gate tool execution before running a step.
pub fn check_tool_in_pipeline(
    tool_name: &str,
    sandbox_level: Option<SandboxLevel>,
) -> Result<(), String> {
    let Some(level) = sandbox_level else {
        return Ok(()); // No sandbox enforcement
    };
    let action = pipeline_tool_to_action(tool_name);
    let result = crate::governance::hardening::SandboxPolicy::check_with_feedback(level, action);
    if result.allowed {
        Ok(())
    } else {
        let hint = result
            .hint
            .unwrap_or("Try a different tool or adjust sandbox level in config.");
        Err(format!(
            "tool '{}' denied by sandbox policy at level '{}' (action: '{}'). {}. Hint: {}",
            tool_name, level, action, result.reason, hint
        ))
    }
}
