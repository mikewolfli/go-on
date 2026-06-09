//! Remote Executor (GAP-B52-21)
//!
//! Defines the TaskPacket, NodeOutput, and RemoteExecutor trait for
//! executing DAG tasks on remote nodes. Includes a gRPC-based implementation
//! leveraging the project's tonic dependency.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::orchestration::tool::{ToolInput, ToolRegistry};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[allow(dead_code)] // F-GAP-49 — reserved for distributed remote executor
#[derive(Debug, Error)]
pub enum RemoteExecutionError {
    #[error("node not found: {0}")]
    NodeNotFound(String),

    #[error("node {0} is offline")]
    NodeOffline(String),

    #[error("execution failed on node {0}: {1}")]
    ExecutionFailed(String, String),

    #[error("gRPC error: {0}")]
    GrpcError(String),

    #[error("packet encoding error: {0}")]
    EncodingError(String),

    #[error("timeout on node {0} after {1}s")]
    Timeout(String, u64),

    #[error("capability mismatch on node {0}: expected {1}")]
    CapabilityMismatch(String, String),
}

// ---------------------------------------------------------------------------
// NodeId / DagId type aliases
// ---------------------------------------------------------------------------

/// Unique identifier for a distributed node.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub String);

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for NodeId {
    fn from(s: String) -> Self {
        NodeId(s)
    }
}

impl From<&str> for NodeId {
    fn from(s: &str) -> Self {
        NodeId(s.to_string())
    }
}

impl std::borrow::Borrow<str> for NodeId {
    fn borrow(&self) -> &str {
        self.0.as_str()
    }
}

impl std::borrow::Borrow<String> for NodeId {
    fn borrow(&self) -> &String {
        &self.0
    }
}

/// Unique identifier for a DAG instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DagId(pub String);

impl std::fmt::Display for DagId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for DagId {
    fn from(s: String) -> Self {
        DagId(s)
    }
}

impl From<&str> for DagId {
    fn from(s: &str) -> Self {
        DagId(s.to_string())
    }
}

impl std::borrow::Borrow<str> for DagId {
    fn borrow(&self) -> &str {
        self.0.as_str()
    }
}

impl std::borrow::Borrow<String> for DagId {
    fn borrow(&self) -> &String {
        &self.0
    }
}

// ---------------------------------------------------------------------------
// TaskPacket
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPacket {
    /// The target node ID to execute on.
    pub node_id: NodeId,
    /// The DAG instance this packet belongs to.
    pub dag_id: DagId,
    /// The tool / function to invoke.
    pub tool_name: String,
    /// JSON input to the tool.
    pub input: serde_json::Value,
    /// Outputs from dependency nodes, keyed by node ID.
    pub dep_outputs: HashMap<NodeId, serde_json::Value>,
    /// Number of retries already attempted.
    pub retry_count: u32,
    /// Maximum retries before giving up.
    pub max_retries: u32,
}

#[allow(dead_code)] // F-GAP-49 — reserved for distributed remote executor
impl TaskPacket {
    /// Create a new TaskPacket for a given node and DAG.
    pub fn new(
        node_id: NodeId,
        dag_id: DagId,
        tool_name: String,
        input: serde_json::Value,
    ) -> Self {
        Self {
            node_id,
            dag_id,
            tool_name,
            input,
            dep_outputs: HashMap::new(),
            retry_count: 0,
            max_retries: 3,
        }
    }

    /// Increment retry count and return true if we should retry.
    pub fn should_retry(&self) -> bool {
        self.retry_count < self.max_retries
    }
}

// ---------------------------------------------------------------------------
// NodeOutput
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeOutput {
    /// The node ID that produced this output.
    pub node_id: NodeId,
    /// The DAG instance ID.
    pub dag_id: DagId,
    /// The tool that was executed.
    pub tool_name: String,
    /// Whether execution succeeded.
    pub success: bool,
    /// JSON output value (present on success).
    pub output: Option<serde_json::Value>,
    /// Error message (present on failure).
    pub error: Option<String>,
    /// Duration of execution in milliseconds.
    pub duration_ms: u64,
    /// Wall-clock timestamp when execution completed.
    pub completed_at_ms: u64,
}

#[allow(dead_code)] // F-GAP-49 — reserved for distributed remote executor
impl NodeOutput {
    pub fn success(
        node_id: NodeId,
        dag_id: DagId,
        tool_name: String,
        output: serde_json::Value,
        duration_ms: u64,
    ) -> Self {
        Self {
            node_id,
            dag_id,
            tool_name,
            success: true,
            output: Some(output),
            error: None,
            duration_ms,
            completed_at_ms: current_timestamp_ms(),
        }
    }

    pub fn failure(
        node_id: NodeId,
        dag_id: DagId,
        tool_name: String,
        error: String,
        duration_ms: u64,
    ) -> Self {
        Self {
            node_id,
            dag_id,
            tool_name,
            success: false,
            output: None,
            error: Some(error),
            duration_ms,
            completed_at_ms: current_timestamp_ms(),
        }
    }
}

// ---------------------------------------------------------------------------
// NodeCapabilities
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeCapabilities {
    pub node_id: NodeId,
    pub node_version: String,
    pub supported_tools: Vec<String>,
    pub max_concurrency: u32,
    pub memory_mb: u64,
    pub cpu_cores: u32,
    pub additional_caps: HashMap<String, String>,
}

#[allow(dead_code)] // F-GAP-49 — reserved for distributed remote executor
impl NodeCapabilities {
    pub fn new(node_id: NodeId, supported_tools: Vec<String>) -> Self {
        Self {
            node_id,
            node_version: env!("CARGO_PKG_VERSION").to_string(),
            supported_tools,
            max_concurrency: 4,
            memory_mb: 1024,
            cpu_cores: 2,
            additional_caps: HashMap::new(),
        }
    }

    /// Check if this node supports a given tool.
    pub fn supports_tool(&self, tool_name: &str) -> bool {
        self.supported_tools.iter().any(|t| t == tool_name)
    }
}

// ---------------------------------------------------------------------------
// NodeRegistration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRegistration {
    pub node_id: NodeId,
    pub address: String,
    pub port: u16,
    pub capabilities: NodeCapabilities,
    pub registered_at_ms: u64,
}

impl NodeRegistration {
    pub fn new(
        node_id: NodeId,
        address: String,
        port: u16,
        capabilities: NodeCapabilities,
    ) -> Self {
        Self {
            node_id,
            address,
            port,
            capabilities,
            registered_at_ms: current_timestamp_ms(),
        }
    }

    /// Full address string (host:port).
    pub fn addr(&self) -> String {
        format!("{}:{}", self.address, self.port)
    }
}

// ---------------------------------------------------------------------------
// RemoteExecutor trait
// ---------------------------------------------------------------------------

/// Trait for executing tasks on remote nodes.
#[allow(dead_code)] // F-GAP-49 — reserved for distributed remote executor
#[async_trait::async_trait]
pub trait RemoteExecutor: Send + Sync {
    /// Execute a task packet on a remote node and return the output.
    async fn execute_remote(&self, packet: TaskPacket) -> Result<NodeOutput, RemoteExecutionError>;

    /// Register a node with its capabilities.
    async fn register_node(
        &self,
        registration: NodeRegistration,
    ) -> Result<(), RemoteExecutionError>;

    /// Unregister a node.
    async fn unregister_node(&self, node_id: &str) -> Result<(), RemoteExecutionError>;

    /// Get the capabilities of a registered node.
    async fn get_capabilities(
        &self,
        node_id: &str,
    ) -> Result<Option<NodeCapabilities>, RemoteExecutionError>;

    /// List all registered nodes.
    async fn list_nodes(&self) -> Result<Vec<NodeRegistration>, RemoteExecutionError>;

    /// Check if a node is alive.
    async fn health_check(&self, node_id: &str) -> Result<bool, RemoteExecutionError>;
}

// ---------------------------------------------------------------------------
// NodeRegistry (in-memory store)
// ---------------------------------------------------------------------------

/// In-memory node registry used as a backing store for executors.
#[allow(dead_code)] // F-GAP-49 — reserved for distributed remote executor
#[derive(Debug)]
pub struct NodeRegistry {
    nodes: RwLock<HashMap<NodeId, NodeRegistration>>,
}

#[allow(dead_code)] // F-GAP-49 — reserved for distributed remote executor
impl NodeRegistry {
    pub fn new() -> Self {
        Self {
            nodes: RwLock::new(HashMap::new()),
        }
    }

    pub async fn register(&self, reg: NodeRegistration) {
        debug!(node = %reg.node_id, addr = %reg.addr(), "Node registered");
        self.nodes.write().await.insert(reg.node_id.clone(), reg);
    }

    pub async fn unregister(&self, node_id: &str) -> bool {
        debug!(node = %node_id, "Node unregistered");
        self.nodes.write().await.remove(node_id).is_some()
    }

    pub async fn get(&self, node_id: &str) -> Option<NodeRegistration> {
        self.nodes.read().await.get(node_id).cloned()
    }

    pub async fn list(&self) -> Vec<NodeRegistration> {
        self.nodes.read().await.values().cloned().collect()
    }

    pub async fn capabilities(&self, node_id: &str) -> Option<NodeCapabilities> {
        self.nodes
            .read()
            .await
            .get(node_id)
            .map(|r| r.capabilities.clone())
    }
}

impl Default for NodeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Mock / InProcessRemoteExecutor (for testing and single-process setups)
// ---------------------------------------------------------------------------

/// An in-process remote executor that runs tasks locally.
/// Useful for testing and single-node deployments.
#[allow(dead_code)] // F-GAP-49 — reserved for distributed remote executor
#[derive(Debug)]
pub struct InProcessRemoteExecutor {
    registry: Arc<NodeRegistry>,
    tool_registry: Arc<ToolRegistry>,
}

#[allow(dead_code)] // F-GAP-49 — reserved for distributed remote executor
impl InProcessRemoteExecutor {
    pub fn new(registry: Arc<NodeRegistry>, tool_registry: Arc<ToolRegistry>) -> Self {
        Self {
            registry,
            tool_registry,
        }
    }
}

#[async_trait::async_trait]
impl RemoteExecutor for InProcessRemoteExecutor {
    async fn execute_remote(&self, packet: TaskPacket) -> Result<NodeOutput, RemoteExecutionError> {
        let node_id = packet.node_id.clone();
        let dag_id = packet.dag_id.clone();
        let tool_name = packet.tool_name.clone();

        // Verify the node is registered
        let reg = self
            .registry
            .get(node_id.0.as_str())
            .await
            .ok_or_else(|| RemoteExecutionError::NodeNotFound(node_id.clone().0))?; // ✅ use .0 for inner string

        // Verify capability
        if !reg.capabilities.supports_tool(&tool_name) {
            return Err(RemoteExecutionError::CapabilityMismatch(
                node_id.0.clone(),
                format!("tool '{}' not in supported set", tool_name),
            ));
        }

        let start = std::time::Instant::now();

        debug!(
            node = %node_id, dag = %dag_id, tool = %tool_name,
            "In-process remote executor: invoking tool"
        );

        // Build ToolInput from the TaskPacket for real execution.
        let tool_input = ToolInput {
            task_id: dag_id.clone().0,
            phase: "remote_execution".to_string(),
            agent_role: "remote_executor".to_string(),
            objective: String::new(),
            constraints: None,
            evidence: if packet.dep_outputs.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&packet.dep_outputs).unwrap_or_default())
            },
            payload: packet.input,
            allowed_base_dir: None,
        };

        // Execute the tool through the ToolRegistry; fail if tool not found.
        let tool = self.tool_registry.get(&tool_name).ok_or_else(|| {
            RemoteExecutionError::ExecutionFailed(
                node_id.0.clone(),
                format!("tool '{}' not found in registry", tool_name),
            )
        })?;

        let tool_output = tool.run(&tool_input).map_err(|e| {
            RemoteExecutionError::ExecutionFailed(
                node_id.0.clone(),
                format!("tool '{}' execution error: {}", tool_name, e),
            )
        })?;

        let duration_ms = start.elapsed().as_millis() as u64;

        if tool_output.success {
            let output_value = tool_output
                .result
                .unwrap_or_else(|| serde_json::json!({"status": "completed"}));
            info!(
                node = %node_id, dag = %dag_id, tool = %tool_name, duration_ms,
                "In-process remote executor: tool completed successfully"
            );
            Ok(NodeOutput::success(
                node_id,
                dag_id,
                tool_name,
                output_value,
                duration_ms,
            ))
        } else {
            let error_msg = tool_output
                .error
                .unwrap_or_else(|| "unknown error".to_string());
            warn!(
                node = %node_id, dag = %dag_id, tool = %tool_name, error = %error_msg,
                "In-process remote executor: tool failed"
            );
            Ok(NodeOutput::failure(
                node_id,
                dag_id,
                tool_name,
                error_msg,
                duration_ms,
            ))
        }
    }

    async fn register_node(
        &self,
        registration: NodeRegistration,
    ) -> Result<(), RemoteExecutionError> {
        self.registry.register(registration).await;
        Ok(())
    }

    async fn unregister_node(&self, node_id: &str) -> Result<(), RemoteExecutionError> {
        self.registry.unregister(node_id).await;
        Ok(())
    }

    async fn get_capabilities(
        &self,
        node_id: &str,
    ) -> Result<Option<NodeCapabilities>, RemoteExecutionError> {
        Ok(self.registry.capabilities(node_id).await)
    }

    async fn list_nodes(&self) -> Result<Vec<NodeRegistration>, RemoteExecutionError> {
        Ok(self.registry.list().await)
    }

    async fn health_check(&self, node_id: &str) -> Result<bool, RemoteExecutionError> {
        Ok(self.registry.get(node_id).await.is_some())
    }
}

// ---------------------------------------------------------------------------
// GrpcRemoteExecutor (tonic-based)
// ---------------------------------------------------------------------------

/// A gRPC-based remote executor that communicates with remote nodes via
/// tonic/protobuf. Reuses the project's existing tonic dependency from
/// opentelemetry-otlp.
///
/// Note: This is a stub that requires a proto service definition and
/// tonic build setup for full functionality. The structure demonstrates
/// the integration boundary.
#[allow(dead_code)] // F-GAP-49 — reserved for distributed remote executor
#[derive(Debug)]
pub struct GrpcRemoteExecutor {
    registry: Arc<NodeRegistry>,
    /// gRPC channel map: node_id -> endpoint address
    channels: RwLock<HashMap<NodeId, String>>,
    /// Default timeout in seconds for gRPC calls.
    #[allow(dead_code)] // F-GAP-49 — reserved for distributed remote executor
    default_timeout_s: u64,
}

#[allow(dead_code)] // F-GAP-49 — reserved for distributed remote executor
impl GrpcRemoteExecutor {
    pub fn new(registry: Arc<NodeRegistry>, default_timeout_s: u64) -> Self {
        Self {
            registry,
            channels: RwLock::new(HashMap::new()),
            default_timeout_s,
        }
    }

    /// Resolve the gRPC address for a node.
    #[allow(dead_code)] // F-GAP-49 — reserved for distributed remote executor
    async fn resolve_addr(&self, node_id: &NodeId) -> Result<String, RemoteExecutionError> {
        if let Some(addr) = self.channels.read().await.get(node_id.0.as_str()) {
            return Ok(addr.clone());
        }
        // Fallback: look up from registry
        let reg = self
            .registry
            .get(node_id.0.as_str())
            .await
            .ok_or_else(|| RemoteExecutionError::NodeNotFound(node_id.0.clone()))?;
        let addr = format!("http://{}", reg.addr());
        self.channels
            .write()
            .await
            .insert(node_id.clone(), addr.clone());
        Ok(addr)
    }
}

#[async_trait::async_trait]
impl RemoteExecutor for GrpcRemoteExecutor {
    async fn execute_remote(&self, packet: TaskPacket) -> Result<NodeOutput, RemoteExecutionError> {
        let node_id = packet.node_id.clone();
        let addr = self.resolve_addr(&node_id).await?;
        let tool_name = packet.tool_name.clone();

        // Fail-fast with a clear error: gRPC execution requires a proto service
        // definition and tonic build setup. Until that is wired, the executor
        // cannot issue real RPCs. Users should fall back to InProcessRemoteExecutor.
        let msg = format!(
            "gRPC execution not available for tool '{tool_name}' on node '{node_id}' (address: {addr}). "
        );
        Err(RemoteExecutionError::GrpcError(msg))
    }

    async fn register_node(
        &self,
        registration: NodeRegistration,
    ) -> Result<(), RemoteExecutionError> {
        self.registry.register(registration).await;
        Ok(())
    }

    async fn unregister_node(&self, node_id: &str) -> Result<(), RemoteExecutionError> {
        self.channels.write().await.remove(node_id);
        self.registry.unregister(node_id).await;
        Ok(())
    }

    async fn get_capabilities(
        &self,
        node_id: &str,
    ) -> Result<Option<NodeCapabilities>, RemoteExecutionError> {
        Ok(self.registry.capabilities(node_id).await)
    }

    async fn list_nodes(&self) -> Result<Vec<NodeRegistration>, RemoteExecutionError> {
        Ok(self.registry.list().await)
    }

    async fn health_check(&self, node_id: &str) -> Result<bool, RemoteExecutionError> {
        Ok(self.registry.get(node_id).await.is_some())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn current_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_in_process_executor() {
        let registry = Arc::new(NodeRegistry::new());
        // Use default tool registry which includes shell_exec tool
        let tool_registry = Arc::new(ToolRegistry::new());
        let executor = InProcessRemoteExecutor::new(registry.clone(), tool_registry);

        let caps = NodeCapabilities::new("node-1".into(), vec!["shell_exec".into()]);
        let reg = NodeRegistration::new("node-1".into(), "127.0.0.1".into(), 9000, caps);
        executor.register_node(reg).await.unwrap();

        let packet = TaskPacket::new(
            "node-1".into(),
            "dag-1".into(),
            "shell_exec".into(),
            serde_json::json!({"command": "echo hello"}),
        );

        let output = executor.execute_remote(packet).await.unwrap();
        assert!(output.success);
        assert_eq!(output.node_id.0, "node-1");
    }

    #[tokio::test]
    async fn test_node_registry() {
        let registry = NodeRegistry::new();
        let caps = NodeCapabilities::new("node-2".into(), vec!["read".into()]);
        let reg = NodeRegistration::new("node-2".into(), "10.0.0.1".into(), 9001, caps);
        registry.register(reg).await;

        let nodes = registry.list().await;
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_id.0, "node-2");

        let caps = registry.capabilities("node-2").await;
        assert!(caps.is_some());
        assert!(caps.unwrap().supports_tool("read"));

        registry.unregister("node-2").await;
        assert!(registry.list().await.is_empty());
    }

    #[tokio::test]
    async fn test_executor_capability_mismatch() {
        let registry = Arc::new(NodeRegistry::new());
        let tool_registry = Arc::new(ToolRegistry::new_empty());
        let executor = InProcessRemoteExecutor::new(registry.clone(), tool_registry);

        let caps = NodeCapabilities::new("node-3".into(), vec!["read".into()]);
        let reg = NodeRegistration::new("node-3".into(), "127.0.0.1".into(), 9002, caps);
        executor.register_node(reg).await.unwrap();

        let packet = TaskPacket::new(
            "node-3".into(),
            "dag-1".into(),
            "write".into(), // not supported
            serde_json::json!({}),
        );

        let err = executor.execute_remote(packet).await.unwrap_err();
        assert!(matches!(err, RemoteExecutionError::CapabilityMismatch(..)));
    }
}
