//! GAP-B52-08: Federated Node Discovery
//!
//! Provides a trait-based abstraction for discovering peers in the federated
//! learning network, along with a static configuration-based discovery and
//! a heartbeat-based health tracker.
//!
//! # CapabilityBus integration
//!
//! This module is **standalone** — it implements full P2P discovery logic but
//! never calls the `CapabilityBus`. To wire it in, register discovered peers
//! as capability route targets in the bus, and use bus reputation scores to
//! influence peer selection or heartbeat timeout thresholds.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::intelligence::reinforcement::federated_transport::{
    FederatedTransport, NodeRole, PeerInfo,
};

// ── NodeInfo ───────────────────────────────────────────────────────────────

/// Information about a discovered node in the federated network.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    /// Unique node identifier.
    pub id: String,
    /// Network address (e.g. `"10.0.0.1:50051"`).
    pub addr: String,
    /// Role this node fulfills.
    pub role: NodeRole,
    /// Arbitrary capability key-value pairs.
    pub capabilities: HashMap<String, String>,
    /// Whether this node is currently considered online.
    pub online: bool,
    /// Timestamp (ms since epoch) of the last successful heartbeat.
    pub last_heartbeat_ms: u64,
}

impl NodeInfo {
    /// Create a new `NodeInfo` from a `PeerInfo` and an initial online state.
    pub fn from_peer(peer: &PeerInfo, online: bool) -> Self {
        Self {
            id: peer.id.clone(),
            addr: peer.addr.clone(),
            role: peer.role,
            capabilities: peer.capabilities.clone(),
            online,
            last_heartbeat_ms: 0,
        }
    }
}

// ── NodeDiscovery trait ────────────────────────────────────────────────────

/// Abstract interface for discovering peers in the federated network.
///
/// Implementations can use static configuration, a registration service,
/// DNS-SD, or any other mechanism.
#[async_trait::async_trait]
pub trait NodeDiscovery: Send + Sync + std::fmt::Debug {
    /// Register this node with the discovery mechanism.
    ///
    /// Returns an error if registration fails (e.g. duplicate node id).
    async fn register(&self, node: &NodeInfo) -> Result<()>;

    /// Discover all currently known nodes.
    async fn discover(&self) -> Result<Vec<NodeInfo>>;

    /// Watch for node changes via a receiver channel.
    /// Returns a receiver that yields updated node lists.
    async fn watch(&self) -> Result<tokio::sync::watch::Receiver<Vec<NodeInfo>>>;
}

// ── StaticDiscovery ────────────────────────────────────────────────────────

/// A static discovery implementation that reads peers from a fixed
/// configuration. Nodes are always considered online unless explicitly
/// marked offline by a heartbeat timeout.
///
/// This is the simplest discovery strategy, suitable for deployments
/// with a known, unchanging set of peers.
#[derive(Debug, Clone)]
pub struct StaticDiscovery {
    /// The known nodes, keyed by node id.
    nodes: Arc<RwLock<HashMap<String, NodeInfo>>>,
    /// Watcher state: a watch channel that broadcasts the latest node list.
    tx: tokio::sync::watch::Sender<Vec<NodeInfo>>,
}

impl StaticDiscovery {
    /// Create a new `StaticDiscovery` from a list of known peers.
    ///
    /// All peers start as online.
    pub fn new(peers: &[PeerInfo]) -> Self {
        let mut nodes = HashMap::new();
        for peer in peers {
            nodes.insert(peer.id.clone(), NodeInfo::from_peer(peer, true));
        }
        let node_list: Vec<NodeInfo> = nodes.values().cloned().collect();
        let (tx, _rx) = tokio::sync::watch::channel(node_list);

        StaticDiscovery {
            nodes: Arc::new(RwLock::new(nodes)),
            tx,
        }
    }

    /// Create a `StaticDiscovery` from the `FEDERATED_PEERS` env var.
    pub fn from_env() -> Self {
        let peers =
            crate::intelligence::reinforcement::federated_transport::parse_federated_peers_env();
        Self::new(&peers)
    }

    /// Manually mark a node as online or offline.
    pub async fn set_online(&self, node_id: &str, online: bool) {
        let mut nodes = self.nodes.write().await;
        if let Some(node) = nodes.get_mut(node_id) {
            node.online = online;
            if online {
                node.last_heartbeat_ms = elapsed_ms();
            }
            self.broadcast(&nodes).await;
        }
    }

    /// Update the heartbeat timestamp for a node.
    pub async fn record_heartbeat(&self, node_id: &str) {
        let mut nodes = self.nodes.write().await;
        if let Some(node) = nodes.get_mut(node_id) {
            node.online = true;
            node.last_heartbeat_ms = elapsed_ms();
            self.broadcast(&nodes).await;
        }
    }

    /// Send the current node list to all watchers.
    async fn broadcast(&self, nodes: &HashMap<String, NodeInfo>) {
        let list: Vec<NodeInfo> = nodes.values().cloned().collect();
        let _ = self.tx.send(list);
    }
}

#[async_trait::async_trait]
impl NodeDiscovery for StaticDiscovery {
    async fn register(&self, node: &NodeInfo) -> Result<()> {
        let mut nodes = self.nodes.write().await;
        if nodes.contains_key(&node.id) {
            anyhow::bail!("node '{}' is already registered", node.id);
        }
        let info = NodeInfo {
            last_heartbeat_ms: elapsed_ms(),
            ..node.clone()
        };
        nodes.insert(node.id.clone(), info);
        self.broadcast(&nodes).await;
        info!("StaticDiscovery: registered node {}", node.id);
        Ok(())
    }

    async fn discover(&self) -> Result<Vec<NodeInfo>> {
        let nodes = self.nodes.read().await;
        let mut list: Vec<NodeInfo> = nodes.values().cloned().collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(list)
    }

    async fn watch(&self) -> Result<tokio::sync::watch::Receiver<Vec<NodeInfo>>> {
        Ok(self.tx.subscribe())
    }
}

// ── Heartbeat ──────────────────────────────────────────────────────────────

/// Default heartbeat interval (10 seconds).
pub const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

/// Default timeout after which a node is considered offline (30 seconds).
pub const DEFAULT_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(30);

/// Callback invoked by the heartbeat when a node's online status changes.
pub type StatusChangeCallback = Arc<dyn Fn(String, bool) -> Result<()> + Send + Sync>;

/// A heartbeat monitor that periodically checks peer health and marks
/// unresponsive nodes as offline.
///
/// The monitor runs in a background tokio task spawned by [`Heartbeat::start`].
///
/// # Online status tracking
///
/// The heartbeat tracks its own per-node heartbeat timestamps internally.
/// To have the discovery reflect online/offline status, provide a
/// [`StatusChangeCallback`] via [`Heartbeat::with_status_callback`], or
/// use [`HeartbeatConfig`] with a [`StaticDiscovery`].
pub struct Heartbeat {
    /// The discovery mechanism to update.
    discovery: Arc<dyn NodeDiscovery>,
    /// The transport used to perform health checks.
    transport: Arc<dyn FederatedTransport>,
    /// Interval between heartbeat checks.
    interval: Duration,
    /// Timeout after which a node is considered offline.
    timeout: Duration,
    /// Shared stop signal.
    stopped: Arc<AtomicBool>,
    /// Optional callback invoked when a node transitions online/offline.
    on_status_change: Option<StatusChangeCallback>,
}

impl std::fmt::Debug for Heartbeat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Heartbeat")
            .field("discovery", &self.discovery)
            .field("transport", &self.transport)
            .field("interval", &self.interval)
            .field("timeout", &self.timeout)
            .field("stopped", &self.stopped)
            .field(
                "on_status_change",
                &self.on_status_change.as_ref().map(|_| "<closure>"),
            )
            .finish()
    }
}

impl Heartbeat {
    /// Create a new heartbeat monitor.
    pub fn new(discovery: Arc<dyn NodeDiscovery>, transport: Arc<dyn FederatedTransport>) -> Self {
        Self {
            discovery,
            transport,
            interval: DEFAULT_HEARTBEAT_INTERVAL,
            timeout: DEFAULT_HEARTBEAT_TIMEOUT,
            stopped: Arc::new(AtomicBool::new(false)),
            on_status_change: None,
        }
    }

    /// Create a heartbeat monitor with custom timing parameters.
    pub fn with_params(
        discovery: Arc<dyn NodeDiscovery>,
        transport: Arc<dyn FederatedTransport>,
        interval: Duration,
        timeout: Duration,
    ) -> Self {
        Self {
            discovery,
            transport,
            interval,
            timeout,
            stopped: Arc::new(AtomicBool::new(false)),
            on_status_change: None,
        }
    }

    /// Attach a callback that is invoked when a node transitions online
    /// or offline.
    pub fn with_status_callback(mut self, callback: StatusChangeCallback) -> Self {
        self.on_status_change = Some(callback);
        self
    }

    /// Convenience: attach a `StaticDiscovery` as the status updater.
    ///
    /// This registers a callback that calls `record_heartbeat` and
    /// `set_online` on the discovery when nodes change state.
    pub fn with_static_discovery(mut self, discovery: Arc<StaticDiscovery>) -> Self {
        self.on_status_change = Some(Arc::new(move |node_id, online| {
            let d = discovery.clone();
            let id = node_id.clone();
            tokio::spawn(async move {
                if online {
                    d.record_heartbeat(&id).await;
                } else {
                    d.set_online(&id, false).await;
                }
            });
            Ok(())
        }));
        self
    }

    /// Get a reference to the stop signal so callers can signal shutdown.
    pub fn stopper(&self) -> Arc<AtomicBool> {
        self.stopped.clone()
    }

    /// Start the heartbeat monitoring loop in a background task.
    ///
    /// Returns a handle to the spawned task. The caller must keep the
    /// handle alive for the monitor to run.
    pub fn start(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!(
                "Heartbeat monitor started: interval={:?}, timeout={:?}",
                self.interval, self.timeout
            );

            // Track the last successful heartbeat per node.
            let mut last_heartbeats: HashMap<String, Instant> = HashMap::new();

            loop {
                if self.stopped.load(Ordering::Relaxed) {
                    info!("Heartbeat monitor stopped");
                    break;
                }

                // Discover all nodes.
                let nodes = match self.discovery.discover().await {
                    Ok(nodes) => nodes,
                    Err(e) => {
                        warn!("Heartbeat: discovery failed: {}", e);
                        tokio::time::sleep(self.interval).await;
                        continue;
                    }
                };

                let now = Instant::now();

                for node in &nodes {
                    let peer = PeerInfo::new(&node.id, &node.addr, node.role);
                    match self.transport.health_check(&peer).await {
                        Ok(true) => {
                            debug!("Heartbeat: {} is healthy", node.id);

                            let was_offline = !last_heartbeats.contains_key(&node.id);
                            last_heartbeats.insert(node.id.clone(), now);

                            // Notify status callback if node just came online.
                            if was_offline {
                                if let Some(ref cb) = self.on_status_change {
                                    let _ = cb(node.id.clone(), true);
                                }
                            }
                        }
                        Ok(false) | Err(_) => {
                            // Node did not respond. Check timeout.
                            let last = last_heartbeats.get(&node.id).copied().unwrap_or(now);

                            if now.duration_since(last) >= self.timeout {
                                warn!(
                                    "Heartbeat: {} exceeded timeout ({:?}), marking offline",
                                    node.id, self.timeout
                                );

                                last_heartbeats.remove(&node.id);

                                // Notify status callback.
                                if let Some(ref cb) = self.on_status_change {
                                    let _ = cb(node.id.clone(), false);
                                }
                            } else {
                                debug!("Heartbeat: {} missed check, but within timeout", node.id);
                            }
                        }
                    }
                }

                tokio::time::sleep(self.interval).await;
            }
        })
    }
}

// ── Utility ────────────────────────────────────────────────────────────────

fn elapsed_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intelligence::reinforcement::federated_transport::InProcessTransport;

    #[tokio::test]
    async fn test_static_discovery_empty() {
        let discovery = StaticDiscovery::new(&[]);
        let nodes = discovery.discover().await.unwrap();
        assert!(nodes.is_empty());
    }

    #[tokio::test]
    async fn test_static_discovery_with_peers() {
        let peers = vec![
            PeerInfo::new("alpha", "10.0.0.1:50051", NodeRole::Coordinator),
            PeerInfo::new("beta", "10.0.0.2:50051", NodeRole::Worker),
        ];
        let discovery = StaticDiscovery::new(&peers);
        let nodes = discovery.discover().await.unwrap();
        assert_eq!(nodes.len(), 2);
        assert!(nodes.iter().any(|n| n.id == "alpha" && n.online));
        assert!(nodes.iter().any(|n| n.id == "beta" && n.online));
    }

    #[tokio::test]
    async fn test_static_discovery_register() {
        let discovery = StaticDiscovery::new(&[]);
        let node = NodeInfo::from_peer(
            &PeerInfo::new("gamma", "10.0.0.3:50051", NodeRole::Full),
            true,
        );
        discovery.register(&node).await.unwrap();
        let nodes = discovery.discover().await.unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, "gamma");
    }

    #[tokio::test]
    async fn test_static_discovery_register_duplicate_fails() {
        let peers = vec![PeerInfo::new("dup", "10.0.0.1:50051", NodeRole::Worker)];
        let discovery = StaticDiscovery::new(&peers);
        let node = NodeInfo::from_peer(
            &PeerInfo::new("dup", "10.0.0.2:50051", NodeRole::Worker),
            true,
        );
        let result = discovery.register(&node).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("already registered"));
    }

    #[tokio::test]
    async fn test_static_discovery_set_online() {
        let peers = vec![PeerInfo::new("alpha", "10.0.0.1:50051", NodeRole::Worker)];
        let discovery = StaticDiscovery::new(&peers);

        // Initially online.
        let nodes = discovery.discover().await.unwrap();
        assert!(nodes[0].online);

        // Mark offline.
        discovery.set_online("alpha", false).await;
        let nodes = discovery.discover().await.unwrap();
        assert!(!nodes[0].online);

        // Mark online again.
        discovery.set_online("alpha", true).await;
        let nodes = discovery.discover().await.unwrap();
        assert!(nodes[0].online);
    }

    #[tokio::test]
    async fn test_static_discovery_watch() {
        let peers = vec![PeerInfo::new(
            "alpha",
            "10.0.0.1:50051",
            NodeRole::Coordinator,
        )];
        let discovery = StaticDiscovery::new(&peers);

        let mut rx = discovery.watch().await.unwrap();

        // Initial list.
        let initial = rx.borrow_and_update().clone();
        assert_eq!(initial.len(), 1);

        // Register a new node and watch for update.
        let node = NodeInfo::from_peer(
            &PeerInfo::new("beta", "10.0.0.2:50051", NodeRole::Worker),
            true,
        );
        discovery.register(&node).await.unwrap();

        let updated = rx.changed().await;
        assert!(updated.is_ok());
        let nodes = rx.borrow_and_update().clone();
        assert_eq!(nodes.len(), 2);
    }

    #[tokio::test]
    async fn test_heartbeat_creation() {
        let discovery = Arc::new(StaticDiscovery::new(&[]));
        let transport = Arc::new(InProcessTransport::new());
        let heartbeat = Heartbeat::new(discovery, transport);
        assert_eq!(heartbeat.interval, DEFAULT_HEARTBEAT_INTERVAL);
        assert_eq!(heartbeat.timeout, DEFAULT_HEARTBEAT_TIMEOUT);
    }

    #[tokio::test]
    async fn test_heartbeat_with_params() {
        let discovery = Arc::new(StaticDiscovery::new(&[]));
        let transport = Arc::new(InProcessTransport::new());
        let heartbeat = Heartbeat::with_params(
            discovery,
            transport,
            Duration::from_secs(5),
            Duration::from_secs(15),
        );
        assert_eq!(heartbeat.interval, Duration::from_secs(5));
        assert_eq!(heartbeat.timeout, Duration::from_secs(15));
    }

    #[tokio::test]
    async fn test_heartbeat_stop() {
        let discovery = Arc::new(StaticDiscovery::new(&[]));
        let transport = Arc::new(InProcessTransport::new());
        let heartbeat = Heartbeat::new(discovery, transport);
        let stopper = heartbeat.stopper();

        let handle = heartbeat.start();

        // Signal stop.
        stopper.store(true, Ordering::Relaxed);

        // Task should finish within a short time.
        let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(result.is_ok(), "heartbeat did not stop in time");
    }

    #[test]
    fn test_node_info_from_peer() {
        let peer = PeerInfo::new("node-1", "10.0.0.1:50051", NodeRole::Full)
            .with_capability("gpu", "A100");
        let info = NodeInfo::from_peer(&peer, true);
        assert_eq!(info.id, "node-1");
        assert_eq!(info.addr, "10.0.0.1:50051");
        assert_eq!(info.role, NodeRole::Full);
        assert!(info.online);
        assert_eq!(info.capabilities.get("gpu"), Some(&"A100".to_string()));
        assert_eq!(info.last_heartbeat_ms, 0);
    }

    #[test]
    fn test_node_info_from_peer_offline() {
        let peer = PeerInfo::new("node-2", "10.0.0.2:50051", NodeRole::Worker);
        let info = NodeInfo::from_peer(&peer, false);
        assert!(!info.online);
    }
}
