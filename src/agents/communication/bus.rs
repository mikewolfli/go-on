//! CommunicationBus — top-level bus aggregating agent communication components (BLUE70 §2.3, §10)
//!
//! The CommunicationBus is the single entry point for all agent communication
//! operations. It owns the AgentTree, AgentMessenger, and provides health reporting.
//!
//! As the 12th bus in the go-on architecture (11 core + 1 communication),
//! it follows the Bus pattern: Builder + Profile + health endpoint.

use std::sync::Arc;
use std::sync::RwLock as SyncRwLock;
use tokio::sync::RwLock as AsyncRwLock;

use crate::agents::communication::message::AgentMessage;
use crate::agents::communication::messenger::AgentMessenger;
use crate::agents::communication::path::AgentPath;
use crate::agents::communication::tree::{AgentNodeMetadata, AgentTree};

/// CommunicationBus profile for governance.status integration.
#[derive(Debug, Clone)]
pub struct CommunicationBusProfile {
    /// Number of registered agents.
    pub registered_agents: usize,
    /// Total messages sent.
    pub messages_sent: u64,
    /// Total messages received.
    pub messages_received: u64,
    /// Number of forks created.
    pub forks_created: u64,
    /// Number of cancellations.
    pub cancellations: u64,
    /// Whether the bus is healthy.
    pub healthy: bool,
}

impl Default for CommunicationBusProfile {
    fn default() -> Self {
        Self {
            registered_agents: 0,
            messages_sent: 0,
            messages_received: 0,
            forks_created: 0,
            cancellations: 0,
            healthy: true,
        }
    }
}

/// Communication health status.
#[derive(Debug, Clone)]
pub struct CommunicationHealth {
    pub healthy: bool,
    pub agent_count: usize,
    pub message_count: u64,
    pub details: String,
}

/// CommunicationBus — agent tree-based communication system (BLUE70 §2.2).
///
/// Design:
/// - Owns AgentTree (hierarchical index) and AgentMessenger (message routing).
/// - Thread-safe via Arc<RwLock<>>.
/// - Profile for governance.status integration.
/// - Health endpoint for system monitoring.
pub struct CommunicationBus {
    /// Agent tree — hierarchical agent index.
    tree: Arc<AsyncRwLock<AgentTree>>,
    /// Agent messenger — message routing and delivery.
    messenger: AgentMessenger,
    /// Metrics counters.
    metrics: Arc<SyncRwLock<CommunicationMetrics>>,
}

#[derive(Debug, Default)]
struct CommunicationMetrics {
    messages_sent: u64,
    messages_received: u64,
    forks_created: u64,
    cancellations: u64,
    tool_calls: u64,
    tool_successes: u64,
    tool_failures: u64,
    total_duration_ms: u64,
}

impl CommunicationBus {
    /// Create a new CommunicationBus.
    pub fn new() -> Self {
        let tree = Arc::new(AsyncRwLock::new(AgentTree::new()));
        let messenger = AgentMessenger::new(tree.clone());
        Self {
            tree,
            messenger,
            metrics: Arc::new(SyncRwLock::new(CommunicationMetrics::default())),
        }
    }

    /// Get a reference to the agent tree.
    pub fn tree(&self) -> &Arc<AsyncRwLock<AgentTree>> {
        &self.tree
    }

    /// Get a reference to the agent messenger.
    pub fn messenger(&self) -> &AgentMessenger {
        &self.messenger
    }

    /// Register an agent in the tree.
    pub async fn register_agent(
        &self,
        path: &AgentPath,
        agent_name: &str,
        metadata: AgentNodeMetadata,
    ) -> Result<(), String> {
        self.tree.write().await.register(path, agent_name, metadata)
    }

    /// Send a message.
    pub async fn send_message(&self, msg: AgentMessage) -> Result<(), String> {
        let result = self.messenger.send(msg).await;
        if result.is_ok() {
            if let Ok(mut metrics) = self.metrics.write() {
                metrics.messages_sent += 1;
            }
        }
        result
    }

    /// Send with AtLeastOnce delivery.
    pub async fn send_at_least_once(&self, msg: AgentMessage) -> Result<(), String> {
        let result = self.messenger.send_at_least_once(msg).await;
        if result.is_ok() {
            if let Ok(mut metrics) = self.metrics.write() {
                metrics.messages_sent += 1;
            }
        }
        result
    }

    /// Cancel a sub-tree.
    pub async fn cancel_subtree(&self, path: &AgentPath, reason: &str) {
        self.messenger.cancel_subtree(path, reason).await;
        if let Ok(mut metrics) = self.metrics.write() {
            metrics.cancellations += 1;
        }
    }

    /// Remove a sub-tree from the agent tree.
    pub async fn remove_subtree(&self, path: &AgentPath) -> Vec<AgentPath> {
        self.tree.write().await.remove_subtree(path)
    }

    /// Record a fork (for metrics).
    pub fn record_fork(&self) {
        if let Ok(mut metrics) = self.metrics.write() {
            metrics.forks_created += 1;
        }
    }

    /// Record a message received (for metrics).
    pub fn record_message_received(&self) {
        if let Ok(mut metrics) = self.metrics.write() {
            metrics.messages_received += 1;
        }
    }

    /// Record metrics for tool execution.
    pub fn record_metrics(&self, tool_name: &str, duration_ms: u64, success: bool) {
        if let Ok(mut metrics) = self.metrics.try_write() {
            metrics.tool_calls += 1;
            metrics.total_duration_ms = metrics.total_duration_ms.wrapping_add(duration_ms);
            if success {
                metrics.tool_successes += 1;
            } else {
                metrics.tool_failures += 1;
            }
            // Track spawn_agent tool calls specifically in messages_sent counter
            if tool_name == "spawn_agent" {
                metrics.messages_sent += 1;
            }
        }
    }

    /// Get the current profile (for governance.status).
    pub async fn profile(&self) -> CommunicationBusProfile {
        // Read metrics first and drop the guard before any await point
        let (messages_sent, messages_received, forks_created, cancellations) = {
            let m = match self.metrics.read() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    tracing::warn!("CommunicationBus metrics lock poisoned, recovering");
                    poisoned.into_inner()
                }
            };
            (m.messages_sent, m.messages_received, m.forks_created, m.cancellations)
        };
        let registered_agents = self.tree.read().await.len();
        CommunicationBusProfile {
            registered_agents,
            messages_sent,
            messages_received,
            forks_created,
            cancellations,
            healthy: true,
        }
    }

    /// Get health status.
    pub async fn health(&self) -> CommunicationHealth {
        let profile = self.profile().await;
        let details = format!(
            "agents={}, sent={}, received={}, forks={}, cancels={}",
            profile.registered_agents,
            profile.messages_sent,
            profile.messages_received,
            profile.forks_created,
            profile.cancellations,
        );
        CommunicationHealth {
            healthy: true,
            agent_count: profile.registered_agents,
            message_count: profile.messages_sent + profile.messages_received,
            details,
        }
    }
}

impl Default for CommunicationBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::communication::message::{AgentMessage, AgentTarget};
    use crate::agents::communication::path::AgentPath;

    fn make_path(s: &str) -> AgentPath {
        AgentPath::parse(s).unwrap()
    }

    #[tokio::test]
    async fn test_bus_register_agent() {
        let bus = CommunicationBus::new();
        let path = make_path("root");
        bus.register_agent(&path, "main", AgentNodeMetadata::new())
            .await
            .unwrap();

        let tree = bus.tree().read().await;
        assert!(tree.resolve(&path).is_some());
    }

    #[tokio::test]
    async fn test_bus_send_message() {
        let bus = CommunicationBus::new();
        bus.register_agent(&make_path("root"), "main", AgentNodeMetadata::new())
            .await
            .unwrap();
        bus.register_agent(&make_path("root/a"), "a", AgentNodeMetadata::new())
            .await
            .unwrap();

        let msg = AgentMessage::status_query(
            make_path("root/a"),
            AgentTarget::ToParent,
        );
        bus.send_message(msg).await.unwrap();

        let received = bus.messenger().recv(&make_path("root")).await;
        assert_eq!(received.len(), 1);
    }

    #[tokio::test]
    async fn test_bus_profile() {
        let bus = CommunicationBus::new();
        bus.register_agent(&make_path("root"), "main", AgentNodeMetadata::new())
            .await
            .unwrap();

        let profile = bus.profile().await;
        assert_eq!(profile.registered_agents, 1);
        assert!(profile.healthy);
    }

    #[tokio::test]
    async fn test_bus_health() {
        let bus = CommunicationBus::new();
        let health = bus.health().await;
        assert!(health.healthy);
        assert_eq!(health.agent_count, 0);
    }

    #[tokio::test]
    async fn test_bus_cancel_subtree() {
        let bus = CommunicationBus::new();
        bus.register_agent(&make_path("root"), "main", AgentNodeMetadata::new())
            .await
            .unwrap();
        bus.register_agent(&make_path("root/a"), "a", AgentNodeMetadata::new())
            .await
            .unwrap();

        bus.cancel_subtree(&make_path("root"), "test_cancel").await;

        let msgs = bus.messenger().recv(&make_path("root/a")).await;
        assert!(!msgs.is_empty());
        assert!(msgs[0].is_cancel());
    }

    #[tokio::test]
    async fn test_bus_remove_subtree() {
        let bus = CommunicationBus::new();
        bus.register_agent(&make_path("root"), "main", AgentNodeMetadata::new())
            .await
            .unwrap();
        bus.register_agent(&make_path("root/a"), "a", AgentNodeMetadata::new())
            .await
            .unwrap();
        bus.register_agent(&make_path("root/a/a1"), "a1", AgentNodeMetadata::new())
            .await
            .unwrap();

        let removed = bus.remove_subtree(&make_path("root/a")).await;
        assert_eq!(removed.len(), 2); // a + a1
    }

    #[tokio::test]
    async fn test_bus_record_fork() {
        let bus = CommunicationBus::new();
        bus.record_fork();
        let profile = bus.profile().await;
        assert_eq!(profile.forks_created, 1);
    }

    #[tokio::test]
    async fn test_bus_record_metrics() {
        let bus = CommunicationBus::new();
        bus.record_metrics("spawn_agent", 100, true);
        bus.record_metrics("spawn_agent", 200, false);
        let profile = bus.profile().await;
        assert!(profile.messages_sent >= 2);
    }
}
