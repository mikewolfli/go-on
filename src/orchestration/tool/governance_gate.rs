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
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::SystemTime;

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
        Self { shards }
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
        guard.get(key)
    }

    /// Insert a permission result into the cache.
    ///
    /// Only the targeted shard is locked.
    pub fn insert(&self, key: String, value: bool) {
        let idx = self.shard_index(&key);
        let mut guard = self.shards[idx].lock().unwrap_or_else(|e| e.into_inner());
        guard.insert(key, value);
    }
}

/// Global governance cache, lazily initialized on first use.
static GOVERNANCE_CACHE: OnceLock<ShardedGovernanceCache> = OnceLock::new();

/// Access the global governance cache singleton.
pub fn governance_cache() -> &'static ShardedGovernanceCache {
    GOVERNANCE_CACHE.get_or_init(|| ShardedGovernanceCache::new(1000))
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
            | "uuid_gen"
            | "random_token"
            | "encode_decode"
            | "hash_file"
            | "diagnostics"
            | "file_diff"
            | "format_code"
            | "code_metrics"
            | "svg_export"
            | "rss_read"
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
// Low-risk tool classification helpers
// ---------------------------------------------------------------------------
