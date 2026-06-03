//! GAP-B50-09 (CRITICAL) — WebSocket 实时通信层
//!
//! Thread-safe hub for managing WebSocket connections with topic-based pub/sub,
//! heartbeat keep-alive, and auto-reconnection support.

// activated, formerly F-GAP-51: all items below are active WebSocket code

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Unique identifier for a WebSocket connection (UUID v4).
pub type ConnectionId = String;

// ---------------------------------------------------------------------------
// WsMessage
// ---------------------------------------------------------------------------

/// A message sent over the WebSocket.
///
/// The `type` field identifies the kind of payload and follows a structured
/// naming convention for topic-based filtering.
// activated, formerly F-GAP-51
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsMessage {
    /// Message type — see module docs for known type strings.
    #[serde(rename = "type")]
    pub msg_type: String,
    /// Arbitrary JSON payload.
    pub payload: Value,
    /// Unix timestamp (seconds since epoch).
    pub timestamp: u64,
}

// activated, formerly F-GAP-51
impl WsMessage {
    /// Create a new `WsMessage` with the current system timestamp.
    pub fn new(msg_type: impl Into<String>, payload: Value) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            msg_type: msg_type.into(),
            payload,
            timestamp,
        }
    }
}

// ---------------------------------------------------------------------------
// ConnectionMetadata
// ---------------------------------------------------------------------------

/// Metadata attached to each registered connection.
// activated, formerly F-GAP-51
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionMetadata {
    /// Timestamp (Unix seconds) when the connection was established.
    pub connected_at: u64,
    /// Optional client-type label (e.g. "browser", "cli", "agent").
    pub client_type: String,
    /// Optional user-agent string.
    pub user_agent: String,
}

// activated, formerly F-GAP-51
impl Default for ConnectionMetadata {
    fn default() -> Self {
        let connected_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            connected_at,
            client_type: String::from("unknown"),
            user_agent: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// WsSender
// ---------------------------------------------------------------------------

/// Channel wrapper for sending messages to a single WebSocket connection.
// activated, formerly F-GAP-51
#[derive(Debug)]
pub struct WsSender {
    /// The unbounded sender used to push messages into the connection's task.
    pub sender: UnboundedSender<WsMessage>,
    /// Connection metadata.
    pub metadata: ConnectionMetadata,
    /// Number of reconnection attempts (0 = first connection).
    pub reconnect_count: u64,
    /// Timestamp of the last heartbeat ping sent to this connection.
    pub last_heartbeat: Instant,
}

// activated, formerly F-GAP-51
impl WsSender {
    /// Create a new `WsSender` wrapping the given channel sender.
    pub fn new(sender: UnboundedSender<WsMessage>, metadata: ConnectionMetadata) -> Self {
        Self {
            sender,
            metadata,
            reconnect_count: 0,
            last_heartbeat: Instant::now(),
        }
    }

    /// Send a message to this connection. Returns `true` if the message was
    /// successfully enqueued, `false` if the receiver has been dropped.
    pub fn send(&self, message: WsMessage) -> bool {
        self.sender.send(message).is_ok()
    }

    /// Increment the reconnect counter.
    ///
    /// Includes a decay mechanism: if the connection has been alive and stable
    /// for more than 60 seconds since the last heartbeat, the counter is
    /// reset to 0 before incrementing. This prevents unbounded backoff growth
    /// on intermittently flaky connections that later stabilise.
    pub fn record_reconnect(&mut self) {
        const DECAY_THRESHOLD_SECS: u64 = 60;
        // If the connection has been stable for longer than the threshold,
        // treat this as a fresh reconnection rather than a continuation of
        // an old backoff series.
        if self.last_heartbeat.elapsed().as_secs() >= DECAY_THRESHOLD_SECS {
            self.reconnect_count = 0;
        }
        self.reconnect_count = self.reconnect_count.saturating_add(1);
    }

    /// Reset the reconnect counter to zero.
    ///
    /// Call this after a successful long-lived connection has been
    /// re-established to allow the backoff strategy to start fresh.
    pub fn reset_reconnect_count(&mut self) {
        self.reconnect_count = 0;
    }
}

// ---------------------------------------------------------------------------
// WebSocketConfig
// ---------------------------------------------------------------------------

/// Configuration for the WebSocket hub.
// activated, formerly F-GAP-51
#[derive(Debug, Clone)]
pub struct WebSocketConfig {
    /// Maximum number of concurrent connections (default 1000).
    pub max_connections: usize,
    /// Interval (seconds) between heartbeat pings (default 30).
    pub heartbeat_interval_secs: u64,
    /// Maximum number of buffered messages per connection (default 256).
    pub message_buffer_size: usize,
}

// activated, formerly F-GAP-51
impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            max_connections: 1000,
            heartbeat_interval_secs: 30,
            message_buffer_size: 256,
        }
    }
}

// ---------------------------------------------------------------------------
// ReconnectHint
// ---------------------------------------------------------------------------

/// Hint sent to a client about recommended reconnection timing.
// activated, formerly F-GAP-51
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconnectHint {
    /// Backoff delay in seconds that the client should wait before retrying.
    pub delay_secs: u64,
    /// Current attempt number.
    pub attempt: u64,
    /// Whether this is the final attempt before permanent rejection.
    pub final_attempt: bool,
}

/// Compute exponential backoff delay (seconds) for the given reconnection count.
///
/// Uses the formula: `min(base * 2^attempt, max_delay)` with ±25% jitter.
// activated, formerly F-GAP-51
pub fn exponential_backoff(attempt: u64, base_secs: u64, max_secs: u64) -> u64 {
    use std::cmp::min;

    let delay = base_secs.saturating_mul(2u64.saturating_pow(attempt as u32));
    let clamped = min(delay, max_secs);

    // Simple pseudo-jitter: add 0–25% of the clamped value.
    let jitter_range = clamped / 4;
    if jitter_range == 0 {
        clamped
    } else {
        let jitter = fastrand::u64(0..=jitter_range);
        clamped + jitter
    }
}

// activated, formerly F-GAP-51
impl ReconnectHint {
    /// Build a hint for a client that has attempted reconnection `reconnect_count` times.
    pub fn new(reconnect_count: u64) -> Self {
        let delay_secs = exponential_backoff(reconnect_count, 1, 60);
        Self {
            delay_secs,
            attempt: reconnect_count,
            final_attempt: reconnect_count >= 10,
        }
    }
}

// ---------------------------------------------------------------------------
// HeartbeatPing / HeartbeatPong
// ---------------------------------------------------------------------------

/// Message sent from the hub to a connection to check liveness.
#[allow(dead_code)] // activated, formerly F-GAP-51 — public API surface
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatPing {
    /// Monotonically increasing ping sequence number.
    pub seq: u64,
    /// Server timestamp (Unix seconds).
    pub timestamp: u64,
}

/// Expected response from a connection acknowledging a heartbeat ping.
#[allow(dead_code)] // activated, formerly F-GAP-51 — public API surface
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatPong {
    /// Echoed ping sequence number.
    pub seq: u64,
    /// Client timestamp (Unix seconds).
    pub timestamp: u64,
}

// ---------------------------------------------------------------------------
// WebSocketHub
// ---------------------------------------------------------------------------

/// Thread-safe, clonable hub that manages WebSocket connections,
/// topic subscriptions, heartbeats, and message broadcasting.
// activated, formerly F-GAP-51
#[derive(Debug, Clone)]
pub struct WebSocketHub {
    inner: Arc<WebSocketHubInner>,
}

#[derive(Debug)]
struct WebSocketHubInner {
    /// All active connections keyed by their ConnectionId.
    connections: RwLock<HashMap<ConnectionId, WsSender>>,
    /// Topic → list of subscribed ConnectionIds.
    topic_subscriptions: RwLock<HashMap<String, Vec<ConnectionId>>>,
    /// Hub configuration.
    config: WebSocketConfig,
    /// Handle for the background heartbeat task.
    heartbeat_handle: RwLock<Option<JoinHandle<()>>>,
    /// Monotonically increasing heartbeat sequence counter.
    heartbeat_seq: RwLock<u64>,
}

// activated, formerly F-GAP-51
impl WebSocketHub {
    /// Create a new hub with the given configuration.
    ///
    /// Once created, call [`start_heartbeat`] to begin the background keep-alive
    /// loop, or rely on it being auto-started on first `register`.
    pub fn new(config: WebSocketConfig) -> Self {
        Self {
            inner: Arc::new(WebSocketHubInner {
                connections: RwLock::new(HashMap::new()),
                topic_subscriptions: RwLock::new(HashMap::new()),
                config,
                heartbeat_handle: RwLock::new(None),
                heartbeat_seq: RwLock::new(0),
            }),
        }
    }
}

impl Default for WebSocketHub {
    fn default() -> Self {
        Self::new(WebSocketConfig::default())
    }
}

impl WebSocketHub {
    /// Start the background heartbeat task if it isn't already running.
    ///
    /// The task will ping all connections at the configured interval and
    /// remove any that have been unresponsive for more than one interval.
    pub async fn start_heartbeat(&self) {
        let mut handle_lock = self.inner.heartbeat_handle.write().await;
        if handle_lock.is_some() {
            debug!("heartbeat task already running");
            return;
        }

        let inner = Arc::clone(&self.inner);
        let interval = Duration::from_secs(inner.config.heartbeat_interval_secs);
        let stale_timeout = inner.config.heartbeat_interval_secs * 2;

        let handle: JoinHandle<()> = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;

                // Bump and read the heartbeat sequence number.
                let mut seq = inner.heartbeat_seq.write().await;
                *seq += 1;
                let current_seq = *seq;
                drop(seq);

                let timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                let ping = WsMessage::new(
                    "heartbeat.ping",
                    serde_json::json!({
                        "seq": current_seq,
                        "timestamp": timestamp,
                    }),
                );

                let mut conns = inner.connections.write().await;
                let stale_threshold = Instant::now() - Duration::from_secs(stale_timeout);

                // Separate stale connections for removal.
                let stale_ids: Vec<ConnectionId> = conns
                    .iter()
                    .filter(|(_, sender)| sender.last_heartbeat < stale_threshold)
                    .map(|(id, _)| id.clone())
                    .collect();

                for id in &stale_ids {
                    warn!(connection_id = %id, "removing stale connection (heartbeat timeout)");
                    conns.remove(id);
                }

                // Send ping to remaining connections.
                for (conn_id, sender) in conns.iter_mut() {
                    if !sender.send(ping.clone()) {
                        debug!(connection_id = %conn_id, "connection channel closed, removing");
                    }
                }

                let remaining = conns.len();
                drop(conns);

                info!(
                    heartbeat_seq = current_seq,
                    active_connections = remaining,
                    stale_removed = stale_ids.len(),
                    "heartbeat cycle complete"
                );
            }
        });

        *handle_lock = Some(handle);
        info!("heartbeat task started");
    }

    /// Gracefully shut down the heartbeat task.
    pub async fn stop_heartbeat(&self) {
        let mut handle_lock = self.inner.heartbeat_handle.write().await;
        if let Some(handle) = handle_lock.take() {
            handle.abort();
            info!("heartbeat task stopped");
        }
    }

    /// Register a new connection with the hub.
    ///
    /// Returns an `UnboundedReceiver<WsMessage>` that the caller should use
    /// to forward messages to the actual WebSocket transport.
    ///
    /// # Errors
    ///
    /// Returns an error if the maximum number of connections has been reached.
    pub async fn register(
        &self,
        metadata: ConnectionMetadata,
    ) -> Result<(ConnectionId, UnboundedReceiver<WsMessage>), WebSocketError> {
        let mut conns = self.inner.connections.write().await;

        if conns.len() >= self.inner.config.max_connections {
            return Err(WebSocketError::MaxConnectionsReached(
                self.inner.config.max_connections,
            ));
        }

        let connection_id = Uuid::new_v4().to_string();
        let (tx, rx) = unbounded_channel();
        let sender = WsSender::new(tx, metadata);

        conns.insert(connection_id.clone(), sender);

        debug!(
            connection_id = %connection_id,
            total_connections = conns.len(),
            "connection registered"
        );

        drop(conns);

        // Ensure heartbeat is running.
        self.start_heartbeat().await;

        Ok((connection_id, rx))
    }

    /// Unregister a connection and remove all its topic subscriptions.
    pub async fn unregister(&self, connection_id: &str) {
        let mut conns = self.inner.connections.write().await;
        conns.remove(connection_id);
        drop(conns);

        // Remove from all topic subscriptions.
        let mut subs = self.inner.topic_subscriptions.write().await;
        for (_topic, members) in subs.iter_mut() {
            members.retain(|id| id != connection_id);
        }
        // Clean up empty topic entries.
        subs.retain(|_topic, members| !members.is_empty());

        debug!(
            connection_id = %connection_id,
            "connection unregistered"
        );
    }

    /// Subscribe a connection to a topic.
    ///
    /// After subscribing, the connection will receive all messages published
    /// to the given topic.
    pub async fn subscribe(&self, connection_id: &str, topic: &str) {
        let mut subs = self.inner.topic_subscriptions.write().await;
        let members = subs.entry(topic.to_string()).or_default();
        if !members.contains(&connection_id.to_string()) {
            members.push(connection_id.to_string());
        }
        debug!(
            connection_id = %connection_id,
            topic = %topic,
            "subscription added"
        );
    }

    /// Unsubscribe a connection from a topic.
    pub async fn unsubscribe(&self, connection_id: &str, topic: &str) {
        let mut subs = self.inner.topic_subscriptions.write().await;
        if let Some(members) = subs.get_mut(topic) {
            members.retain(|id| id != connection_id);
            if members.is_empty() {
                subs.remove(topic);
            }
        }
        debug!(
            connection_id = %connection_id,
            topic = %topic,
            "subscription removed"
        );
    }

    /// Publish a message to all subscribers of a topic.
    ///
    /// Also publishes to connections that subscribed via wildcard prefixes
    /// (e.g. a subscription to `task.*` will match `task.abc.progress`).
    pub async fn publish(&self, topic: &str, message: WsMessage) {
        let subs = self.inner.topic_subscriptions.read().await;
        let conns = self.inner.connections.read().await;

        // Collect all connection ids that match the topic (exact or wildcard).
        // Only exact matches and wildcard-path matches are supported.
        let mut targets: Vec<ConnectionId> = Vec::new();

        // 1. Exact match.
        if let Some(members) = subs.get(topic) {
            targets.extend(members.iter().cloned());
        }

        // 2. Wildcard prefix matches: topics ending with `.*`
        //    e.g. subscription to `task.*` matches `task.abc.progress`
        let topic_parts: Vec<&str> = topic.splitn(3, '.').collect();
        if topic_parts.len() >= 2 {
            let wildcard = format!("{}.*", topic_parts[0]);
            if let Some(members) = subs.get(&wildcard) {
                for id in members {
                    if !targets.contains(id) {
                        targets.push(id.clone());
                    }
                }
            }
        }
        if topic_parts.len() >= 3 {
            let wildcard = format!("{}.{}.*", topic_parts[0], topic_parts[1]);
            if let Some(members) = subs.get(&wildcard) {
                for id in members {
                    if !targets.contains(id) {
                        targets.push(id.clone());
                    }
                }
            }
        }

        drop(subs);

        // Send to each target.
        for conn_id in &targets {
            if let Some(sender) = conns.get(conn_id) {
                sender.send(message.clone());
            }
        }

        let sent_count = targets.len();
        drop(conns);

        debug!(
            topic = %topic,
            recipients = sent_count,
            "message published"
        );
    }

    /// Send a message directly to a specific connection.
    ///
    /// Returns `true` if the connection exists and the message was enqueued.
    pub async fn send(&self, connection_id: &str, message: WsMessage) -> bool {
        let conns = self.inner.connections.read().await;
        if let Some(sender) = conns.get(connection_id) {
            sender.send(message)
        } else {
            false
        }
    }

    /// Broadcast a message to every connected client.
    pub async fn broadcast(&self, message: WsMessage) {
        let conns = self.inner.connections.read().await;
        let count = conns.len();
        for (_conn_id, sender) in conns.iter() {
            sender.send(message.clone());
        }
        drop(conns);
        debug!(recipients = count, "message broadcast");
    }

    /// Return the number of active connections.
    pub async fn get_connection_count(&self) -> usize {
        self.inner.connections.read().await.len()
    }

    /// Return a list of all topics that currently have subscribers.
    pub async fn get_active_topics(&self) -> Vec<String> {
        self.inner
            .topic_subscriptions
            .read()
            .await
            .keys()
            .cloned()
            .collect()
    }

    /// Return metadata for a specific connection, if it exists.
    pub async fn get_connection_metadata(&self, connection_id: &str) -> Option<ConnectionMetadata> {
        let conns = self.inner.connections.read().await;
        conns.get(connection_id).map(|s| s.metadata.clone())
    }

    /// Update the reconnect count for a connection (used when a client
    /// re-establishes a dropped connection with the same logical identity).
    pub async fn record_reconnect(&self, connection_id: &str) {
        let mut conns = self.inner.connections.write().await;
        if let Some(sender) = conns.get_mut(connection_id) {
            sender.record_reconnect();
        }
    }

    /// Reset the reconnect count for a connection to zero.
    ///
    /// Call this after a successful reconnection to prevent unbounded
    /// exponential backoff growth.
    pub async fn reset_reconnect(&self, connection_id: &str) {
        let mut conns = self.inner.connections.write().await;
        if let Some(sender) = conns.get_mut(connection_id) {
            sender.reset_reconnect_count();
        }
    }

    /// Get a reconnection hint for the given connection, based on its
    /// current reconnect count.
    pub async fn get_reconnect_hint(&self, connection_id: &str) -> Option<ReconnectHint> {
        let conns = self.inner.connections.read().await;
        conns
            .get(connection_id)
            .map(|sender| ReconnectHint::new(sender.reconnect_count))
    }

    /// Check if a connection is currently registered.
    pub async fn is_connected(&self, connection_id: &str) -> bool {
        self.inner
            .connections
            .read()
            .await
            .contains_key(connection_id)
    }

    /// Create a broadcast function that publishes messages to all WebSocket connections.
    /// Used to wire SessionRegistry changes to WebSocket clients.
    pub fn create_broadcast_fn(self: &Arc<Self>) -> crate::protocol::session_sync::BroadcastFn {
        let hub = self.clone();
        Arc::new(move |msg: &str| {
            let hub = hub.clone();
            let payload: serde_json::Value = serde_json::from_str(msg).unwrap_or_default();
            tokio::spawn(async move {
                let ws_msg = WsMessage::new("session.sync", payload);
                hub.broadcast(ws_msg).await;
            });
        })
    }
}

// ---------------------------------------------------------------------------
// WebSocketError
// ---------------------------------------------------------------------------

/// Errors that can occur within the WebSocket hub.
#[derive(Debug, thiserror::Error)]
pub enum WebSocketError {
    /// The maximum number of concurrent connections has been reached.
    #[error("max connections reached ({0})")]
    MaxConnectionsReached(usize),
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_msg(payload: &str) -> WsMessage {
        WsMessage::new("test.event", json!({"data": payload}))
    }

    #[tokio::test]
    async fn test_register_and_connection_count() {
        let hub = WebSocketHub::default();
        assert_eq!(hub.get_connection_count().await, 0);

        let (id, _rx) = hub
            .register(ConnectionMetadata::default())
            .await
            .expect("register should succeed");
        assert_eq!(hub.get_connection_count().await, 1);
        assert!(hub.is_connected(&id).await);

        hub.unregister(&id).await;
        assert_eq!(hub.get_connection_count().await, 0);
        assert!(!hub.is_connected(&id).await);
    }

    #[tokio::test]
    async fn test_max_connections() {
        let config = WebSocketConfig {
            max_connections: 2,
            ..Default::default()
        };
        let hub = WebSocketHub::new(config);

        let (_id1, _rx1) = hub.register(ConnectionMetadata::default()).await.unwrap();
        let (_id2, _rx2) = hub.register(ConnectionMetadata::default()).await.unwrap();

        let err = hub
            .register(ConnectionMetadata::default())
            .await
            .unwrap_err();
        assert!(matches!(err, WebSocketError::MaxConnectionsReached(2)));
    }

    #[tokio::test]
    async fn test_send_to_specific_connection() {
        let hub = WebSocketHub::default();
        let (id, mut rx) = hub.register(ConnectionMetadata::default()).await.unwrap();

        let msg = test_msg("hello");
        let sent = hub.send(&id, msg.clone()).await;
        assert!(sent);

        let received = rx.recv().await.expect("should receive message");
        assert_eq!(received.msg_type, "test.event");
    }

    #[tokio::test]
    async fn test_send_to_nonexistent_connection() {
        let hub = WebSocketHub::default();
        let sent = hub.send("nonexistent", test_msg("nope")).await;
        assert!(!sent);
    }

    #[tokio::test]
    async fn test_broadcast() {
        let hub = WebSocketHub::default();
        let (_id1, mut rx1) = hub.register(ConnectionMetadata::default()).await.unwrap();
        let (_id2, mut rx2) = hub.register(ConnectionMetadata::default()).await.unwrap();

        let msg = test_msg("broadcast");
        hub.broadcast(msg).await;

        let r1 = rx1.recv().await.expect("rx1 should receive");
        let r2 = rx2.recv().await.expect("rx2 should receive");
        assert_eq!(r1.payload, json!({"data": "broadcast"}));
        assert_eq!(r2.payload, json!({"data": "broadcast"}));
    }

    #[tokio::test]
    async fn test_topic_publish_and_subscribe() {
        let hub = WebSocketHub::default();
        let (id1, mut rx1) = hub.register(ConnectionMetadata::default()).await.unwrap();
        let (id2, mut rx2) = hub.register(ConnectionMetadata::default()).await.unwrap();

        hub.subscribe(&id1, "task.abc.progress").await;
        hub.subscribe(&id2, "task.abc.progress").await;

        let msg = WsMessage::new("task.progress", json!({"percent": 50}));
        hub.publish("task.abc.progress", msg).await;

        let received_1 = rx1.recv().await.expect("subscriber 1 should receive");
        let received_2 = rx2.recv().await.expect("subscriber 2 should receive");
        assert_eq!(received_1.payload["percent"], 50);
        assert_eq!(received_2.payload["percent"], 50);
    }

    #[tokio::test]
    async fn test_unsubscribe() {
        let hub = WebSocketHub::default();
        let (id, mut rx) = hub.register(ConnectionMetadata::default()).await.unwrap();

        // Stop the heartbeat to prevent ping messages from interfering.
        hub.stop_heartbeat().await;

        hub.subscribe(&id, "test.topic").await;
        hub.unsubscribe(&id, "test.topic").await;

        hub.publish("test.topic", test_msg("after-unsub")).await;

        // The receiver should NOT get the published message after unsubscribing.
        let result = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
        assert!(result.is_err(), "should not receive after unsubscribe");
    }

    #[tokio::test]
    async fn test_publish_to_no_subscribers() {
        let hub = WebSocketHub::default();
        // Publishing without any subscribers should not panic.
        hub.publish("orphan.topic", test_msg("orphan")).await;
    }

    #[tokio::test]
    async fn test_unregister_removes_subscriptions() {
        let hub = WebSocketHub::default();
        let (id, _rx) = hub.register(ConnectionMetadata::default()).await.unwrap();
        hub.subscribe(&id, "my.topic").await;

        hub.unregister(&id).await;

        let topics = hub.get_active_topics().await;
        assert!(topics.is_empty(), "topic should be cleaned up");
    }

    #[tokio::test]
    async fn test_get_active_topics() {
        let hub = WebSocketHub::default();
        let (id1, _rx1) = hub.register(ConnectionMetadata::default()).await.unwrap();
        let (id2, _rx2) = hub.register(ConnectionMetadata::default()).await.unwrap();

        hub.subscribe(&id1, "alpha").await;
        hub.subscribe(&id2, "beta").await;
        hub.subscribe(&id1, "gamma").await;

        let mut topics = hub.get_active_topics().await;
        topics.sort();
        assert_eq!(topics, vec!["alpha", "beta", "gamma"]);
    }

    #[tokio::test]
    async fn test_connection_metadata() {
        let hub = WebSocketHub::default();
        let metadata = ConnectionMetadata {
            client_type: "browser".into(),
            user_agent: "Mozilla/5.0".into(),
            ..Default::default()
        };
        let (id, _rx) = hub.register(metadata.clone()).await.unwrap();

        let retrieved = hub
            .get_connection_metadata(&id)
            .await
            .expect("metadata should exist");
        assert_eq!(retrieved.client_type, "browser");
        assert_eq!(retrieved.user_agent, "Mozilla/5.0");
    }

    #[tokio::test]
    async fn test_reconnect_hint() {
        let hub = WebSocketHub::default();
        let (id, _rx) = hub.register(ConnectionMetadata::default()).await.unwrap();

        // Initial reconnect count is 0.
        let hint = hub
            .get_reconnect_hint(&id)
            .await
            .expect("hint should exist");
        assert_eq!(hint.attempt, 0);
        assert!(!hint.final_attempt);

        hub.record_reconnect(&id).await;
        hub.record_reconnect(&id).await;

        let hint = hub
            .get_reconnect_hint(&id)
            .await
            .expect("hint should exist");
        assert_eq!(hint.attempt, 2);
    }

    #[tokio::test]
    async fn test_reconnect_hint_nonexistent() {
        let hub = WebSocketHub::default();
        let hint = hub.get_reconnect_hint("ghost").await;
        assert!(hint.is_none());
    }

    #[test]
    fn test_exponential_backoff_base() {
        // attempt 0 → base = 1
        let delay = exponential_backoff(0, 1, 60);
        assert!((1..=2).contains(&delay), "delay was {delay}");
    }

    #[test]
    fn test_exponential_backoff_capped() {
        // attempt 8 → 1 * 2^8 = 256, capped at max 60
        let delay = exponential_backoff(8, 1, 60);
        assert!((60..=75).contains(&delay), "delay was {delay}");
    }

    #[test]
    fn test_exponential_backoff_max_zero() {
        let delay = exponential_backoff(5, 1, 0);
        assert_eq!(delay, 0);
    }

    #[tokio::test]
    async fn test_start_and_stop_heartbeat() {
        let hub = WebSocketHub::default();

        // Start should succeed.
        hub.start_heartbeat().await;

        // Starting again should be a no-op.
        hub.start_heartbeat().await;

        // Stop should succeed.
        hub.stop_heartbeat().await;
    }

    #[tokio::test]
    async fn test_hub_is_clonable() {
        let hub = WebSocketHub::default();
        let hub2 = hub.clone();

        let (id, _rx) = hub.register(ConnectionMetadata::default()).await.unwrap();
        assert!(hub2.is_connected(&id).await);
    }

    #[tokio::test]
    async fn test_ws_message_timestamp() {
        let base = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let msg = WsMessage::new("test", json!({}));
        assert!(msg.timestamp >= base);
    }

    #[tokio::test]
    async fn test_wildcard_subscription() {
        let hub = WebSocketHub::default();
        let (id, mut rx) = hub.register(ConnectionMetadata::default()).await.unwrap();

        // Subscribe with a wildcard pattern `task.*`
        hub.subscribe(&id, "task.*").await;

        // Publish to a sub-topic — wildcard should match.
        let msg = WsMessage::new("task.progress", json!({"id": "abc", "percent": 75}));
        hub.publish("task.abc.progress", msg).await;

        let received = rx.recv().await.expect("wildcard subscriber should receive");
        assert_eq!(received.payload["percent"], 75);
        assert_eq!(received.payload["id"], "abc");
    }

    #[tokio::test]
    async fn test_multiple_publishes_to_subscriber() {
        let hub = WebSocketHub::default();
        let (id, mut rx) = hub.register(ConnectionMetadata::default()).await.unwrap();
        hub.subscribe(&id, "updates").await;

        for i in 0..5 {
            hub.publish("updates", WsMessage::new("update", json!({"i": i})))
                .await;
        }

        for i in 0..5 {
            let msg = rx.recv().await.expect("should receive all messages");
            assert_eq!(msg.payload["i"], i);
        }
    }

    #[tokio::test]
    async fn test_clone_independent_heartbeat() {
        // Verify that cloned hubs share state (they use the same Arc).
        let hub = WebSocketHub::default();
        let hub_clone = hub.clone();

        hub.start_heartbeat().await;
        assert!(hub_clone.inner.heartbeat_handle.read().await.is_some());

        hub.stop_heartbeat().await;
        assert!(hub_clone.inner.heartbeat_handle.read().await.is_none());
    }
}
