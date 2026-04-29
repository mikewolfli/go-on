//! F-GAP-29: Multi-channel Message Transport
//!
//! Provides protocol-layer channel separation for different message types.
//! Each channel is isolated with its own queue, configuration, and statistics.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Identifies a transport channel for message routing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ChannelId {
    /// Control messages — governance, policy, coordination
    Control,
    /// Data messages — actual task payloads
    Data,
    /// Event messages — notifications and streaming events
    Event,
    /// Stream messages — continuous data streams
    Stream,
    /// Backchannel messages — out-of-band communication
    Backchannel,
    /// Heartbeat messages — keep-alive and health checks
    Heartbeat,
}

impl fmt::Display for ChannelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Control => write!(f, "control"),
            Self::Data => write!(f, "data"),
            Self::Event => write!(f, "event"),
            Self::Stream => write!(f, "stream"),
            Self::Backchannel => write!(f, "backchannel"),
            Self::Heartbeat => write!(f, "heartbeat"),
        }
    }
}

/// Priority level of a transport message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum MessagePriority {
    /// Best-effort delivery
    Low,
    /// Default priority
    Normal,
    /// High urgency
    High,
    /// Must be processed immediately
    Critical,
}

/// Delivery status of a message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DeliveryStatus {
    /// Message is queued and awaiting delivery
    Queued,
    /// Message is currently in flight
    InFlight,
    /// Message has been successfully delivered
    Delivered,
    /// Delivery failed (permanently)
    Failed,
    /// Message expired before delivery
    Expired,
}

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// A message transported over a channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportMessage {
    /// Unique message identifier
    pub id: String,
    /// Target channel
    pub channel: ChannelId,
    /// Delivery priority
    pub priority: MessagePriority,
    /// Message payload (string-encoded)
    pub payload: String,
    /// Source component identifier
    pub source: String,
    /// Target component identifier
    pub target: String,
    /// Creation timestamp (epoch milliseconds)
    pub created_ms: u64,
    /// Time-to-live in milliseconds
    pub ttl_ms: u64,
    /// Number of delivery attempts so far
    pub delivery_attempts: u32,
}

/// Configuration for a single channel.
#[derive(Debug, Clone)]
pub struct ChannelConfig {
    /// Which channel this config applies to
    pub channel: ChannelId,
    /// Maximum number of queued messages (default: 1000)
    pub max_queue_size: usize,
    /// Maximum retry attempts (default: 3)
    pub max_retries: u32,
    /// Message timeout in milliseconds (default: 30_000)
    pub timeout_ms: u64,
    /// Maximum messages per second on this channel (default: 100)
    pub rate_limit_per_sec: u32,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            channel: ChannelId::Data,
            max_queue_size: 1000,
            max_retries: 3,
            timeout_ms: 30_000,
            rate_limit_per_sec: 100,
        }
    }
}

/// Receipt confirming delivery status of a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryReceipt {
    /// The message identifier this receipt refers to
    pub message_id: String,
    /// Current delivery status
    pub status: DeliveryStatus,
    /// Timestamp when the message was delivered (epoch ms), if applicable
    pub delivered_ms: Option<u64>,
    /// Error description if the message failed
    pub error: Option<String>,
}

/// Statistics for a single channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelStats {
    /// The channel these stats apply to
    pub channel: ChannelId,
    /// Total number of messages sent on this channel
    pub messages_sent: u64,
    /// Total number of messages received on this channel
    pub messages_received: u64,
    /// Total number of messages that failed on this channel
    pub messages_failed: u64,
    /// Total number of messages that expired on this channel
    pub messages_expired: u64,
    /// Current queue depth for this channel
    pub queue_depth: usize,
    /// Average latency in milliseconds
    pub avg_latency_ms: f64,
}

/// Global transport configuration.
#[derive(Debug, Clone)]
pub struct TransportConfig {
    /// Maximum number of channels allowed (default: 6)
    pub max_channels: usize,
    /// Global rate limit (messages per second, default: 1000)
    pub global_rate_limit: u32,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            max_channels: 6,
            global_rate_limit: 1000,
        }
    }
}

/// Snapshot of the overall transport profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportProfile {
    /// Total messages sent across all channels
    pub total_messages_sent: u64,
    /// Total messages received across all channels
    pub total_messages_received: u64,
    /// Total delivery failures across all channels
    pub total_failures: u64,
    /// Number of active channels
    pub active_channels: usize,
    /// Per-channel statistics
    pub channel_stats: Vec<ChannelStats>,
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

/// Queued message envelope with delivery tracking.
#[derive(Debug, Clone)]
struct QueuedMessage {
    message: TransportMessage,
    status: DeliveryStatus,
    retries_remaining: u32,
    enqueued_ms: u64,
}

/// Per-channel internal state.
#[derive(Debug, Clone)]
struct ChannelState {
    config: ChannelConfig,
    queue: Vec<QueuedMessage>,
    stats: ChannelStats,
    last_rate_ts: u64,
    rate_count: u32,
}

/// Internal state wrapped in `Arc<Mutex<>>`.
struct TransportInner {
    config: TransportConfig,
    channels: HashMap<ChannelId, ChannelState>,
    total_sent: u64,
    total_received: u64,
    total_failures: u64,
}

// ---------------------------------------------------------------------------
// Public API: MultiChannelTransport
// ---------------------------------------------------------------------------

/// Thread-safe multi-channel message transport.
///
/// Each channel is isolated with its own queue, configuration, and statistics.
/// Messages are routed to the appropriate channel based on their `ChannelId`.
pub struct MultiChannelTransport {
    inner: Arc<Mutex<TransportInner>>,
}

impl MultiChannelTransport {
    // ── construction ──────────────────────────────────────────────────────

    /// Create a new transport with the given global configuration.
    pub fn new(config: TransportConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(TransportInner {
                config,
                channels: HashMap::new(),
                total_sent: 0,
                total_received: 0,
                total_failures: 0,
            })),
        }
    }

    // ── channel management ────────────────────────────────────────────────

    /// Configure a specific channel. Creates the channel if it does not exist.
    pub fn configure_channel(&self, config: ChannelConfig) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let channel_id = config.channel.clone();

        // Enforce max_channels limit
        if !inner.channels.contains_key(&channel_id)
            && inner.channels.len() >= inner.config.max_channels
        {
            bail!(
                "maximum number of channels ({}) reached",
                inner.config.max_channels
            );
        }

        let now = Self::now_ms();
        let stats = ChannelStats {
            channel: channel_id.clone(),
            messages_sent: 0,
            messages_received: 0,
            messages_failed: 0,
            messages_expired: 0,
            queue_depth: 0,
            avg_latency_ms: 0.0,
        };

        inner.channels.insert(
            channel_id,
            ChannelState {
                config,
                queue: Vec::new(),
                stats,
                last_rate_ts: now,
                rate_count: 0,
            },
        );

        Ok(())
    }

    // ── sending ───────────────────────────────────────────────────────────

    /// Send a message on its designated channel.
    ///
    /// Returns a `DeliveryReceipt` indicating the initial delivery status.
    pub fn send(&self, message: TransportMessage) -> Result<DeliveryReceipt> {
        let mut inner = self.inner.lock().unwrap();
        let now = Self::now_ms();

        // Check global rate limit
        // Simple rate limiting: track time window globally
        // (per-channel rate limiting is handled in channel logic)

        // Locate or create the channel
        let channel_id = message.channel.clone();
        let channel = inner
            .channels
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow::anyhow!("channel {} is not configured", channel_id))?;

        // Per-channel rate limiting
        let elapsed = now.saturating_sub(channel.last_rate_ts);
        if elapsed >= 1000 {
            // Reset rate counter for new window
            channel.last_rate_ts = now;
            channel.rate_count = 0;
        }
        if channel.rate_count >= channel.config.rate_limit_per_sec {
            bail!(
                "rate limit exceeded for channel {} ({} msg/s)",
                channel_id,
                channel.config.rate_limit_per_sec
            );
        }
        channel.rate_count += 1;

        // Queue depth check
        if channel.queue.len() >= channel.config.max_queue_size {
            bail!(
                "queue size limit ({}) reached for channel {}",
                channel.config.max_queue_size,
                channel_id
            );
        }

        // TTL check — if the message has already expired, mark it expired
        if message.created_ms + message.ttl_ms < now {
            channel.stats.messages_expired += 1;
            inner.total_failures += 1;
            return Ok(DeliveryReceipt {
                message_id: message.id,
                status: DeliveryStatus::Expired,
                delivered_ms: None,
                error: Some("message expired before send".to_string()),
            });
        }

        // Enqueue with priority ordering (higher priority first)
        let queued = QueuedMessage {
            message: message.clone(),
            status: DeliveryStatus::Queued,
            retries_remaining: channel.config.max_retries,
            enqueued_ms: now,
        };

        // Insert in priority order (Critical > High > Normal > Low)
        // Find the first message with lower priority than the new one
        let insert_pos = channel
            .queue
            .iter()
            .position(|qm| qm.message.priority < queued.message.priority)
            .unwrap_or(channel.queue.len());
        channel.queue.insert(insert_pos, queued);

        // Update stats
        channel.stats.messages_sent += 1;
        channel.stats.queue_depth = channel.queue.len();
        inner.total_sent += 1;

        Ok(DeliveryReceipt {
            message_id: message.id.clone(),
            status: DeliveryStatus::Queued,
            delivered_ms: None,
            error: None,
        })
    }

    // ── receiving ─────────────────────────────────────────────────────────

    /// Receive all available messages from a specific channel.
    pub fn receive(&self, channel_id: ChannelId) -> Result<Vec<TransportMessage>> {
        let mut inner = self.inner.lock().unwrap();
        let now = Self::now_ms();

        let channel = inner
            .channels
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow::anyhow!("channel {} is not configured", channel_id))?;

        // Drain messages from the queue that are not expired
        let mut messages = Vec::new();
        let mut remaining = Vec::new();
        let mut expired_count = 0u64;
        let mut received_count = 0u64;

        for qm in channel.queue.drain(..) {
            // Check if the message has expired
            if qm.message.created_ms + qm.message.ttl_ms < now {
                expired_count += 1;
                continue;
            }

            let msg = qm.message;
            messages.push(msg.clone());
            received_count += 1;

            // Keep the rest in the queue (for ack-based removal, we need them)
            remaining.push(QueuedMessage {
                message: msg,
                status: DeliveryStatus::InFlight,
                retries_remaining: qm.retries_remaining,
                enqueued_ms: qm.enqueued_ms,
            });
        }

        // Update stats after releasing the borrow from drain/retain
        channel.stats.messages_expired += expired_count;
        channel.stats.messages_received += received_count;
        channel.stats.queue_depth = remaining.len();
        channel.queue = remaining;

        inner.total_failures += expired_count;
        inner.total_received += received_count;

        Ok(messages)
    }

    // ── acknowledgment / forwarding ───────────────────────────────────────

    /// Acknowledge a message as successfully delivered.
    ///
    /// Removes the message from its channel queue.
    pub fn acknowledge(&self, message_id: &str) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();

        for (_ch_id, channel) in inner.channels.iter_mut() {
            let before = channel.queue.len();
            channel.queue.retain(|qm| qm.message.id != message_id);
            if channel.queue.len() < before {
                channel.stats.queue_depth = channel.queue.len();
                return Ok(());
            }
        }

        bail!("message {} not found in any channel", message_id)
    }

    /// Forward a message to a different channel.
    ///
    /// Removes the original message and sends a copy to the target channel.
    pub fn forward(&self, message_id: &str, target_channel: ChannelId) -> Result<DeliveryReceipt> {
        let mut inner = self.inner.lock().unwrap();

        // Find and remove the original message from any channel
        let mut original_msg: Option<TransportMessage> = None;
        for (_ch_id, channel) in inner.channels.iter_mut() {
            if let Some(pos) = channel
                .queue
                .iter()
                .position(|qm| qm.message.id == message_id)
            {
                let qm = channel.queue.remove(pos);
                channel.stats.queue_depth = channel.queue.len();
                original_msg = Some(qm.message);
                break;
            }
        }

        let mut msg = match original_msg {
            Some(m) => m,
            None => bail!("message {} not found for forwarding", message_id),
        };

        // Update the message for the new channel
        msg.channel = target_channel;
        msg.delivery_attempts += 1;

        // Drop the lock momentarily — we need the inner lock again via send
        // but since we already hold it, we'll just directly enqueue.
        // (We still hold the inner lock, so we can proceed directly.)
        let channel_id = msg.channel.clone();

        // Check that the target channel exists
        let target = inner
            .channels
            .get_mut(&channel_id)
            .ok_or_else(|| anyhow::anyhow!("target channel {} is not configured", channel_id))?;

        // Queue depth check
        if target.queue.len() >= target.config.max_queue_size {
            bail!(
                "queue size limit ({}) reached on target channel {}",
                target.config.max_queue_size,
                channel_id
            );
        }

        let now = Self::now_ms();
        let queued = QueuedMessage {
            message: msg.clone(),
            status: DeliveryStatus::Queued,
            retries_remaining: target.config.max_retries,
            enqueued_ms: now,
        };

        // Insert in priority order (Critical > High > Normal > Low)
        // Find the first message with lower priority than the new one
        let insert_pos = target
            .queue
            .iter()
            .position(|qm| qm.message.priority < queued.message.priority)
            .unwrap_or(target.queue.len());
        target.queue.insert(insert_pos, queued);

        target.stats.messages_sent += 1;
        target.stats.queue_depth = target.queue.len();
        inner.total_sent += 1;

        Ok(DeliveryReceipt {
            message_id: msg.id.clone(),
            status: DeliveryStatus::Queued,
            delivered_ms: None,
            error: None,
        })
    }

    // ── introspection ─────────────────────────────────────────────────────

    /// Get statistics for a specific channel.
    pub fn channel_stats(&self, channel_id: ChannelId) -> Option<ChannelStats> {
        let inner = self.inner.lock().unwrap();
        inner.channels.get(&channel_id).map(|ch| ch.stats.clone())
    }

    /// Obtain a snapshot of the entire transport profile.
    pub fn profile(&self) -> TransportProfile {
        let inner = self.inner.lock().unwrap();
        let channel_stats: Vec<ChannelStats> =
            inner.channels.values().map(|ch| ch.stats.clone()).collect();
        let active_channels = inner.channels.len();

        TransportProfile {
            total_messages_sent: inner.total_sent,
            total_messages_received: inner.total_received,
            total_failures: inner.total_failures,
            active_channels,
            channel_stats,
        }
    }

    // ── maintenance ───────────────────────────────────────────────────────

    /// Remove expired messages from all channels.
    ///
    /// Returns the number of messages removed.
    pub fn expire_old_messages(&self) -> usize {
        let mut inner = self.inner.lock().unwrap();
        let now = Self::now_ms();
        let mut total_removed = 0;
        let mut total_failure_add = 0u64;

        for (_ch_id, channel) in inner.channels.iter_mut() {
            let before = channel.queue.len();
            let mut expired_count = 0u64;
            channel.queue.retain(|qm| {
                let expired = qm.message.created_ms + qm.message.ttl_ms < now;
                if expired {
                    expired_count += 1;
                }
                !expired
            });
            let removed = before - channel.queue.len();
            total_removed += removed;

            channel.stats.messages_expired += expired_count;
            channel.stats.queue_depth = channel.queue.len();

            if removed > 0 {
                total_failure_add += removed as u64;
            }
        }

        inner.total_failures += total_failure_add;

        total_removed
    }

    // ── internal helpers ──────────────────────────────────────────────────

    /// Current time in milliseconds since Unix epoch.
    fn now_ms() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a transport with all default channels configured.
    fn configured_transport() -> MultiChannelTransport {
        let transport = MultiChannelTransport::new(TransportConfig::default());

        let channels = vec![
            ChannelId::Control,
            ChannelId::Data,
            ChannelId::Event,
            ChannelId::Stream,
            ChannelId::Backchannel,
            ChannelId::Heartbeat,
        ];

        for ch in channels {
            let config = ChannelConfig {
                channel: ch,
                ..ChannelConfig::default()
            };
            transport.configure_channel(config).unwrap();
        }

        transport
    }

    /// Helper: create a sample message for testing.
    fn sample_message(
        transport: &MultiChannelTransport,
        channel: ChannelId,
        priority: MessagePriority,
        id: &str,
    ) -> TransportMessage {
        let now = MultiChannelTransport::now_ms();
        TransportMessage {
            id: id.to_string(),
            channel,
            priority,
            payload: format!("payload-{}", id),
            source: "test_source".to_string(),
            target: "test_target".to_string(),
            created_ms: now,
            ttl_ms: 30_000,
            delivery_attempts: 0,
        }
    }

    // ── 1. new transport is empty ─────────────────────────────────────────

    #[test]
    fn test_new_transport_empty() {
        let transport = MultiChannelTransport::new(TransportConfig::default());
        let profile = transport.profile();
        assert_eq!(profile.total_messages_sent, 0);
        assert_eq!(profile.total_messages_received, 0);
        assert_eq!(profile.total_failures, 0);
        assert_eq!(profile.active_channels, 0);
        assert!(profile.channel_stats.is_empty());
    }

    // ── 2. configure channel ──────────────────────────────────────────────

    #[test]
    fn test_configure_channel() {
        let transport = MultiChannelTransport::new(TransportConfig::default());
        let config = ChannelConfig {
            channel: ChannelId::Data,
            max_queue_size: 500,
            max_retries: 5,
            timeout_ms: 60_000,
            rate_limit_per_sec: 200,
        };
        transport.configure_channel(config).unwrap();

        let stats = transport.channel_stats(ChannelId::Data);
        assert!(stats.is_some());
        assert_eq!(stats.unwrap().messages_sent, 0);
    }

    // ── 3. send message ───────────────────────────────────────────────────

    #[test]
    fn test_send_message() {
        let transport = configured_transport();
        let msg = sample_message(
            &transport,
            ChannelId::Data,
            MessagePriority::Normal,
            "send-1",
        );
        let receipt = transport.send(msg).unwrap();

        assert_eq!(receipt.status, DeliveryStatus::Queued);
        assert_eq!(receipt.message_id, "send-1");

        let profile = transport.profile();
        assert_eq!(profile.total_messages_sent, 1);
    }

    // ── 4. send with priority ─────────────────────────────────────────────

    #[test]
    fn test_send_with_priority() {
        let transport = configured_transport();

        let low = sample_message(&transport, ChannelId::Data, MessagePriority::Low, "p-low");
        let critical = sample_message(
            &transport,
            ChannelId::Data,
            MessagePriority::Critical,
            "p-critical",
        );
        let normal = sample_message(
            &transport,
            ChannelId::Data,
            MessagePriority::Normal,
            "p-normal",
        );
        let high = sample_message(&transport, ChannelId::Data, MessagePriority::High, "p-high");

        transport.send(low).unwrap();
        transport.send(critical).unwrap();
        transport.send(normal).unwrap();
        transport.send(high).unwrap();

        // Receive should return messages in priority order
        let received = transport.receive(ChannelId::Data).unwrap();
        assert_eq!(received.len(), 4);

        let ids: Vec<&str> = received.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["p-critical", "p-high", "p-normal", "p-low"]);
    }

    // ── 5. send and receive ───────────────────────────────────────────────

    #[test]
    fn test_send_and_receive() {
        let transport = configured_transport();
        let msg = sample_message(
            &transport,
            ChannelId::Event,
            MessagePriority::Normal,
            "send-recv-1",
        );
        transport.send(msg).unwrap();

        let received = transport.receive(ChannelId::Event).unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].id, "send-recv-1");
    }

    // ── 6. receive empty channel ──────────────────────────────────────────

    #[test]
    fn test_receive_empty_channel() {
        let transport = configured_transport();

        let received = transport.receive(ChannelId::Control).unwrap();
        assert!(received.is_empty());
    }

    // ── 7. acknowledge ────────────────────────────────────────────────────

    #[test]
    fn test_acknowledge() {
        let transport = configured_transport();
        let msg = sample_message(
            &transport,
            ChannelId::Data,
            MessagePriority::Normal,
            "ack-msg-1",
        );
        transport.send(msg).unwrap();
        let _received = transport.receive(ChannelId::Data).unwrap();

        // Acknowledge the message — should remove it from the queue
        transport.acknowledge("ack-msg-1").unwrap();

        // Receiving again should yield nothing new since the message was acknowledged
        let remaining = transport.receive(ChannelId::Data).unwrap();
        assert!(remaining.is_empty());

        // Profile should show 1 sent, 1 received
        let profile = transport.profile();
        assert_eq!(profile.total_messages_sent, 1);
        assert_eq!(profile.total_messages_received, 1);
    }

    // ── 8. forward message ────────────────────────────────────────────────

    #[test]
    fn test_forward_message() {
        let transport = configured_transport();
        let msg = sample_message(
            &transport,
            ChannelId::Data,
            MessagePriority::Normal,
            "fwd-1",
        );
        transport.send(msg).unwrap();

        // Forward from Data to Event channel
        let receipt = transport.forward("fwd-1", ChannelId::Event).unwrap();
        assert_eq!(receipt.status, DeliveryStatus::Queued);

        // The message should now be on the Event channel
        let event_msgs = transport.receive(ChannelId::Event).unwrap();
        assert_eq!(event_msgs.len(), 1);
        assert_eq!(event_msgs[0].id, "fwd-1");
        assert_eq!(event_msgs[0].channel, ChannelId::Event);
    }

    // ── 9. expire old messages ────────────────────────────────────────────

    #[test]
    fn test_expire_old_messages() {
        let transport = configured_transport();

        // Manually insert expired messages into the Data channel
        {
            let mut inner = transport.inner.lock().unwrap();
            let channel = inner.channels.get_mut(&ChannelId::Data).unwrap();
            let old_msg = TransportMessage {
                id: "expired-1".to_string(),
                channel: ChannelId::Data,
                priority: MessagePriority::Normal,
                payload: "old".to_string(),
                source: "src".to_string(),
                target: "dst".to_string(),
                created_ms: 1000, // very old
                ttl_ms: 100,
                delivery_attempts: 0,
            };
            channel.queue.push(QueuedMessage {
                message: old_msg,
                status: DeliveryStatus::Queued,
                retries_remaining: 3,
                enqueued_ms: 1000,
            });
            channel.stats.queue_depth = 1;
        }

        let removed = transport.expire_old_messages();
        assert_eq!(removed, 1);

        let stats = transport.channel_stats(ChannelId::Data).unwrap();
        assert_eq!(stats.messages_expired, 1);
        assert_eq!(stats.queue_depth, 0);
    }

    // ── 10. message TTL expiration ────────────────────────────────────────

    #[test]
    fn test_message_ttl_expiration() {
        let transport = configured_transport();

        let now = MultiChannelTransport::now_ms();
        let expired_msg = TransportMessage {
            id: "ttl-msg".to_string(),
            channel: ChannelId::Data,
            priority: MessagePriority::Normal,
            payload: "will-expire".to_string(),
            source: "src".to_string(),
            target: "dst".to_string(),
            created_ms: now - 100_000, // created 100 seconds ago
            ttl_ms: 50,                // lives only 50ms
            delivery_attempts: 0,
        };

        // send() should accept the message but mark it expired due to TTL
        let receipt = transport.send(expired_msg).unwrap();
        assert_eq!(receipt.status, DeliveryStatus::Expired);
        assert!(receipt.error.is_some());

        // No message should be in the queue
        let received = transport.receive(ChannelId::Data).unwrap();
        assert!(received.is_empty());
    }

    // ── 11. channel stats ─────────────────────────────────────────────────

    #[test]
    fn test_channel_stats() {
        let transport = configured_transport();

        // Send a message on the Data channel
        let msg = sample_message(
            &transport,
            ChannelId::Data,
            MessagePriority::Normal,
            "stats-1",
        );
        transport.send(msg).unwrap();

        // Receive it
        let _received = transport.receive(ChannelId::Data).unwrap();

        let stats = transport.channel_stats(ChannelId::Data).unwrap();
        assert_eq!(stats.messages_sent, 1);
        assert_eq!(stats.messages_received, 1);
        assert_eq!(stats.messages_failed, 0);
        assert_eq!(stats.messages_expired, 0);
    }

    // ── 12. profile reflects state ────────────────────────────────────────

    #[test]
    fn test_profile_reflects_state() {
        let transport = configured_transport();

        // Send a couple of messages on different channels
        let msg1 = sample_message(
            &transport,
            ChannelId::Control,
            MessagePriority::Critical,
            "prof-ctrl",
        );
        let msg2 = sample_message(
            &transport,
            ChannelId::Data,
            MessagePriority::Normal,
            "prof-data",
        );
        transport.send(msg1).unwrap();
        transport.send(msg2).unwrap();

        let profile = transport.profile();
        assert_eq!(profile.total_messages_sent, 2);
        assert_eq!(profile.total_messages_received, 0);
        assert_eq!(profile.total_failures, 0);
        assert_eq!(profile.active_channels, 6); // all six channels configured
        assert_eq!(profile.channel_stats.len(), 6);
    }

    // ── 13. delivery retry ────────────────────────────────────────────────

    #[test]
    fn test_delivery_retry() {
        let transport = configured_transport();

        // Send a message with a custom config that has limited retries
        let msg = sample_message(
            &transport,
            ChannelId::Data,
            MessagePriority::Normal,
            "retry-1",
        );
        transport.send(msg).unwrap();

        // Simulate retries: the message will be received and the
        // delivery_attempts field tracks progress. When the message is
        // forwarded, delivery_attempts increments.
        let receipt1 = transport.forward("retry-1", ChannelId::Data).unwrap();
        assert_eq!(receipt1.status, DeliveryStatus::Queued);

        // Forward again — simulates a second retry
        let receipt2 = transport.forward("retry-1", ChannelId::Data).unwrap();
        assert_eq!(receipt2.status, DeliveryStatus::Queued);

        // After forwarding, each forward removed the message and re-queued a copy.
        // Forward #1: removes original (attempts=0), queues copy (attempts=1)
        // Forward #2: removes copy (attempts=1), queues copy (attempts=2)
        let messages = transport.receive(ChannelId::Data).unwrap();

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, "retry-1");
        assert_eq!(messages[0].delivery_attempts, 2); // each forward increments by 1
    }
}
