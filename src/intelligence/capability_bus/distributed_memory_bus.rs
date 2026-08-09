//! DistributedMemoryBus — Cross-node memory sharing sub-bus (BLUE38 ARCH-13)
//!
//! DistributedMemoryBus enables agents on different server instances to share
//! experience and knowledge by coordinating local memory entries with known
//! remote peers.
//!
//! # Protocol
//!
//! 1. `store_local` records an entry locally; `share_with_peers` copies it
//!    into the local `shared_entries` view.
//! 2. The transport loop (`start_transport` / `sync_now`) serialises local
//!    entries and pushes them to each registered peer over real HTTP
//!    (JSON-RPC `memory.ingest` against the peer's `/rpc` endpoint).
//! 3. On receipt, the peer's hub stores the entries (`hub` `memory.ingest`
//!    method); the node's own bus can ingest them via `ingest_shared`.
//! 4. Expired entries are pruned periodically by `prune_expired`.
//!
//! The transport is only active when a deployment calls `start_transport`
//! and registers peers via `register_peer`; until then the bus is purely
//! local. Sync attempts fail loudly (never silently dropped or simulated)
//! when a peer cannot be reached.
//!
//! # Feature gates
//!
//! - `#[cfg(not(feature = "multi-users-server"))]` — single‑node;
//!   the bus still compiles but remote‑peer operations are no‑ops (or
//!   strictly local).
//! - `#[cfg(feature = "multi-users-server")]` — multi‑node; the
//!   full peer set and shared‑entry machinery is active.

use crate::i18n::runtime::tf;

use serde::{Deserialize, Serialize};
#[cfg(feature = "multi-users-server")]
use serde_json::json;
use tracing;

use std::collections::{HashMap, VecDeque};
#[cfg(feature = "multi-users-server")]
use std::sync::atomic::AtomicBool;
#[cfg(feature = "multi-users-server")]
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, RwLock};
#[cfg(feature = "multi-users-server")]
use std::thread;
#[cfg(feature = "multi-users-server")]
use std::thread::JoinHandle;
#[cfg(feature = "multi-users-server")]
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Transport data types
// ---------------------------------------------------------------------------

/// Configuration for the HTTP-based memory transport.
#[derive(Debug, Clone)]
pub struct MemoryTransportConfig {
    /// Address to listen on for incoming sync requests.
    /// Default: "127.0.0.1:0" (OS-assigned port).
    pub listen_addr: String,
    /// Timeout in milliseconds for outbound connect attempts.
    /// Default: 5000.
    pub connect_timeout_ms: u64,
    /// Interval in milliseconds between background sync cycles.
    /// Default: 30000.
    pub sync_interval_ms: u64,
    /// Maximum payload size in bytes for a single sync request.
    /// Default: 1_048_576 (1 MiB).
    pub max_payload_bytes: usize,
    /// Optional bearer token for authenticating to peer hubs.
    /// The receiving hub's `memory.ingest` method requires the same
    /// `Authorization: Bearer <token>` as the other hub RPC methods.
    pub auth_token: Option<String>,
}

impl Default for MemoryTransportConfig {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1:0".to_string(),
            connect_timeout_ms: 5000,
            sync_interval_ms: 30000,
            max_payload_bytes: 1_048_576,
            auth_token: None,
        }
    }
}

/// Represents the current status of a sync operation.
#[derive(Debug, Clone, Default)]
pub enum SyncStatus {
    /// No sync operation is in progress.
    #[default]
    Idle,
    /// A sync operation is currently running.
    Syncing,
    /// The last sync operation failed with the given error message.
    Failed(String),
    /// The last sync operation completed successfully.
    Completed {
        /// Number of entries that were synced.
        entries_synced: usize,
        /// Duration of the sync operation in milliseconds.
        duration_ms: u64,
    },
}

/// Statistics collected by the transport layer.
#[derive(Debug, Clone, Default)]
pub struct TransportStats {
    /// Total number of sync operations sent to peers.
    pub total_syncs_sent: u64,
    /// Total number of sync operations received from peers.
    pub total_syncs_received: u64,
    /// Total number of transport-level errors encountered.
    pub total_errors: u64,
    /// Status of the last sync operation.
    pub last_sync_status: SyncStatus,
    /// Total bytes sent over the transport.
    pub bytes_sent: u64,
    /// Total bytes received over the transport.
    pub bytes_received: u64,
}

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// A single memory entry that originated on some node.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemoryBusEntry {
    /// Unique identifier (e.g. UUID or ULID).
    pub id: String,
    /// Node that created this entry.
    pub node_id: String,
    /// Logical key used for lookups.
    pub key: String,
    /// Opaque payload value.
    pub value: String,
    /// Free‑form tags for discovery / filtering.
    pub tags: Vec<String>,
    /// Confidence score in [0.0, 1.0].
    pub confidence: f64,
    /// Creation timestamp (milliseconds since epoch).
    pub created_ms: u64,
    /// Time‑to‑live in milliseconds (0 = immortal).
    pub ttl_ms: u64,
}

/// A memory entry that was synced from a remote peer.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SharedMemoryEntry {
    /// The original entry data.
    pub entry: MemoryBusEntry,
    /// Local timestamp of the sync operation (milliseconds since epoch).
    pub synced_ms: u64,
    /// Node ID of the peer that provided this entry.
    pub source_node: String,
    /// How many times this entry has been forwarded (sync hops).
    pub sync_count: u32,
}

/// Snapshot of runtime metrics / profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributedMemoryBusProfile {
    /// Whether the distributed bus is enabled.
    pub enabled: bool,
    /// Number of local entries currently held.
    pub local_entries: u32,
    /// Number of known remote peers.
    pub remote_peers: u32,
    /// Number of shared entries currently held.
    pub shared_entries: u32,
    /// Total sync operations performed.
    pub total_syncs: u64,
    /// Total entries removed by pruning.
    pub entries_pruned: u64,
    /// Whether the transport layer is running.
    pub transport_running: bool,
    /// Number of reachable transport peers.
    pub transport_peers_reachable: u32,
    /// Total bytes synced over the transport layer.
    pub total_bytes_synced: u64,
}

impl Default for DistributedMemoryBusProfile {
    fn default() -> Self {
        Self {
            enabled: true,
            local_entries: 0,
            remote_peers: 0,
            shared_entries: 0,
            total_syncs: 0,
            entries_pruned: 0,
            transport_running: false,
            transport_peers_reachable: 0,
            total_bytes_synced: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// DistributedMemoryBus
// ---------------------------------------------------------------------------

/// Cross‑node memory sharing sub‑bus.
///
/// All data is kept in‑memory.  See the [module documentation](self) for the
/// planned network transport protocol.
pub struct DistributedMemoryBus {
    /// Local memory entries (originated on this node).
    local_entries: Arc<Mutex<VecDeque<MemoryBusEntry>>>,
    /// Known remote peers — maps `node_id` → `address` (e.g. "host:port").
    remote_peers: Arc<RwLock<HashMap<String, String>>>,
    /// Shared memory entries synced from peers.
    shared_entries: Arc<Mutex<VecDeque<SharedMemoryEntry>>>,
    /// Maximum number of entries to retain (local + shared).
    max_entries: usize,
    /// Profile / metrics snapshot.
    profile: Arc<Mutex<DistributedMemoryBusProfile>>,
    /// Transport running flag.
    #[cfg(feature = "multi-users-server")]
    transport_running: Arc<AtomicBool>,
    /// Transport configuration.
    #[cfg(feature = "multi-users-server")]
    transport_config: Arc<Mutex<Option<MemoryTransportConfig>>>,
    /// Transport statistics.
    transport_stats: Arc<Mutex<TransportStats>>,
    /// Handle for the background sync thread.
    #[cfg(feature = "multi-users-server")]
    sync_thread: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl DistributedMemoryBus {
    /// Create a new `DistributedMemoryBus` with the given capacity.
    ///
    /// `max_entries` controls how many local + shared entries are kept before
    /// the oldest (by creation time) are evicted.  Defaults to 10 000.
    pub fn new(max_entries: usize) -> Self {
        let max = if max_entries == 0 {
            10_000
        } else {
            max_entries
        };
        Self {
            local_entries: Arc::new(Mutex::new(VecDeque::with_capacity(max))),
            remote_peers: Arc::new(RwLock::new(HashMap::new())),
            shared_entries: Arc::new(Mutex::new(VecDeque::with_capacity(max))),
            max_entries: max,
            profile: Arc::new(Mutex::new(DistributedMemoryBusProfile::default())),
            #[cfg(feature = "multi-users-server")]
            transport_running: Arc::new(AtomicBool::new(false)),
            #[cfg(feature = "multi-users-server")]
            transport_config: Arc::new(Mutex::new(None)),
            transport_stats: Arc::new(Mutex::new(TransportStats::default())),
            #[cfg(feature = "multi-users-server")]
            sync_thread: Arc::new(Mutex::new(None)),
        }
    }

    // ------------------------------------------------------------------
    // Local storage
    // ------------------------------------------------------------------

    /// Store a new local memory entry.
    ///
    /// Returns the generated entry ID.
    ///
    /// If the total entry count would exceed `max_entries`, the oldest entry
    /// (by creation time) is evicted first.
    pub fn store_local(
        &self,
        key: &str,
        value: &str,
        tags: Vec<String>,
        confidence: f64,
        ttl_ms: u64,
    ) -> String {
        let id = uuid_v4();
        let entry = MemoryBusEntry {
            id: id.clone(),
            node_id: local_node_id(),
            key: key.to_string(),
            value: value.to_string(),
            tags,
            confidence: confidence.clamp(0.0, 1.0),
            created_ms: crate::shared::timestamps::now_ts_ms() as u64,
            ttl_ms,
        };

        // Compute shared length inside the local_entries lock scope.
        // Lock order: local → shared (consistent with share_with_peers / prune_expired).
        let mut entries = self.local_entries.lock().unwrap_or_else(|e| e.into_inner());
        let shared_len = self.shared_entries.lock().map(|g| g.len()).unwrap_or(0);
        entries.push_back(entry);
        Self::maybe_evict_oldest(&mut entries, shared_len, self.max_entries);

        // Update profile
        let mut p = self.profile.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned");
            poisoned.into_inner()
        });
        p.local_entries = entries.len() as u32;

        id
    }

    // ------------------------------------------------------------------
    // Query
    // ------------------------------------------------------------------

    /// Find all entries (local + shared) whose `key` matches exactly.
    pub fn find_by_key(&self, key: &str) -> Vec<MemoryBusEntry> {
        let mut results = Vec::new();

        let local = self.local_entries.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned");
            poisoned.into_inner()
        });
        for e in local.iter() {
            if e.key == key {
                results.push(e.clone());
            }
        }
        drop(local);

        let shared = self.shared_entries.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned");
            poisoned.into_inner()
        });
        for se in shared.iter() {
            if se.entry.key == key {
                results.push(se.entry.clone());
            }
        }

        results
    }

    /// Find all entries (local + shared) that have **any** of the given tags.
    pub fn find_by_tags(&self, tags: &[String]) -> Vec<MemoryBusEntry> {
        let mut results = Vec::new();

        if tags.is_empty() {
            return results;
        }

        let local = self.local_entries.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned");
            poisoned.into_inner()
        });
        for e in local.iter() {
            if e.tags.iter().any(|t| tags.contains(t)) {
                results.push(e.clone());
            }
        }
        drop(local);

        let shared = self.shared_entries.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned");
            poisoned.into_inner()
        });
        for se in shared.iter() {
            if se.entry.tags.iter().any(|t| tags.contains(t)) {
                results.push(se.entry.clone());
            }
        }

        results
    }

    // ------------------------------------------------------------------
    // Peer management
    // ------------------------------------------------------------------

    /// Register (or update) a remote peer.
    #[cfg(feature = "multi-users-server")]
    pub fn register_peer(&self, node_id: &str, address: &str) {
        let mut peers = self.remote_peers.write().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned");
            poisoned.into_inner()
        });
        peers.insert(node_id.to_string(), address.to_string());
        let mut p = self.profile.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned");
            poisoned.into_inner()
        });
        p.remote_peers = peers.len() as u32;
    }

    /// No‑op on single‑node builds.
    #[cfg(not(feature = "multi-users-server"))]
    pub fn register_peer(&self, _node_id: &str, _address: &str) {
        // Single‑node mode — nothing to register.
    }

    /// Remove a remote peer.
    #[cfg(feature = "multi-users-server")]
    pub fn unregister_peer(&self, node_id: &str) {
        let mut peers = self.remote_peers.write().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned");
            poisoned.into_inner()
        });
        peers.remove(node_id);
        let mut p = self.profile.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned");
            poisoned.into_inner()
        });
        p.remote_peers = peers.len() as u32;
    }

    /// Configure the HTTP transport from environment variables and start it.
    ///
    /// Reads:
    /// - `GOON_MEMORY_PEERS` — comma-separated `node_id=host:port` pairs
    ///   (e.g. `node1=10.0.0.2:8090,node2=10.0.0.3:8090`)
    /// - `GOON_MEMORY_SYNC_INTERVAL_MS` — background sync cadence (default 30000)
    /// - `GOON_MEMORY_AUTH_TOKEN` — bearer token sent to peer hubs
    ///
    /// Returns `Ok(false)` when no peers are configured (transport stays
    /// purely local); `Ok(true)` once the transport thread is running.
    #[cfg(feature = "multi-users-server")]
    pub fn configure_from_env(&self) -> anyhow::Result<bool> {
        let peers = std::env::var("GOON_MEMORY_PEERS").unwrap_or_default();
        if peers.trim().is_empty() {
            return Ok(false);
        }
        for pair in peers.split(',') {
            let pair = pair.trim();
            if let Some((node_id, addr)) = pair.split_once('=') {
                self.register_peer(node_id.trim(), addr.trim());
            }
        }
        let config = MemoryTransportConfig {
            sync_interval_ms: std::env::var("GOON_MEMORY_SYNC_INTERVAL_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30_000),
            auth_token: std::env::var("GOON_MEMORY_AUTH_TOKEN").ok(),
            ..Default::default()
        };
        self.start_transport(config)?;
        Ok(true)
    }

    /// No-op on single-node builds.
    #[cfg(not(feature = "multi-users-server"))]
    pub fn configure_from_env(&self) -> anyhow::Result<bool> {
        Ok(false)
    }

    /// No‑op on single‑node builds.
    #[cfg(not(feature = "multi-users-server"))]
    pub fn unregister_peer(&self, _node_id: &str) {
        // Single‑node mode — nothing to unregister.
    }

    /// Return a snapshot of all known peers (node_id → address).
    pub fn peers(&self) -> HashMap<String, String> {
        if let Ok(peers) = self.remote_peers.read() {
            peers.clone()
        } else {
            HashMap::new()
        }
    }

    // ------------------------------------------------------------------
    // Sharing
    // ------------------------------------------------------------------

    /// Mark a local entry (by `entry_id`) as shared with all known peers.
    ///
    /// Copies the entry into the local `shared_entries` view; the transport
    /// loop (`start_transport` / `sync_now`) fan‑outs the data to each
    /// peer's network address over HTTP.
    ///
    /// Returns `true` if the entry was found and shared, `false` otherwise.
    pub fn share_with_peers(&self, entry_id: &str) -> bool {
        // 1. Find the local entry
        let entry = {
            let entries = self.local_entries.lock().unwrap_or_else(|e| e.into_inner());
            entries.iter().find(|e| e.id == entry_id).cloned()
        };

        let entry = match entry {
            Some(e) => e,
            None => return false,
        };

        // 2. Create a shared wrapper
        let shared_entry = SharedMemoryEntry {
            entry,
            synced_ms: crate::shared::timestamps::now_ts_ms() as u64,
            source_node: local_node_id(),
            sync_count: 1,
        };

        // 3. Insert into shared_entries
        //    We compute the total length without re-acquiring shared_entries
        //    to avoid a deadlock with maybe_evict_oldest.
        {
            let mut guard = self
                .shared_entries
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            guard.push_back(shared_entry);
            let shared_len = guard.len();
            drop(guard);

            let mut local = self.local_entries.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned");
                poisoned.into_inner()
            });
            let total = local.len() + shared_len;
            if total > self.max_entries {
                let to_remove = total - self.max_entries;
                for _ in 0..to_remove {
                    local.pop_front();
                }
            }
        }

        // 4. Update profile
        #[cfg(feature = "multi-users-server")]
        {
            let mut p = self.profile.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned");
                poisoned.into_inner()
            });
            p.total_syncs = p.total_syncs.wrapping_add(1);
            // We would also increment per‑peer counters here in a real
            // transport implementation.
        }

        true
    }

    // ------------------------------------------------------------------
    // Maintenance
    // ------------------------------------------------------------------

    /// Remove expired entries from both local and shared stores.
    ///
    /// An entry is considered expired when `created_ms + ttl_ms < now` and
    /// `ttl_ms > 0`.
    pub fn prune_expired(&self) {
        let now = crate::shared::timestamps::now_ts_ms() as u64;
        let mut pruned = 0u64;

        // Prune local entries
        let mut local = self.local_entries.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned");
            poisoned.into_inner()
        });
        let before = local.len();
        local.retain(|e| e.ttl_ms == 0 || e.created_ms + e.ttl_ms >= now);
        pruned += (before - local.len()) as u64;
        let mut p = self.profile.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned");
            poisoned.into_inner()
        });
        p.local_entries = local.len() as u32;
        drop(p);
        drop(local);

        // Prune shared entries
        let mut shared = self.shared_entries.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned");
            poisoned.into_inner()
        });
        let before = shared.len();
        shared.retain(|se| se.entry.ttl_ms == 0 || se.entry.created_ms + se.entry.ttl_ms >= now);
        pruned += (before - shared.len()) as u64;
        let mut p = self.profile.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned");
            poisoned.into_inner()
        });
        p.shared_entries = shared.len() as u32;
        drop(p);
        drop(shared);

        if pruned > 0 {
            let mut p = self.profile.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned");
                poisoned.into_inner()
            });
            p.entries_pruned = p.entries_pruned.wrapping_add(pruned);
        }
    }

    // ------------------------------------------------------------------
    // Profile
    // ------------------------------------------------------------------

    /// Return a snapshot of the current profile / metrics.
    pub fn profile(&self) -> DistributedMemoryBusProfile {
        let mut p = self
            .profile
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();

        // Refresh live counters from actual data structures
        {
            let local = self.local_entries.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned");
                poisoned.into_inner()
            });
            p.local_entries = local.len() as u32;
        }
        if let Ok(peers) = self.remote_peers.read() {
            p.remote_peers = peers.len() as u32;
        }
        {
            let shared = self.shared_entries.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned");
                poisoned.into_inner()
            });
            p.shared_entries = shared.len() as u32;
        }

        p
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    /// Evict the oldest entries (by insertion order = creation order) from the
    /// combined local + shared stores until we are at or under `max_entries`.
    ///
    /// Eviction only happens from the **local** store; shared entries are
    /// treated as higher priority.
    fn maybe_evict_oldest(
        local: &mut VecDeque<MemoryBusEntry>,
        shared_len: usize,
        max_entries: usize,
    ) {
        let total = local.len() + shared_len;
        if total <= max_entries {
            return;
        }
        let to_remove = total - max_entries;
        for _ in 0..to_remove {
            local.pop_front();
        }
    }

    // ------------------------------------------------------------------
    // Transport layer
    // ------------------------------------------------------------------

    /// Start the background transport sync thread.
    ///
    /// This spawns a thread that periodically serialises local entries and
    /// pushes them to all known peers via the HTTP transport.
    #[cfg(feature = "multi-users-server")]
    pub fn start_transport(&self, config: MemoryTransportConfig) -> anyhow::Result<()> {
        if self.transport_running.load(Ordering::SeqCst) {
            anyhow::bail!("{}", tf("error.transport_already_running", &[]));
        }

        // Store config
        {
            let mut cfg = self
                .transport_config
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *cfg = Some(config.clone());
        }

        self.transport_running.store(true, Ordering::SeqCst);

        // Clone Arcs for the background thread
        let running = Arc::clone(&self.transport_running);
        let transport_config = Arc::clone(&self.transport_config);
        let local_entries = Arc::clone(&self.local_entries);
        let remote_peers = Arc::clone(&self.remote_peers);
        let profile = Arc::clone(&self.profile);
        let stats = Arc::clone(&self.transport_stats);

        let handle = thread::Builder::new()
            .name("dmb-transport".into())
            .spawn(move || {
                let interval = {
                    let cfg = transport_config.lock().unwrap_or_else(|e| e.into_inner());
                    cfg.as_ref().map(|c| c.sync_interval_ms).unwrap_or(30_000)
                };

                while running.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(interval));

                    if !running.load(Ordering::SeqCst) {
                        break;
                    }

                    // Perform sync
                    let cfg = transport_config.lock().unwrap_or_else(|e| e.into_inner());
                    let config = cfg.as_ref().cloned().unwrap_or_default();
                    let result =
                        Self::do_sync(&local_entries, &remote_peers, &profile, &stats, &config);

                    // Update profile with transport state
                    let mut p = profile.lock().unwrap_or_else(|poisoned| {
                        tracing::warn!("lock poisoned");
                        poisoned.into_inner()
                    });
                    let peers_ok = remote_peers.read().map(|r| r.len() as u32).unwrap_or(0);
                    p.transport_running = true;
                    p.transport_peers_reachable = peers_ok;

                    if let Err(e) = result {
                        tracing::warn!("[dmb-transport] Sync error: {}", e);
                        let mut s = stats.lock().unwrap_or_else(|poisoned| {
                            tracing::warn!("lock poisoned");
                            poisoned.into_inner()
                        });
                        s.total_errors = s.total_errors.wrapping_add(1);
                        s.last_sync_status = SyncStatus::Failed(e.to_string());
                    }
                }
            })?;

        *self.sync_thread.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);

        // Update profile
        let mut p = self.profile.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned");
            poisoned.into_inner()
        });
        p.transport_running = true;

        Ok(())
    }

    /// Single‑node fallback (multi‑user feature not enabled)
    #[cfg(not(feature = "multi-users-server"))]
    pub fn start_transport(&self, _config: MemoryTransportConfig) -> anyhow::Result<()> {
        anyhow::bail!("{}", tf("error.transport_single_node", &[]));
    }

    /// Stop the background transport sync thread.
    #[cfg(feature = "multi-users-server")]
    pub fn stop_transport(&self) -> anyhow::Result<()> {
        if !self.transport_running.load(Ordering::SeqCst) {
            anyhow::bail!("{}", tf("error.transport_not_running", &[]));
        }

        self.transport_running.store(false, Ordering::SeqCst);

        // Join the background thread
        if let Some(handle) = self
            .sync_thread
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            let _ = handle.join();
        }

        // Update profile
        let mut p = self.profile.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned");
            poisoned.into_inner()
        });
        p.transport_running = false;

        Ok(())
    }

    /// Single‑node fallback (multi‑user feature not enabled)
    #[cfg(not(feature = "multi-users-server"))]
    pub fn stop_transport(&self) -> anyhow::Result<()> {
        anyhow::bail!("{}", tf("error.transport_single_node", &[]));
    }

    /// Trigger an immediate sync operation.
    ///
    /// Returns the [`SyncStatus`] of the operation.
    #[cfg(feature = "multi-users-server")]
    pub fn sync_now(&self) -> anyhow::Result<SyncStatus> {
        let config = self
            .transport_config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_default();
        Self::do_sync(
            &self.local_entries,
            &self.remote_peers,
            &self.profile,
            &self.transport_stats,
            &config,
        )?;

        let status = self
            .transport_stats
            .lock()
            .map(|s| s.last_sync_status.clone())
            .unwrap_or(SyncStatus::Idle);

        Ok(status)
    }

    /// Single‑node fallback (multi‑user feature not enabled)
    #[cfg(not(feature = "multi-users-server"))]
    pub fn sync_now(&self) -> anyhow::Result<SyncStatus> {
        anyhow::bail!("Transport is not available in single-node mode");
    }

    /// Return a snapshot of the current transport statistics.
    pub fn transport_stats(&self) -> TransportStats {
        self.transport_stats
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    /// Ingest shared entries received from a remote peer (JSON payload).
    ///
    /// Deserialises the JSON payload and stores the entries as shared entries
    /// on this node. This is the receiving side of the HTTP transport.
    #[cfg(feature = "multi-users-server")]
    pub fn ingest_shared(&self, entries_json: &str) -> anyhow::Result<usize> {
        let entries: Vec<MemoryBusEntry> = serde_json::from_str(entries_json)?;

        let count = entries.len();
        let now = crate::shared::timestamps::now_ts_ms() as u64;

        let mut guard = self
            .shared_entries
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for entry in entries {
            let shared = SharedMemoryEntry {
                entry,
                synced_ms: now,
                source_node: "remote".to_string(),
                sync_count: 1,
            };
            guard.push_back(shared);
        }
        let shared_len = guard.len();
        drop(guard);

        // Evict from local if over capacity
        let mut local = self.local_entries.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned");
            poisoned.into_inner()
        });
        let total = local.len() + shared_len;
        if total > self.max_entries {
            let to_remove = total - self.max_entries;
            for _ in 0..to_remove {
                local.pop_front();
            }
        }
        drop(local);

        // Update profile
        let mut p = self.profile.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned");
            poisoned.into_inner()
        });
        p.shared_entries = shared_len as u32;
        p.total_syncs = p.total_syncs.wrapping_add(1);

        Ok(count)
    }

    /// Single‑node fallback (multi‑user feature not enabled)
    #[cfg(not(feature = "multi-users-server"))]
    pub fn ingest_shared(&self, _entries_json: &str) -> anyhow::Result<usize> {
        anyhow::bail!("ingest_shared is not available in single-node mode");
    }

    // ------------------------------------------------------------------
    // Internal transport helpers
    // ------------------------------------------------------------------

    /// Perform a single sync cycle: collect local entries and push them
    /// to all known peers over real HTTP (JSON-RPC `memory.ingest` against
    /// each peer's `/rpc` endpoint).
    #[cfg(feature = "multi-users-server")]
    fn do_sync(
        local_entries: &Arc<Mutex<VecDeque<MemoryBusEntry>>>,
        remote_peers: &Arc<RwLock<HashMap<String, String>>>,
        profile: &Arc<Mutex<DistributedMemoryBusProfile>>,
        stats: &Arc<Mutex<TransportStats>>,
        config: &MemoryTransportConfig,
    ) -> anyhow::Result<SyncStatus> {
        let start = Instant::now();

        // Collect local entries that haven't been synced yet. The deque is
        // drained so each entry is sent exactly once: entries added while we
        // are sending stay queued for the NEXT cycle (incremental sync —
        // previously the whole deque was re-sent on every 30s cycle).
        let entries_to_sync: Vec<MemoryBusEntry> = {
            let mut guard = local_entries.lock().unwrap_or_else(|e| e.into_inner());
            guard.drain(..).collect()
        };

        if entries_to_sync.is_empty() {
            let mut s = stats.lock().unwrap_or_else(|e| e.into_inner());
            s.last_sync_status = SyncStatus::Idle;
            return Ok(SyncStatus::Idle);
        }

        // Serialise all entries to JSON
        let payload = serde_json::to_string(&json!({ "entries": entries_to_sync }))?;
        let payload_bytes = payload.len();

        // Get current peers
        let peers: HashMap<String, String> =
            remote_peers.read().map(|r| r.clone()).unwrap_or_default();

        if peers.is_empty() {
            // No peers to sync to — still track as completed.
            let duration = start.elapsed().as_millis() as u64;
            let status = SyncStatus::Completed {
                entries_synced: 0,
                duration_ms: duration,
            };
            let mut s = stats.lock().unwrap_or_else(|e| e.into_inner());
            s.last_sync_status = status.clone();
            return Ok(status);
        }

        // Real HTTP transport: POST each batch to the peer's JSON-RPC endpoint.
        // Reuse the process-global blocking client (previously a fresh client
        // — including TLS setup — was built on every sync cycle for every peer).
        let client = match crate::shared::http_client::blocking_http_client() {
            Ok(c) => c,
            Err(e) => {
                let message = format!("failed to build dmb transport client: {}", e);
                let mut s = stats.lock().unwrap_or_else(|e| e.into_inner());
                s.total_errors = s.total_errors.wrapping_add(1);
                s.last_sync_status = SyncStatus::Failed(message.clone());
                return Err(anyhow::anyhow!("dmb sync failed: {}", message));
            }
        };

        let mut total_entries_synced = 0usize;
        let mut total_bytes_sent = 0u64;
        let mut failures: Vec<String> = Vec::new();

        // Push to all peers in parallel (previously serial: with several peers
        // each taking up to the connect timeout, a cycle could stall for
        // `peers × timeout` before failing).
        let results: Vec<(String, Result<usize, String>)> = std::thread::scope(|scope| {
            let handles: Vec<_> = peers
                .iter()
                .map(|(node_id, address)| {
                    let client = &client;
                    let endpoint = if address.contains("://") {
                        format!("{}/rpc", address.trim_end_matches('/'))
                    } else {
                        format!("http://{}/rpc", address)
                    };
                    let payload = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": format!("dmb-sync-{}", node_id),
                        "method": "memory.ingest",
                        "params": {
                            "source": node_id,
                            "entries": &entries_to_sync,
                        },
                    });
                    let entries_for_peer = entries_to_sync.clone();
                    scope.spawn(move || {
                        let mut request = client.post(&endpoint).json(&payload);
                        request = request
                            .timeout(Duration::from_millis(config.connect_timeout_ms.max(1000)));
                        if let Some(token) = &config.auth_token {
                            request = request.bearer_auth(token);
                        }
                        let result = request.send().map(|resp| {
                            if resp.status().is_success() {
                                Ok(entries_for_peer.len())
                            } else {
                                Err(format!("HTTP {}", resp.status()))
                            }
                        });
                        (
                            node_id.clone(),
                            result.unwrap_or_else(|e| Err(e.to_string())),
                        )
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("dmb peer sync thread panicked"))
                .collect()
        });

        for (node_id, result) in results {
            match result {
                Ok(count) => {
                    total_entries_synced += count;
                    total_bytes_sent += payload_bytes as u64;
                    tracing::info!(
                        "[dmb-transport] synced {} entries to peer {} @ {}",
                        count,
                        node_id,
                        peers.get(&node_id).map(String::as_str).unwrap_or("?")
                    );
                }
                Err(reason) => failures.push(format!("peer {}: {}", node_id, reason)),
            }
        }

        if total_entries_synced == 0 && !failures.is_empty() {
            // No peer acknowledged the batch — re-queue the entries so they
            // are retried on the next cycle, and report the failure instead of
            // pretending the entries were delivered.
            let mut guard = local_entries.lock().unwrap_or_else(|e| e.into_inner());
            for entry in entries_to_sync {
                guard.push_front(entry);
            }
            drop(guard);
            let message = failures.join("; ");
            {
                let mut s = stats.lock().unwrap_or_else(|e| e.into_inner());
                s.total_errors = s.total_errors.wrapping_add(1);
                s.last_sync_status = SyncStatus::Failed(message.clone());
            }
            return Err(anyhow::anyhow!("dmb sync failed: {}", message));
        }

        let duration = start.elapsed().as_millis() as u64;
        let status = SyncStatus::Completed {
            entries_synced: total_entries_synced,
            duration_ms: duration,
        };

        // Update stats
        {
            let mut s = stats.lock().unwrap_or_else(|e| e.into_inner());
            s.total_syncs_sent = s.total_syncs_sent.wrapping_add(1);
            s.total_syncs_received = s.total_syncs_received.wrapping_add(peers.len() as u64);
            s.bytes_sent = s.bytes_sent.wrapping_add(total_bytes_sent);
            s.last_sync_status = status.clone();
        }

        // Update profile
        let mut p = profile.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned");
            poisoned.into_inner()
        });
        p.total_syncs = p.total_syncs.wrapping_add(1);
        p.transport_peers_reachable = peers.len() as u32;
        p.total_bytes_synced = p.total_bytes_synced.wrapping_add(total_bytes_sent);

        Ok(status)
    }
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

/// Generate a v4 UUID string.
fn uuid_v4() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Return the local node identifier.
///
/// Under `multi-users-server` this uses `hostname`, otherwise a fixed
/// default is returned.
fn local_node_id() -> String {
    #[cfg(feature = "multi-users-server")]
    {
        std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("HOST"))
            .or_else(|_| std::env::var("COMPUTERNAME"))
            .unwrap_or_else(|_| "local-node".to_string())
    }

    #[cfg(not(feature = "multi-users-server"))]
    {
        "local-single-node".to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_bus(max: usize) -> DistributedMemoryBus {
        DistributedMemoryBus::new(max)
    }

    #[test]
    fn store_local_returns_id() {
        let bus = make_bus(100);
        let id = bus.store_local("test-key", "hello", vec![], 1.0, 60_000);
        assert!(!id.is_empty(), "expected a non-empty UUID");
    }

    #[test]
    fn find_by_key_local() {
        let bus = make_bus(100);
        bus.store_local("color", "red", vec![], 1.0, 0);
        let results = bus.find_by_key("color");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, "red");
    }

    #[test]
    fn find_by_key_no_match() {
        let bus = make_bus(100);
        let results = bus.find_by_key("nope");
        assert!(results.is_empty());
    }

    #[test]
    fn find_by_tags() {
        let bus = make_bus(100);
        bus.store_local(
            "car",
            "tesla",
            vec!["ev".to_string(), "fast".to_string()],
            0.9,
            0,
        );
        bus.store_local("bike", "canyon", vec!["slow".to_string()], 0.5, 0);

        let tag_filter = vec!["ev".to_string()];
        let results = bus.find_by_tags(&tag_filter);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "car");
    }

    #[test]
    fn find_by_tags_empty_input() {
        let bus = make_bus(100);
        bus.store_local("a", "1", vec!["x".to_string()], 1.0, 0);
        let results = bus.find_by_tags(&[]);
        assert!(results.is_empty());
    }

    #[test]
    fn register_and_unregister_peer() {
        let bus = make_bus(100);
        bus.register_peer("node-alpha", "10.0.0.1:9000");
        // Under local, register_peer is a no-op, so peers may be empty.
        // Under multi-users-server, the peer should be registered.
        let count = bus.peers().len();
        assert!(count == 0 || count == 1, "unexpected peer count: {}", count);

        bus.unregister_peer("node-alpha");
        // After unregister, peers should always be empty (no-op or actual removal)
        assert!(bus.peers().is_empty());
    }

    #[test]
    fn share_with_peers_copies_entry() {
        let bus = make_bus(100);
        let id = bus.store_local("secret", "sauce", vec![], 0.8, 0);
        assert!(bus.share_with_peers(&id));

        // Should appear in both local and shared queries
        let all = bus.find_by_key("secret");
        assert_eq!(all.len(), 2, "expected local + shared copy");
    }

    #[test]
    fn share_with_peers_unknown_id() {
        let bus = make_bus(100);
        assert!(!bus.share_with_peers("does-not-exist"));
    }

    #[test]
    fn prune_expired_removes_old_entries() {
        let bus = make_bus(100);

        // Insert an entry with a TTL that is already expired
        let past = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
            - 10_000; // 10 seconds ago

        // We cheat by storing directly so we can set created_ms
        {
            let mut local = bus.local_entries.lock().expect("lock local_entries");
            local.push_back(MemoryBusEntry {
                id: "expired-id".into(),
                node_id: local_node_id(),
                key: "expired".into(),
                value: "gone".into(),
                tags: vec![],
                confidence: 1.0,
                created_ms: past,
                ttl_ms: 1, // expired (1 ms TTL ~ 10 s ago)
            });
        }

        bus.prune_expired();
        let results = bus.find_by_key("expired");
        assert!(results.is_empty(), "expired entry should have been pruned");
    }

    #[test]
    fn prune_expired_keeps_immortal_entries() {
        let bus = make_bus(100);
        bus.store_local("keep", "me", vec![], 1.0, 0); // ttl_ms = 0 → immortal
        bus.prune_expired();
        let results = bus.find_by_key("keep");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn profile_snapshot() {
        let bus = make_bus(10);
        bus.store_local("k1", "v1", vec![], 0.5, 0);
        bus.store_local("k2", "v2", vec![], 0.5, 0);
        bus.register_peer("peer-1", "addr");

        let p = bus.profile();
        assert_eq!(p.local_entries, 2);
        assert!(p.enabled);
        // remote_peers may be 0 (local no-op) or 1 (multi-users-server)
        assert!(p.remote_peers == 0 || p.remote_peers == 1);
    }

    #[test]
    fn eviction_oldest_when_over_capacity() {
        let bus = make_bus(3); // very small capacity
        bus.store_local("a", "1", vec![], 1.0, 0);
        bus.store_local("b", "2", vec![], 1.0, 0);
        bus.store_local("c", "3", vec![], 1.0, 0);
        bus.store_local("d", "4", vec![], 1.0, 0); // should evict "a"

        let results = bus.find_by_key("a");
        assert!(
            results.is_empty(),
            "oldest entry 'a' should have been evicted"
        );
        assert_eq!(bus.find_by_key("d").len(), 1);
    }
}
