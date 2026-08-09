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

use crate::agents::communication::forker::ContextForker;
use crate::agents::communication::governor::ExecutionGovernor;
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
    /// Number of forks created.
    pub forks_created: u64,
    /// Whether the bus is healthy.
    pub healthy: bool,
}

impl Default for CommunicationBusProfile {
    fn default() -> Self {
        Self {
            registered_agents: 0,
            messages_sent: 0,
            forks_created: 0,
            healthy: true,
        }
    }
}

/// CommunicationBus — agent tree-based communication system (BLUE70 §2.2).
///
/// Design:
/// - Owns AgentTree (hierarchical index) and AgentMessenger (message routing).
/// - ContextForker for parent-to-child context inheritance (§6).
/// - ExecutionGovernor for budget-aware execution control (§7).
/// - Thread-safe via Arc<RwLock<>>.
/// - Profile for governance.status integration.
/// - Health endpoint for system monitoring.
pub struct CommunicationBus {
    /// Agent tree — hierarchical agent index.
    tree: Arc<AsyncRwLock<AgentTree>>,
    /// Agent messenger — message routing and delivery.
    messenger: Arc<AgentMessenger>,
    /// Context forker — parent-to-child context inheritance (BLUE70 §6).
    forker: ContextForker,
    /// Execution governor — budget-aware execution control (BLUE70 §7).
    governor: ExecutionGovernor,
    /// Metrics counters.
    metrics: Arc<SyncRwLock<CommunicationMetrics>>,
}

#[derive(Debug, Default)]
struct CommunicationMetrics {
    messages_sent: u64,
    messages_failed: u64,
    forks_created: u64,
}

impl CommunicationBus {
    /// Create a new CommunicationBus.
    pub fn new() -> Self {
        let tree = Arc::new(AsyncRwLock::new(AgentTree::new()));
        let messenger_inner = AgentMessenger::new(tree.clone());
        let messenger = Arc::new(messenger_inner);
        let governor = ExecutionGovernor::new(tree.clone());
        Self {
            tree,
            messenger,
            forker: ContextForker::new(),
            governor,
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

    /// Get a reference to the context forker (BLUE70 §6).
    pub fn forker(&self) -> &ContextForker {
        &self.forker
    }

    /// Get a reference to the execution governor (BLUE70 §7).
    pub fn governor(&self) -> &ExecutionGovernor {
        &self.governor
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

    /// Remove an agent (and its descendants) from the tree once it finishes
    /// executing, so the tree does not accumulate every spawned agent.
    pub async fn remove_agent(&self, path: &AgentPath) -> Vec<AgentPath> {
        self.tree.write().await.remove_subtree(path)
    }

    /// Set the lifecycle state of the node at `path` (watch-channel notify).
    pub async fn set_lifecycle(
        &self,
        path: &AgentPath,
        state: crate::agents::communication::lifecycle::AgentLifecycle,
    ) {
        if let Some(node) = self.tree.read().await.resolve(path) {
            node.set_lifecycle(state);
        }
    }

    /// Remove all communication state for a finished agent: tree node,
    /// descendants, and the messenger inbox.
    pub async fn cleanup_agent(&self, path: &AgentPath) {
        self.remove_agent(path).await;
        self.messenger.remove_inbox(path).await;
    }

    /// Send a message.
    pub async fn send_message(&self, msg: AgentMessage) -> Result<(), String> {
        let result = self.messenger.send(msg).await;
        if let Ok(mut metrics) = self.metrics.write() {
            if result.is_ok() {
                metrics.messages_sent += 1;
            } else {
                metrics.messages_failed += 1;
            }
        }
        result
    }

    /// Record a fork (for metrics).
    pub fn record_fork(&self) {
        if let Ok(mut metrics) = self.metrics.write() {
            metrics.forks_created += 1;
        }
    }

    /// Get the current profile (for governance.status).
    pub async fn profile(&self) -> CommunicationBusProfile {
        // Read metrics first and drop the guard before any await point
        let (messages_sent, messages_failed, forks_created) = {
            let m = match self.metrics.read() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    tracing::warn!("CommunicationBus metrics lock poisoned, recovering");
                    poisoned.into_inner()
                }
            };
            (m.messages_sent, m.messages_failed, m.forks_created)
        };
        let registered_agents = self.tree.read().await.len();
        CommunicationBusProfile {
            registered_agents,
            messages_sent,
            forks_created,
            // Honest health: any undelivered message marks the bus unhealthy.
            // Previously this was hard-coded `true` (a fake metric).
            healthy: messages_failed == 0,
        }
    }

    /// Synchronous profile snapshot for non-async consumers (e.g.
    /// `governance.status`). Agent count degrades to 0 when the tree lock is
    /// contended — counters are always exact.
    pub fn profile_sync(&self) -> CommunicationBusProfile {
        let (messages_sent, messages_failed, forks_created) = {
            let m = match self.metrics.read() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    tracing::warn!("CommunicationBus metrics lock poisoned, recovering");
                    poisoned.into_inner()
                }
            };
            (m.messages_sent, m.messages_failed, m.forks_created)
        };
        let registered_agents = self.tree.try_read().map(|t| t.len()).unwrap_or(0);
        CommunicationBusProfile {
            registered_agents,
            messages_sent,
            forks_created,
            healthy: messages_failed == 0,
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

        let msg = AgentMessage::status_query(make_path("root/a"), AgentTarget::ToParent);
        bus.send_message(msg).await.unwrap();

        // Delivery is recorded in the sent counter (inbox consumption is
        // internal to AgentMessenger and not part of the public bus API).
        let profile = bus.profile().await;
        assert_eq!(profile.messages_sent, 1);
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
    async fn test_bus_record_fork() {
        let bus = CommunicationBus::new();
        bus.record_fork();
        let profile = bus.profile().await;
        assert_eq!(profile.forks_created, 1);
    }
}
