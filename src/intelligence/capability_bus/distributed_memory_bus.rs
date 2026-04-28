//! DistributedMemoryBus — Cross-node memory sharing sub-bus (BLUE38 ARCH-13)
//!
//! DistributedMemoryBus enables agents on different server instances to share
//! experience and knowledge by coordinating local memory entries with known
//! remote peers.  This implementation is in-memory only — the actual network
//! transport (gRPC, HTTP, etc.) is left to a later integration layer so that
//! we avoid heavy dependency chains at this level.
//!
//! # Protocol sketch (future)
//!
//! 1. `share_with_peers` serialises the entry and queues it for transport.
//! 2. The transport layer sends the entry to each registered peer.
//! 3. On receipt, the peer calls an internal `ingest_shared` method.
//! 4. Expired entries are pruned periodically by `prune_expired`.
//!
//! # Feature gates
//!
//! - `#[cfg(not(feature = "profile-multi-users-server"))]` — single‑node;
//!   the bus still compiles but remote‑peer operations are no‑ops (or
//!   strictly local).
//! - `#[cfg(feature = "profile-multi-users-server")]` — multi‑node; the
//!   full peer set and shared‑entry machinery is active.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
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
}

impl Default for MemoryTransportConfig {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1:0".to_string(),
            connect_timeout_ms: 5000,
            sync_interval_ms: 30000,
            max_payload_bytes: 1_048_576,
        }
    }
}

/// Represents the current status of a sync operation.
#[derive(Debug, Clone)]
pub enum SyncStatus {
    /// No sync operation is in progress.
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

impl Default for SyncStatus {
    fn default() -> Self {
        Self::Idle
    }
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
#[derive(Debug, Clone)]
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
    transport_running: Arc<AtomicBool>,
    /// Transport configuration.
    transport_config: Arc<Mutex<Option<MemoryTransportConfig>>>,
    /// Transport statistics.
    transport_stats: Arc<Mutex<TransportStats>>,
    /// Handle for the background sync thread.
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
            transport_running: Arc::new(AtomicBool::new(false)),
            transport_config: Arc::new(Mutex::new(None)),
            transport_stats: Arc::new(Mutex::new(TransportStats::default())),
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
            created_ms: now_ms(),
            ttl_ms,
        };

        // Compute shared length outside the local_entries lock scope to avoid
        // potential deadlock with maybe_evict_oldest re-locking shared_entries.
        let shared_len = self.shared_entries.lock().map(|g| g.len()).unwrap_or(0);

        let mut entries = self.local_entries.lock().expect("local_entries lock");
        entries.push_back(entry);
        Self::maybe_evict_oldest(&mut entries, shared_len, self.max_entries);

        // Update profile
        if let Ok(mut p) = self.profile.lock() {
            p.local_entries = entries.len() as u32;
        }

        id
    }

    // ------------------------------------------------------------------
    // Query
    // ------------------------------------------------------------------

    /// Find all entries (local + shared) whose `key` matches exactly.
    pub fn find_by_key(&self, key: &str) -> Vec<MemoryBusEntry> {
        let mut results = Vec::new();

        if let Ok(local) = self.local_entries.lock() {
            for e in local.iter() {
                if e.key == key {
                    results.push(e.clone());
                }
            }
        }

        if let Ok(shared) = self.shared_entries.lock() {
            for se in shared.iter() {
                if se.entry.key == key {
                    results.push(se.entry.clone());
                }
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

        if let Ok(local) = self.local_entries.lock() {
            for e in local.iter() {
                if e.tags.iter().any(|t| tags.contains(t)) {
                    results.push(e.clone());
                }
            }
        }

        if let Ok(shared) = self.shared_entries.lock() {
            for se in shared.iter() {
                if se.entry.tags.iter().any(|t| tags.contains(t)) {
                    results.push(se.entry.clone());
                }
            }
        }

        results
    }

    // ------------------------------------------------------------------
    // Peer management
    // ------------------------------------------------------------------

    /// Register (or update) a remote peer.
    #[cfg(feature = "profile-multi-users-server")]
    pub fn register_peer(&self, node_id: &str, address: &str) {
        if let Ok(mut peers) = self.remote_peers.write() {
            peers.insert(node_id.to_string(), address.to_string());
            if let Ok(mut p) = self.profile.lock() {
                p.remote_peers = peers.len() as u32;
            }
        }
    }

    /// No‑op on single‑node builds.
    #[cfg(not(feature = "profile-multi-users-server"))]
    pub fn register_peer(&self, _node_id: &str, _address: &str) {
        // Single‑node mode — nothing to register.
    }

    /// Remove a remote peer.
    #[cfg(feature = "profile-multi-users-server")]
    pub fn unregister_peer(&self, node_id: &str) {
        if let Ok(mut peers) = self.remote_peers.write() {
            peers.remove(node_id);
            if let Ok(mut p) = self.profile.lock() {
                p.remote_peers = peers.len() as u32;
            }
        }
    }

    /// No‑op on single‑node builds.
    #[cfg(not(feature = "profile-multi-users-server"))]
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
    /// In the current in‑memory implementation this simply copies the entry
    /// into `shared_entries`.  The future transport layer will fan‑out the
    /// data to each peer's network address.
    ///
    /// Returns `true` if the entry was found and shared, `false` otherwise.
    pub fn share_with_peers(&self, entry_id: &str) -> bool {
        // 1. Find the local entry
        let entry = {
            let entries = self.local_entries.lock().expect("local_entries lock");
            entries.iter().find(|e| e.id == entry_id).cloned()
        };

        let entry = match entry {
            Some(e) => e,
            None => return false,
        };

        // 2. Create a shared wrapper
        let shared_entry = SharedMemoryEntry {
            entry,
            synced_ms: now_ms(),
            source_node: local_node_id(),
            sync_count: 1,
        };

        // 3. Insert into shared_entries
        //    We compute the total length without re-acquiring shared_entries
        //    to avoid a deadlock with maybe_evict_oldest.
        {
            let mut guard = self.shared_entries.lock().expect("shared_entries lock");
            guard.push_back(shared_entry);
            let shared_len = guard.len();
            drop(guard);

            if let Ok(mut local) = self.local_entries.lock() {
                let total = local.len() + shared_len;
                if total > self.max_entries {
                    let to_remove = total - self.max_entries;
                    for _ in 0..to_remove {
                        local.pop_front();
                    }
                }
            }
        }

        // 4. Update profile
        #[cfg(feature = "profile-multi-users-server")]
        if let Ok(mut p) = self.profile.lock() {
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
        let now = now_ms();
        let mut pruned = 0u64;

        // Prune local entries
        if let Ok(mut local) = self.local_entries.lock() {
            let before = local.len();
            local.retain(|e| e.ttl_ms == 0 || e.created_ms + e.ttl_ms >= now);
            pruned += (before - local.len()) as u64;
            if let Ok(mut p) = self.profile.lock() {
                p.local_entries = local.len() as u32;
            }
        }

        // Prune shared entries
        if let Ok(mut shared) = self.shared_entries.lock() {
            let before = shared.len();
            shared
                .retain(|se| se.entry.ttl_ms == 0 || se.entry.created_ms + se.entry.ttl_ms >= now);
            pruned += (before - shared.len()) as u64;
            if let Ok(mut p) = self.profile.lock() {
                p.shared_entries = shared.len() as u32;
            }
        }

        if pruned > 0 {
            if let Ok(mut p) = self.profile.lock() {
                p.entries_pruned = p.entries_pruned.wrapping_add(pruned);
            }
        }
    }

    // ------------------------------------------------------------------
    // Profile
    // ------------------------------------------------------------------

    /// Return a snapshot of the current profile / metrics.
    pub fn profile(&self) -> DistributedMemoryBusProfile {
        let mut p = self.profile.lock().expect("profile lock").clone();

        // Refresh live counters from actual data structures
        if let Ok(local) = self.local_entries.lock() {
            p.local_entries = local.len() as u32;
        }
        if let Ok(peers) = self.remote_peers.read() {
            p.remote_peers = peers.len() as u32;
        }
        if let Ok(shared) = self.shared_entries.lock() {
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
    /// pushes them to all known peers via the simulated HTTP transport.
    #[cfg(feature = "profile-multi-users-server")]
    pub fn start_transport(&self, config: MemoryTransportConfig) -> anyhow::Result<()> {
        if self.transport_running.load(Ordering::SeqCst) {
            anyhow::bail!("Transport is already running");
        }

        // Store config
        {
            let mut cfg = self.transport_config.lock().expect("transport_config lock");
            *cfg = Some(config.clone());
        }

        self.transport_running.store(true, Ordering::SeqCst);

        // Clone Arcs for the background thread
        let running = Arc::clone(&self.transport_running);
        let transport_config = Arc::clone(&self.transport_config);
        let local_entries = Arc::clone(&self.local_entries);
        let remote_peers = Arc::clone(&self.remote_peers);
        let shared_entries = Arc::clone(&self.shared_entries);
        let profile = Arc::clone(&self.profile);
        let stats = Arc::clone(&self.transport_stats);

        let handle = thread::Builder::new()
            .name("dmb-transport".into())
            .spawn(move || {
                let interval = {
                    let cfg = transport_config.lock().expect("transport_config lock");
                    cfg.as_ref().map(|c| c.sync_interval_ms).unwrap_or(30_000)
                };

                while running.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(interval));

                    if !running.load(Ordering::SeqCst) {
                        break;
                    }

                    // Perform sync
                    let result = Self::do_sync(
                        &local_entries,
                        &remote_peers,
                        &shared_entries,
                        &profile,
                        &stats,
                    );

                    // Update profile with transport state
                    if let Ok(mut p) = profile.lock() {
                        let peers_ok = remote_peers
                            .read()
                            .map(|r| r.len() as u32)
                            .unwrap_or(0);
                        p.transport_running = true;
                        p.transport_peers_reachable = peers_ok;
                    }

                    if let Err(e) = result {
                        eprintln!("[dmb-transport] Sync error: {}", e);
                        if let Ok(mut s) = stats.lock() {
                            s.total_errors = s.total_errors.wrapping_add(1);
                            s.last_sync_status = SyncStatus::Failed(e.to_string());
                        }
                    }
                }
            })?;

        *self.sync_thread.lock().expect("sync_thread lock") = Some(handle);

        // Update profile
        if let Ok(mut p) = self.profile.lock() {
            p.transport_running = true;
        }

        Ok(())
    }

    /// Non‑multi‑user stub: transport is a no‑op.
    #[cfg(not(feature = "profile-multi-users-server"))]
    pub fn start_transport(&self, _config: MemoryTransportConfig) -> anyhow::Result<()> {
        anyhow::bail!("Transport is not available in single-node mode");
    }

    /// Stop the background transport sync thread.
    #[cfg(feature = "profile-multi-users-server")]
    pub fn stop_transport(&self) -> anyhow::Result<()> {
        if !self.transport_running.load(Ordering::SeqCst) {
            anyhow::bail!("Transport is not running");
        }

        self.transport_running.store(false, Ordering::SeqCst);

        // Join the background thread
        if let Some(handle) = self.sync_thread.lock().expect("sync_thread lock").take() {
            let _ = handle.join();
        }

        // Update profile
        if let Ok(mut p) = self.profile.lock() {
            p.transport_running = false;
        }

        Ok(())
    }

    /// Non‑multi‑user stub.
    #[cfg(not(feature = "profile-multi-users-server"))]
    pub fn stop_transport(&self) -> anyhow::Result<()> {
        anyhow::bail!("Transport is not available in single-node mode");
    }

    /// Trigger an immediate sync operation.
    ///
    /// Returns the [`SyncStatus`] of the operation.
    #[cfg(feature = "profile-multi-users-server")]
    pub fn sync_now(&self) -> anyhow::Result<SyncStatus> {
        Self::do_sync(
            &self.local_entries,
            &self.remote_peers,
            &self.shared_entries,
            &self.profile,
            &self.transport_stats,
        )?;

        let status = self
            .transport_stats
            .lock()
            .map(|s| s.last_sync_status.clone())
            .unwrap_or(SyncStatus::Idle);

        Ok(status)
    }

    /// Non‑multi‑user stub.
    #[cfg(not(feature = "profile-multi-users-server"))]
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

    /// Simulate receiving shared entries from a remote peer.
    ///
    /// Deserialises the JSON payload and stores the entries as shared entries
    /// on this node.
    #[cfg(feature = "profile-multi-users-server")]
    pub fn ingest_shared(&self, entries_json: &str) -> anyhow::Result<usize> {
        let entries: Vec<MemoryBusEntry> = serde_json::from_str(entries_json)?;

        let count = entries.len();
        let now = now_ms();

        let mut guard = self.shared_entries.lock().expect("shared_entries lock");
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
        if let Ok(mut local) = self.local_entries.lock() {
            let total = local.len() + shared_len;
            if total > self.max_entries {
                let to_remove = total - self.max_entries;
                for _ in 0..to_remove {
                    local.pop_front();
                }
            }
        }

        // Update profile
        if let Ok(mut p) = self.profile.lock() {
            p.shared_entries = shared_len as u32;
            p.total_syncs = p.total_syncs.wrapping_add(1);
        }

        Ok(count)
    }

    /// Non‑multi‑user stub.
    #[cfg(not(feature = "profile-multi-users-server"))]
    pub fn ingest_shared(&self, _entries_json: &str) -> anyhow::Result<usize> {
        anyhow::bail!("ingest_shared is not available in single-node mode");
    }

    // ------------------------------------------------------------------
    // Internal transport helpers
    // ------------------------------------------------------------------

    /// Perform a single sync cycle: collect local entries and push them
    /// to all known peers via the simulated transport.
    #[cfg(feature = "profile-multi-users-server")]
    fn do_sync(
        local_entries: &Arc<Mutex<VecDeque<MemoryBusEntry>>>,
        remote_peers: &Arc<RwLock<HashMap<String, String>>>,
        shared_entries: &Arc<Mutex<VecDeque<SharedMemoryEntry>>>,
        profile: &Arc<Mutex<DistributedMemoryBusProfile>>,
        stats: &Arc<Mutex<TransportStats>>,
    ) -> anyhow::Result<SyncStatus> {
        let start = Instant::now();

        // Collect local entries that haven't been synced yet
        let entries_to_sync: Vec<MemoryBusEntry> = {
            let guard = local_entries.lock().expect("local_entries lock");
            guard.iter().cloned().collect()
        };

        if entries_to_sync.is_empty() {
            let mut s = stats.lock().expect("stats lock");
            s.last_sync_status = SyncStatus::Idle;
            return Ok(SyncStatus::Idle);
        }

        // Serialise all entries to JSON
        let payload = serde_json::to_string(&entries_to_sync)?;
        let payload_bytes = payload.len();

        // Get current peers
        let peers: HashMap<String, String> = remote_peers
            .read()
            .map(|r| r.clone())
            .unwrap_or_default();

        if peers.is_empty() {
            // No peers to sync to — still track as completed
            let duration = start.elapsed().as_millis() as u64;
            let status = SyncStatus::Completed {
                entries_synced: 0,
                duration_ms: duration,
            };
            let mut s = stats.lock().expect("stats lock");
            s.last_sync_status = status.clone();
            return Ok(status);
        }

        let mut total_entries_synced = 0usize;
        let mut total_bytes_sent = 0u64;

        // Simulate sending to each peer
        for (node_id, address) in &peers {
            // Simulated HTTP POST of JSON payload
            eprintln!(
                "[dmb-transport] SYNC to peer {} @ {}: {} entries, {} bytes",
                node_id,
                address,
                entries_to_sync.len(),
                payload_bytes
            );

            // Simulate successful transmission: ingest the entries as if the
            // peer received them. In a real implementation this would be an
            // actual HTTP call. Here we simulate by storing locally as shared.
            {
                let mut guard = shared_entries.lock().expect("shared_entries lock");
                let now = now_ms();
                for entry in &entries_to_sync {
                    let shared = SharedMemoryEntry {
                        entry: entry.clone(),
                        synced_ms: now,
                        source_node: node_id.clone(),
                        sync_count: 1,
                    };
                    guard.push_back(shared);
                }
            }

            total_entries_synced += entries_to_sync.len();
            total_bytes_sent += payload_bytes as u64;
        }

        let duration = start.elapsed().as_millis() as u64;
        let status = SyncStatus::Completed {
            entries_synced: total_entries_synced,
            duration_ms: duration,
        };

        // Update stats
        {
            let mut s = stats.lock().expect("stats lock");
            s.total_syncs_sent = s.total_syncs_sent.wrapping_add(1);
            s.total_syncs_received = s.total_syncs_received.wrapping_add(peers.len() as u64);
            s.bytes_sent = s.bytes_sent.wrapping_add(total_bytes_sent);
            s.last_sync_status = status.clone();
        }

        // Update profile
        if let Ok(mut p) = profile.lock() {
            p.total_syncs = p.total_syncs.wrapping_add(1);
            p.transport_peers_reachable = peers.len() as u32;
            p.total_bytes_synced = p.total_bytes_synced.wrapping_add(total_bytes_sent);
        }

        Ok(status)
    }
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

/// Generate a v4 UUID string.
///
/// Uses a simple random generator seeded from the system clock to avoid
/// pulling in the full `uuid` crate as a hard dependency.  In production you
/// may wish to replace this with `uuid::Uuid::new_v4()`.
fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    // Use the low bits of the timestamp as a crude random-ish value.
    let r0 = (nanos & 0xFFFF_FFFF_FFFF) as u64;
    let r1 = ((nanos >> 48) & 0xFFFF_FFFF_FFFF) as u64;

    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        (r0 >> 16) as u32,
        (r0 & 0xFFFF) as u16,
        ((r1 >> 12) & 0x0FFF) as u16,
        (0x8000 | ((r1 >> 4) & 0x3FFF)) as u16,
        (r1 & 0x0000_FFFF_FFFF) as u64,
    )
}

/// Current wall clock in milliseconds since Unix epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Return the local node identifier.
///
/// Under `profile-multi-users-server` this uses `hostname`, otherwise a fixed
/// placeholder is returned.
fn local_node_id() -> String {
    #[cfg(feature = "profile-multi-users-server")]
    {
        std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("HOST"))
            .unwrap_or_else(|_| "local-node".to_string())
    }

    #[cfg(not(feature = "profile-multi-users-server"))]
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
    use std::time::Duration;

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
        // Under profile-local, register_peer is a no-op, so peers may be empty.
        // Under profile-multi-users-server, the peer should be registered.
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
            let mut local = bus.local_entries.lock().unwrap();
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
        // remote_peers may be 0 (profile-local no-op) or 1 (multi-users-server)
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

    // ------------------------------------------------------------------
    // Transport tests
    // ------------------------------------------------------------------

    #[cfg(feature = "profile-multi-users-server")]
    fn make_bus_with_peers(max: usize) -> DistributedMemoryBus {
        let bus = DistributedMemoryBus::new(max);
        bus.register_peer("peer-alpha", "10.0.0.1:9001");
        bus.register_peer("peer-beta", "10.0.0.2:9002");
        bus
    }

    #[test]
    #[cfg(feature = "profile-multi-users-server")]
    fn test_transport_start_stop() {
        let bus = make_bus(100);
        let config = MemoryTransportConfig::default();

        // Start transport
        assert!(bus.start_transport(config.clone()).is_ok());
        // Starting again should fail
        assert!(bus.start_transport(config.clone()).is_err());

        // Stop transport
        assert!(bus.stop_transport().is_ok());
        // Stopping again should fail
        assert!(bus.stop_transport().is_err());
    }

    #[test]
    #[cfg(feature = "profile-multi-users-server")]
    fn test_sync_now_returns_status() {
        let bus = make_bus_with_peers(100);
        bus.store_local("alpha", "value-1", vec![], 0.9, 0);

        let status = bus.sync_now().expect("sync_now should succeed");
        match status {
            SyncStatus::Completed {
                entries_synced,
                duration_ms: _,
            } => {
                assert!(
                    entries_synced > 0,
                    "expected entries to be synced"
                );
            }
            other => panic!("expected Completed status, got {:?}", other),
        }
    }

    #[test]
    #[cfg(feature = "profile-multi-users-server")]
    fn test_ingest_shared_entries() {
        let bus = make_bus(100);

        let entries = vec![
            MemoryBusEntry {
                id: "id-1".into(),
                node_id: "remote-node".into(),
                key: "shared-key".into(),
                value: "shared-val".into(),
                tags: vec!["tag-a".into()],
                confidence: 0.85,
                created_ms: 1000,
                ttl_ms: 0,
            },
        ];

        let json = serde_json::to_string(&entries).unwrap();
        let count = bus.ingest_shared(&json).expect("ingest_shared should succeed");
        assert_eq!(count, 1, "expected 1 ingested entry");

        let results = bus.find_by_key("shared-key");
        assert_eq!(results.len(), 1, "should find the ingested entry");
        assert_eq!(results[0].value, "shared-val");
    }

    #[test]
    #[cfg(feature = "profile-multi-users-server")]
    fn test_ingest_shared_invalid_json() {
        let bus = make_bus(100);
        let result = bus.ingest_shared("this is not valid json");
        assert!(result.is_err(), "invalid JSON should return an error");
    }

    #[test]
    fn test_transport_config_defaults() {
        let config = MemoryTransportConfig::default();
        assert_eq!(config.listen_addr, "127.0.0.1:0");
        assert_eq!(config.connect_timeout_ms, 5000);
        assert_eq!(config.sync_interval_ms, 30000);
        assert_eq!(config.max_payload_bytes, 1_048_576);
    }

    #[test]
    #[cfg(feature = "profile-multi-users-server")]
    fn test_transport_stats() {
        let bus = make_bus_with_peers(100);
        bus.store_local("stats-key", "stats-val", vec![], 0.7, 0);

        // Initial stats
        let stats = bus.transport_stats();
        assert_eq!(stats.total_syncs_sent, 0);
        assert_eq!(stats.total_errors, 0);

        // After sync
        let _ = bus.sync_now();
        let stats = bus.transport_stats();
        assert_eq!(stats.total_syncs_sent, 1, "should have one sync sent");
        assert!(stats.bytes_sent > 0, "should have sent some bytes");
    }

    #[test]
    #[cfg(feature = "profile-multi-users-server")]
    fn test_ingested_entries_are_findable() {
        let bus = make_bus(100);

        // Ingest an entry via simulated peer sync
        let entries = vec![
            MemoryBusEntry {
                id: "remote-id-1".into(),
                node_id: "peer-node".into(),
                key: "remote-findable".into(),
                value: "found-me".into(),
                tags: vec!["discoverable".into()],
                confidence: 0.95,
                created_ms: 2000,
                ttl_ms: 0,
            },
        ];

        let json = serde_json::to_string(&entries).unwrap();
        bus.ingest_shared(&json).unwrap();

        // Query by key
        let by_key = bus.find_by_key("remote-findable");
        assert_eq!(by_key.len(), 1, "should find by key");
        assert_eq!(by_key[0].value, "found-me");

        // Query by tags
        let by_tags = bus.find_by_tags(&["discoverable".to_string()]);
        assert_eq!(by_tags.len(), 1, "should find by tags");
        assert_eq!(by_tags[0].key, "remote-findable");
    }

    #[test]
    #[cfg(feature = "profile-multi-users-server")]
    fn test_transport_profile_includes_transport() {
        let bus = make_bus(100);
        let config = MemoryTransportConfig::default();

        // Start transport
        bus.start_transport(config).unwrap();

        let p = bus.profile();
        assert!(p.transport_running, "transport should be running");

        // Stop and verify
        bus.stop_transport().unwrap();
        let p = bus.profile();
        assert!(!p.transport_running, "transport should be stopped");
    }

    /// Test that sync_now with no entries returns Idle status.
    #[test]
    #[cfg(feature = "profile-multi-users-server")]
    fn test_sync_now_empty_entries() {
        let bus = make_bus_with_peers(100);
        // No entries stored
        let status = bus.sync_now().expect("sync_now should succeed");
        match status {
            SyncStatus::Idle => {} // expected
            other => panic!("expected Idle status with no entries, got {:?}", other),
        }
    }

    /// Test that sync_now with no peers returns Completed with zero entries.
    #[test]
    #[cfg(feature = "profile-multi-users-server")]
    fn test_sync_now_no_peers() {
        let bus = make_bus(100); // no peers registered
        bus.store_local("orphan", "alone", vec![], 0.5, 0);

        let status = bus.sync_now().expect("sync_now should succeed");
        match status {
            SyncStatus::Completed {
                entries_synced,
                duration_ms: _,
            } => {
                assert_eq!(
                    entries_synced, 0,
                    "no peers means no entries should be synced"
                );
            }
            other => panic!("expected Completed status, got {:?}", other),
        }
    }
}
