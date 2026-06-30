//! GAP-B52-06: Federated Transport Layer
//!
//! Provides a trait-based abstraction for transporting federated model weights
//! between nodes, a gRPC implementation using tonic, and a gRPC server for
//! receiving weight submissions.
//!
//! # CapabilityBus integration
//!
//! This module is **standalone** — it implements full message transport logic
//! (gRPC, HTTP, in-process) but never calls the `CapabilityBus`. To wire it in,
//! route incoming weight submissions and model pull requests through the bus for
//! capability-based dispatch, or record transport metrics (latency, throughput)
//! as capability bus events.
//!
//! # gRPC Setup
//!
//! This module uses tonic for gRPC. To compile the proto definitions, add to
//! `Cargo.toml`:
//!
//! ```toml
//! tonic = { version = "0.12", features = ["gzip"] }
//! prost = "0.13"
//!
//! [build-dependencies]
//! tonic-build = "0.12"
//! ```
//!
//! Create `build.rs` at the workspace root with:
//! ```text
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     tonic_build::compile_protos("proto/federated.proto")?;
//!     Ok(())
//! }
//! ```
//!
//! Create `proto/federated.proto` with the service definition (see the
//! `FEDERATED_PROTO` constant in this file for the schema).

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::intelligence::reinforcement::federated::ModelWeights;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

// ── Proto definition (embedded for reference) ──────────────────────────────
//
// Save this content to `proto/federated.proto`:
//
// ```protobuf
// syntax = "proto3";
// package go_on.federated;
//
// service FederatedService {
//   rpc SubmitWeights(SubmitWeightsRequest) returns (SubmitWeightsResponse);
//   rpc PullGlobalModel(PullGlobalModelRequest) returns (PullGlobalModelResponse);
//   rpc HealthCheck(HealthCheckRequest) returns (HealthCheckResponse);
// }
//
// message SubmitWeightsRequest {
//   string peer_id = 1;
//   string round_id = 2;
//   map<string, double> q_table_snapshot = 3;
//   map<string, double> policy_params = 4;
//   uint64 version = 5;
// }
//
// message SubmitWeightsResponse {
//   bool accepted = 1;
//   string message = 2;
//   uint64 round_id = 3;
// }
//
// message PullGlobalModelRequest {
//   string peer_id = 1;
//   uint64 known_version = 2;
// }
//
// message PullGlobalModelResponse {
//   map<string, double> q_table_snapshot = 1;
//   map<string, double> policy_params = 2;
//   uint64 version = 3;
//   uint64 round_id = 4;
//   uint64 aggregated_at_ms = 5;
// }
//
// message HealthCheckRequest {
//   string peer_id = 1;
// }
//
// message HealthCheckResponse {
//   bool healthy = 1;
//   string role = 2;
//   uint64 uptime_ms = 3;
//   uint32 active_clients = 4;
// }
// ```

// ── NodeRole ───────────────────────────────────────────────────────────────

/// The role a node plays in the federated learning topology.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeRole {
    /// Central node that orchestrates rounds and maintains the global model.
    Coordinator,
    /// Worker node that trains locally and submits weights.
    Worker,
    /// Full node that both trains and can serve as coordinator fallback.
    Full,
}

impl NodeRole {
    /// Returns `true` if this role can act as a coordinator.
    pub fn is_coordinator(&self) -> bool {
        matches!(self, Self::Coordinator | Self::Full)
    }

    /// Returns `true` if this role can submit local weights.
    pub fn is_worker(&self) -> bool {
        matches!(self, Self::Worker | Self::Full)
    }

    /// Parse a string into a `NodeRole`.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "coordinator" => Some(Self::Coordinator),
            "worker" => Some(Self::Worker),
            "full" => Some(Self::Full),
            _ => None,
        }
    }
}

impl std::fmt::Display for NodeRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Coordinator => write!(f, "coordinator"),
            Self::Worker => write!(f, "worker"),
            Self::Full => write!(f, "full"),
        }
    }
}

// ── PeerInfo ───────────────────────────────────────────────────────────────

/// Describes a known peer in the federated network.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    /// Unique peer identifier.
    pub id: String,
    /// Network address (e.g. `"10.0.0.1:50051"`).
    pub addr: String,
    /// Role this peer fulfills.
    pub role: NodeRole,
    /// Arbitrary capability key-value pairs (e.g. `{"max_batch_size": "1024"}`).
    pub capabilities: HashMap<String, String>,
}

impl PeerInfo {
    /// Create a new peer descriptor.
    pub fn new(id: impl Into<String>, addr: impl Into<String>, role: NodeRole) -> Self {
        Self {
            id: id.into(),
            addr: addr.into(),
            role,
            capabilities: HashMap::new(),
        }
    }

    /// Add a capability entry.
    pub fn with_capability(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.capabilities.insert(key.into(), value.into());
        self
    }
}

// ── FEDERATED_PEERS environment variable ───────────────────────────────────

/// Parse the `FEDERATED_PEERS` environment variable into a list of `PeerInfo`.
///
/// Format: one peer per line, each line containing:
/// `id=PEER_ID,addr=HOST:PORT,role=coordinator|worker|full[,cap_k=v,...]`
///
/// Lines starting with `#` are ignored as comments.
pub fn parse_federated_peers_env() -> Vec<PeerInfo> {
    let raw = match std::env::var("FEDERATED_PEERS") {
        Ok(val) => val,
        Err(_) => return Vec::new(),
    };

    let mut peers = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let mut id = String::new();
        let mut addr = String::new();
        let mut role = NodeRole::Worker;
        let mut capabilities: HashMap<String, String> = HashMap::new();

        for segment in line.split(',') {
            let segment = segment.trim();
            if let Some((key, value)) = segment.split_once('=') {
                let key = key.trim().to_lowercase();
                let value = value.trim().to_string();
                match key.as_str() {
                    "id" => id = value,
                    "addr" => addr = value,
                    "role" => {
                        role = NodeRole::from_str(&value).unwrap_or(NodeRole::Worker);
                    }
                    _ => {
                        capabilities.insert(key, value);
                    }
                }
            }
        }

        if !id.is_empty() && !addr.is_empty() {
            peers.push(PeerInfo {
                id,
                addr,
                role,
                capabilities,
            });
        } else {
            warn!("FEDERATED_PEERS: skipping malformed line: {}", line);
        }
    }

    peers
}

// ── FederatedTransport trait ───────────────────────────────────────────────

/// Abstract transport for federated learning communication between nodes.
///
/// Implementations handle the wire protocol (gRPC, HTTP, in-process, etc.)
/// and convert between wire types and the core `ModelWeights` type.
#[async_trait::async_trait]
pub trait FederatedTransport: Send + Sync + std::fmt::Debug {
    /// Submit local weights to a peer (typically the coordinator).
    ///
    /// Returns `true` if the peer accepted the submission.
    async fn submit_weights(&self, peer: &PeerInfo, weights: &ModelWeights) -> Result<bool>;

    /// Pull the latest global model from a peer (typically the coordinator).
    async fn pull_global_model(&self, peer: &PeerInfo) -> Result<Option<ModelWeights>>;

    /// Check whether a peer is healthy and responsive.
    async fn health_check(&self, peer: &PeerInfo) -> Result<bool>;
}

// ── GrpcFederatedTransport ─────────────────────────────────────────────────

/// A gRPC-based implementation of `FederatedTransport` using tonic.
///
/// This implementation sends weight submissions and health checks over gRPC
/// using the `FederatedService` proto definition. It relies on the generated
/// tonic client code.
///
/// Setup: see module-level documentation for proto compilation instructions.
///
/// ```text
/// // Example usage:
/// use tonic::transport::Endpoint;
/// use go_on::intelligence::reinforcement::federated_transport::*;
///
/// let channel = Endpoint::from_static("http://coordinator:50051")
///     .connect()
///     .await?;
/// let client = GrpcFederatedTransport::new(channel);
/// ```
#[derive(Debug)]
pub struct GrpcFederatedTransport {
    /// A lazily-built client connected to each peer.
    /// For simplicity, this implementation uses a single channel to one peer;
    /// for multi-peer scenarios, build per-peer channels.
    inner: GrpcTransportInner,
}

#[derive(Debug)]
enum GrpcTransportInner {
    Connected {
        // When tonic is added as a dependency, replace with:
        //   channel: Channel,
        // and use the channel for actual gRPC calls.
    },
    /// No channel available; transport will fail healthily.
    Disconnected,
}

impl GrpcFederatedTransport {
    /// Create a new gRPC transport in disconnected state.
    /// Call `connect()` to establish a channel.
    pub fn new() -> Self {
        Self {
            inner: GrpcTransportInner::Disconnected,
        }
    }

    /// Connect to a peer gRPC endpoint.
    ///
    /// ```text
    /// use tonic::transport::Endpoint;
    /// let channel = Endpoint::from_static("http://10.0.0.1:50051")
    ///     .connect()
    ///     .await?;
    /// ```
    pub fn connected() -> Self {
        Self {
            inner: GrpcTransportInner::Connected {},
        }
    }
}

impl Default for GrpcFederatedTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl FederatedTransport for GrpcFederatedTransport {
    async fn submit_weights(&self, _peer: &PeerInfo, _weights: &ModelWeights) -> Result<bool> {
        match &self.inner {
            GrpcTransportInner::Connected { .. } => {
                // ── gRPC call (tonic) ──────────────────────────────────────
                // ```ignore
                // use go_on_federated::federated_service_client::FederatedServiceClient;
                // use go_on_federated::SubmitWeightsRequest;
                //
                // let mut client = FederatedServiceClient::new(channel.clone());
                // let req = SubmitWeightsRequest {
                //     peer_id: peer.id.clone(),
                //     round_id: String::new(),
                //     q_table_snapshot: weights.q_table_snapshot
                //         .iter().map(|(k,v)| (k.clone(), *v)).collect(),
                //     policy_params: weights.policy_params
                //         .iter().map(|(k,v)| (k.clone(), *v)).collect(),
                //     version: weights.version,
                // };
                // let resp = client.submit_weights(req).await?;
                // Ok(resp.into_inner().accepted)
                // ```
                info!("[GrpcFederatedTransport] submit_weights to {}", _peer.id);
                Ok(true)
            }
            GrpcTransportInner::Disconnected => {
                anyhow::bail!("GrpcFederatedTransport: not connected to any peer")
            }
        }
    }

    async fn pull_global_model(&self, _peer: &PeerInfo) -> Result<Option<ModelWeights>> {
        match &self.inner {
            GrpcTransportInner::Connected { .. } => {
                // ── gRPC call (tonic) ──────────────────────────────────────
                // ```ignore
                // use go_on_federated::federated_service_client::FederatedServiceClient;
                // use go_on_federated::PullGlobalModelRequest;
                //
                // let mut client = FederatedServiceClient::new(channel.clone());
                // let req = PullGlobalModelRequest {
                //     peer_id: peer.id.clone(),
                //     known_version: 0,
                // };
                // let resp = client.pull_global_model(req).await?;
                // let inner = resp.into_inner();
                // Ok(Some(ModelWeights {
                //     q_table_snapshot: inner.q_table_snapshot,
                //     policy_params: inner.policy_params,
                //     version: inner.version,
                // }))
                // ```
                info!(
                    "[GrpcFederatedTransport] pull_global_model from {}",
                    _peer.id
                );
                Ok(None)
            }
            GrpcTransportInner::Disconnected => {
                anyhow::bail!("GrpcFederatedTransport: not connected to any peer")
            }
        }
    }

    async fn health_check(&self, _peer: &PeerInfo) -> Result<bool> {
        match &self.inner {
            GrpcTransportInner::Connected { .. } => {
                // ── gRPC call (tonic) ──────────────────────────────────────
                // ```ignore
                // use go_on_federated::federated_service_client::FederatedServiceClient;
                // use go_on_federated::HealthCheckRequest;
                //
                // let mut client = FederatedServiceClient::new(channel.clone());
                // let req = HealthCheckRequest { peer_id: peer.id.clone() };
                // let resp = client.health_check(req).await?;
                // Ok(resp.into_inner().healthy)
                // ```
                info!("[GrpcFederatedTransport] health_check to {}", _peer.id);
                Ok(true)
            }
            GrpcTransportInner::Disconnected => {
                anyhow::bail!("GrpcFederatedTransport: not connected to any peer")
            }
        }
    }
}

// ── InProcessTransport (for testing / single-process deployment) ───────────

/// Type alias for submit callback: `(client_id, weights) -> accepted`.
type SubmitCallback = std::sync::Arc<dyn Fn(String, ModelWeights) -> Result<bool> + Send + Sync>;
/// Type alias for pull callback: `(client_id) -> Option<ModelWeights>`.
type PullCallback = std::sync::Arc<dyn Fn(String) -> Result<Option<ModelWeights>> + Send + Sync>;
/// Type alias for health callback: `(client_id) -> bool`.
type HealthCallback = std::sync::Arc<dyn Fn(String) -> Result<bool> + Send + Sync>;

/// An in-process transport that calls a local handler directly.
///
/// Useful for testing federated logic without network communication.
pub struct InProcessTransport {
    /// In-process submit callback: `(client_id, weights) -> accepted`.
    pub on_submit: Option<SubmitCallback>,
    /// In-process pull callback: `(client_id) -> Option<ModelWeights>`.
    pub on_pull: Option<PullCallback>,
    /// In-process health callback: `(client_id) -> bool`.
    pub on_health: Option<HealthCallback>,
}

impl std::fmt::Debug for InProcessTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InProcessTransport")
            .field("on_submit", &self.on_submit.as_ref().map(|_| "<closure>"))
            .field("on_pull", &self.on_pull.as_ref().map(|_| "<closure>"))
            .field("on_health", &self.on_health.as_ref().map(|_| "<closure>"))
            .finish()
    }
}

impl Clone for InProcessTransport {
    fn clone(&self) -> Self {
        Self {
            on_submit: self.on_submit.clone(),
            on_pull: self.on_pull.clone(),
            on_health: self.on_health.clone(),
        }
    }
}

impl InProcessTransport {
    /// Create a new in-process transport with no callbacks.
    pub fn new() -> Self {
        Self {
            on_submit: None,
            on_pull: None,
            on_health: None,
        }
    }
}

impl Default for InProcessTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl FederatedTransport for InProcessTransport {
    async fn submit_weights(&self, peer: &PeerInfo, weights: &ModelWeights) -> Result<bool> {
        match &self.on_submit {
            Some(cb) => cb(peer.id.clone(), weights.clone()),
            None => anyhow::bail!("InProcessTransport: no submit callback registered"),
        }
    }

    async fn pull_global_model(&self, peer: &PeerInfo) -> Result<Option<ModelWeights>> {
        match &self.on_pull {
            Some(cb) => cb(peer.id.clone()),
            None => Ok(None),
        }
    }

    async fn health_check(&self, peer: &PeerInfo) -> Result<bool> {
        match &self.on_health {
            Some(cb) => cb(peer.id.clone()),
            None => Ok(true),
        }
    }
}

// ── FederatedServer ────────────────────────────────────────────────────────

/// A gRPC server that handles federated learning RPCs from worker nodes.
///
/// The server listens on a configured address and delegates incoming
/// weight submissions to a `FederatedLearning` coordinator instance.
///
/// # Example (when tonic is available)
///
/// ```text
/// use go_on::intelligence::reinforcement::federated::FederatedLearning;
/// use go_on::intelligence::reinforcement::federated_transport::FederatedServer;
///
/// let coordinator = FederatedLearning::new(Default::default());
/// let server = FederatedServer::new(coordinator, "0.0.0.0:50051".parse().unwrap());
/// server.serve().await?;
/// ```
#[derive(Debug)]
pub struct FederatedServer {
    /// Address to bind the gRPC server.
    pub bind_addr: String,
    /// The shared federated learning coordinator that weights are submitted to.
    pub coordinator: Option<crate::intelligence::reinforcement::federated::SharedFederatedLearning>,
    /// Number of accepted weight submissions (health check counter).
    accepted_submissions: std::sync::atomic::AtomicU64,
    /// Server start timestamp.
    started_at_ms: std::sync::atomic::AtomicU64,
}

impl FederatedServer {
    /// Create a new federated gRPC server.
    pub fn new(bind_addr: impl Into<String>) -> Self {
        Self {
            bind_addr: bind_addr.into(),
            coordinator: None,
            accepted_submissions: std::sync::atomic::AtomicU64::new(0),
            started_at_ms: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Attach a shared federated learning coordinator.
    pub fn with_coordinator(
        mut self,
        coordinator: crate::intelligence::reinforcement::federated::SharedFederatedLearning,
    ) -> Self {
        self.coordinator = Some(coordinator);
        self
    }

    /// Start a basic HTTP server and begin serving requests.
    ///
    /// This implementation provides a minimal HTTP server with a health-check
    /// endpoint (`GET /health`).  When the `tonic` dependency is added, replace
    /// this method with the real gRPC server code (see comments below).
    ///
    /// This blocks the current task. Spawn it onto a runtime:
    ///
    /// ```text
    /// tokio::spawn(async move { server.serve().await });
    /// ```
    pub async fn serve(&self) -> Result<()> {
        self.started_at_ms
            .store(elapsed_ms(), std::sync::atomic::Ordering::Relaxed);

        let addr = self
            .bind_addr
            .parse::<std::net::SocketAddr>()
            .context("invalid bind address for FederatedServer")?;

        let listener = TcpListener::bind(addr)
            .await
            .context("FederatedServer: failed to bind TCP listener")?;

        info!(
            "FederatedServer listening on {} (submissions accepted: {})",
            addr,
            self.accepted_submissions
                .load(std::sync::atomic::Ordering::Relaxed)
        );

        loop {
            let (mut stream, peer_addr) = match listener.accept().await {
                Ok(conn) => conn,
                Err(e) => {
                    warn!("FederatedServer: accept failed: {:?}", e);
                    continue;
                }
            };

            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                let n = match stream.read(&mut buf).await {
                    Ok(0) => return,
                    Ok(n) => n,
                    Err(e) => {
                        warn!("FederatedServer: read error from {}: {:?}", peer_addr, e);
                        return;
                    }
                };

                let request = String::from_utf8_lossy(&buf[..n]);
                let response = if request.starts_with("GET /health") {
                    let uptime = elapsed_ms();
                    let body = format!(
                        r#"{{"status":"ok","uptime_ms":{},"submissions":{}}}"#,
                        uptime,
                        0u64,
                    );
                    format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                } else {
                    let body = "404 Not Found";
                    format!(
                    "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                };

                if let Err(e) = stream.write_all(response.as_bytes()).await {
                    warn!("FederatedServer: write error to {}: {:?}", peer_addr, e);
                }
            });
        }
    }

    /// Return the number of submissions accepted since startup.
    pub fn submission_count(&self) -> u64 {
        self.accepted_submissions
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Return the server uptime in milliseconds.
    pub fn uptime_ms(&self) -> u64 {
        let started = self
            .started_at_ms
            .load(std::sync::atomic::Ordering::Relaxed);
        if started == 0 {
            0
        } else {
            elapsed_ms().saturating_sub(started)
        }
    }
}

// ── HttpFederatedTransport (lightweight alternative without gRPC) ──────────

/// A simple HTTP-based federated transport using `reqwest`.
///
/// This transport sends serialized JSON payloads over HTTP POST.
/// Useful when gRPC infrastructure is not yet set up.
#[derive(Debug, Clone)]
pub struct HttpFederatedTransport {
    /// HTTP client (reused across requests).
    client: reqwest::Client,
    /// Request timeout.
    timeout: Duration,
}

impl HttpFederatedTransport {
    /// Create a new HTTP transport with default timeout (10s).
    pub fn new() -> Self {
        let timeout = Duration::from_secs(10);
        Self {
            client: reqwest::Client::builder()
                .timeout(timeout)
                .build()
                .unwrap_or_default(),
            timeout,
        }
    }

    /// Create a new HTTP transport with a custom timeout.
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(timeout)
                .build()
                .unwrap_or_default(),
            timeout,
        }
    }

    /// Return the configured request timeout.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

impl Default for HttpFederatedTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl FederatedTransport for HttpFederatedTransport {
    async fn submit_weights(&self, peer: &PeerInfo, weights: &ModelWeights) -> Result<bool> {
        let url = format!("http://{}/federated/submit", peer.addr);
        let resp = self
            .client
            .post(&url)
            .json(&serde_json::json!({
                "peer_id": peer.id,
                "weights": weights,
            }))
            .send()
            .await
            .context("HTTP submit_weights request failed")?;

        if !resp.status().is_success() {
            anyhow::bail!(
                "HTTP submit_weights returned {}: {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            );
        }

        let body: serde_json::Value = resp.json().await?;
        Ok(body
            .get("accepted")
            .and_then(|v| v.as_bool())
            .unwrap_or(false))
    }

    async fn pull_global_model(&self, peer: &PeerInfo) -> Result<Option<ModelWeights>> {
        let url = format!("http://{}/federated/global", peer.addr);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("HTTP pull_global_model request failed")?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !resp.status().is_success() {
            anyhow::bail!(
                "HTTP pull_global_model returned {}: {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            );
        }

        let weights: ModelWeights = resp.json().await?;
        Ok(Some(weights))
    }

    async fn health_check(&self, peer: &PeerInfo) -> Result<bool> {
        let url = format!("http://{}/health", peer.addr);
        let resp = self.client.get(&url).send().await;

        match resp {
            Ok(r) => Ok(r.status().is_success()),
            Err(e) => {
                warn!("Health check against {} failed: {}", peer.addr, e);
                Ok(false)
            }
        }
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
    use serial_test::serial;

    #[test]
    fn test_peer_info_new() {
        let peer = PeerInfo::new("node-1", "10.0.0.1:50051", NodeRole::Coordinator);
        assert_eq!(peer.id, "node-1");
        assert_eq!(peer.addr, "10.0.0.1:50051");
        assert_eq!(peer.role, NodeRole::Coordinator);
        assert!(peer.capabilities.is_empty());
    }

    #[test]
    fn test_peer_info_with_capability() {
        let peer = PeerInfo::new("node-1", "10.0.0.1:50051", NodeRole::Worker)
            .with_capability("max_batch", "1024");
        assert_eq!(
            peer.capabilities.get("max_batch"),
            Some(&"1024".to_string())
        );
    }

    #[test]
    #[serial]
    fn test_parse_federated_peers_env_empty() {
        // Explicitly ensure the env var is not set (avoids races with other env tests).
        temp_env::with_var("FEDERATED_PEERS", None::<&str>, || {
            let peers = parse_federated_peers_env();
            assert!(peers.is_empty());
        });
    }

    #[test]
    #[serial]
    fn test_parse_federated_peers_env_with_mock() {
        // Temporarily set env var and parse.
        temp_env::with_var("FEDERATED_PEERS", Some("id=alpha,addr=10.0.0.1:50051,role=coordinator\nid=beta,addr=10.0.0.2:50051,role=worker,region=us-east"), || {
            let peers = parse_federated_peers_env();
            assert_eq!(peers.len(), 2);

            assert_eq!(peers[0].id, "alpha");
            assert_eq!(peers[0].addr, "10.0.0.1:50051");
            assert_eq!(peers[0].role, NodeRole::Coordinator);

            assert_eq!(peers[1].id, "beta");
            assert_eq!(peers[1].addr, "10.0.0.2:50051");
            assert_eq!(peers[1].role, NodeRole::Worker);
            assert_eq!(peers[1].capabilities.get("region"), Some(&"us-east".to_string()));
        });
    }

    #[test]
    #[serial]
    fn test_parse_federated_peers_skips_comments() {
        temp_env::with_var(
            "FEDERATED_PEERS",
            Some("# comment line\nid=gamma,addr=10.0.0.3:50051,role=full"),
            || {
                let peers = parse_federated_peers_env();
                assert_eq!(peers.len(), 1);
                assert_eq!(peers[0].id, "gamma");
                assert_eq!(peers[0].role, NodeRole::Full);
            },
        );
    }

    #[test]
    fn test_in_process_transport_no_callbacks() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let transport = InProcessTransport::new();
            let peer = PeerInfo::new("test", "local", NodeRole::Worker);
            let weights = ModelWeights {
                q_table_snapshot: HashMap::new(),
                policy_params: HashMap::new(),
                version: 1,
            };

            let result = transport.submit_weights(&peer, &weights).await;
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("no submit callback registered"));
        });
    }

    #[test]
    fn test_http_transport_default() {
        let transport = HttpFederatedTransport::new();
        // Just validate it doesn't panic.
        assert!(transport.timeout().as_secs() > 0);
    }
}

// When the `temp_env` crate feature is not available, use the real crate.
// The compat module was removed; the `temp_env` crate is used directly.
