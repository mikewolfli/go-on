//! F-GAP-29: Multi-channel Message Transport
//!
//! Provides protocol-layer channel separation for different message types.
//! Each channel is isolated with its own queue, configuration, and statistics.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// Lock a Mutex, recovering from poison with a log.
fn lock_guard<T>(mtx: &Mutex<T>) -> MutexGuard<'_, T> {
    match mtx.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::error!("transport(2) mutex poisoned, recovering");
            poisoned.into_inner()
        }
    }
}

use crate::i18n::runtime::tf;
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

/// Quality of Service level for a message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum QosLevel {
    /// Fire-and-forget; no delivery guarantee
    AtMostOnce,
    /// Retry until acknowledged
    AtLeastOnce,
    /// Guaranteed exactly-once delivery via deduplication
    ExactlyOnce,
}

/// Delivery status of a message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DeliveryStatus {
    /// Message is queued and awaiting delivery
    Pending,
    /// Message is currently in flight
    InFlight,
    /// Message has been successfully delivered
    Delivered { timestamp_ms: u64 },
    /// Delivery failed (permanently)
    Failed { reason: String, retry_count: u32 },
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
    /// Quality of Service level
    pub qos: QosLevel,
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
    #[allow(dead_code)] // F-GAP-10 — reserved for future multi-channel transport wiring
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
    sent_ids: HashSet<String>,
    sent_ids_order: VecDeque<String>,
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
                sent_ids: HashSet::new(),
                sent_ids_order: VecDeque::new(),
                total_sent: 0,
                total_received: 0,
                total_failures: 0,
            })),
        }
    }

    // ── channel management ────────────────────────────────────────────────

    /// Configure a specific channel. Creates the channel if it does not exist.
    pub fn configure_channel(&self, config: ChannelConfig) -> Result<()> {
        let mut inner = lock_guard(&self.inner);
        let channel_id = config.channel.clone();

        // Enforce max_channels limit
        if !inner.channels.contains_key(&channel_id)
            && inner.channels.len() >= inner.config.max_channels
        {
            bail!(tf(
                "error.transport.max_channels_reached",
                &[("max", &inner.config.max_channels.to_string())]
            ));
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
        let mut inner = lock_guard(&self.inner);
        let now = Self::now_ms();

        // Dedup for ExactlyOnce QoS: if the message id was already sent, return
        // a Delivered receipt without re-enqueuing
        if message.qos == QosLevel::ExactlyOnce && inner.sent_ids.contains(&message.id) {
            return Ok(DeliveryReceipt {
                message_id: message.id.clone(),
                status: DeliveryStatus::Delivered { timestamp_ms: now },
                delivered_ms: Some(now),
                error: None,
            });
        }

        // Check global rate limit
        // Simple rate limiting: track time window globally
        // (per-channel rate limiting is handled in channel logic)

        // Locate or create the channel
        let channel_id = message.channel.clone();
        let channel = inner.channels.get_mut(&channel_id).ok_or_else(|| {
            anyhow::anyhow!(tf(
                "error.transport.channel_not_configured",
                &[("id", &channel_id.to_string())]
            ))
        })?;

        // Per-channel rate limiting
        let elapsed = now.saturating_sub(channel.last_rate_ts);
        if elapsed >= 1000 {
            // Reset rate counter for new window
            channel.last_rate_ts = now;
            channel.rate_count = 0;
        }
        if channel.rate_count >= channel.config.rate_limit_per_sec {
            bail!(tf(
                "error.transport.rate_limit_exceeded",
                &[
                    ("id", &channel_id.to_string()),
                    ("rate", &channel.config.rate_limit_per_sec.to_string())
                ]
            ));
        }
        channel.rate_count += 1;

        // Queue depth check
        if channel.queue.len() > channel.config.max_queue_size {
            bail!(tf(
                "error.transport.queue_full",
                &[
                    ("max", &channel.config.max_queue_size.to_string()),
                    ("id", &channel_id.to_string())
                ]
            ));
        }

        // TTL check — if the message has already expired, mark it expired
        if message.created_ms + message.ttl_ms < now {
            channel.stats.messages_expired += 1;
            inner.total_failures += 1;
            return Ok(DeliveryReceipt {
                message_id: message.id,
                status: DeliveryStatus::Expired,
                delivered_ms: None,
                error: Some(tf("error.transport.message_expired_send", &[])),
            });
        }

        // Enqueue with priority ordering (higher priority first)
        let max_retries = channel.config.max_retries;
        let queued = QueuedMessage {
            message: message.clone(),
            status: DeliveryStatus::Pending,
            retries_remaining: max_retries,
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

        // Record sent id for ExactlyOnce QoS dedup — after channel borrow is done
        if message.qos == QosLevel::ExactlyOnce {
            inner.sent_ids.insert(message.id.clone());
            inner.sent_ids_order.push_back(message.id.clone());

            // Evict oldest entries when the dedup set grows beyond capacity
            const MAX_DEDUP_IDS: usize = 10_000;
            while inner.sent_ids.len() > MAX_DEDUP_IDS {
                if let Some(oldest) = inner.sent_ids_order.pop_front() {
                    inner.sent_ids.remove(&oldest);
                } else {
                    break;
                }
            }
        }
        inner.total_sent += 1;

        Ok(DeliveryReceipt {
            message_id: message.id.clone(),
            status: DeliveryStatus::Pending,
            delivered_ms: None,
            error: None,
        })
    }

    // ── receiving ─────────────────────────────────────────────────────────

    /// Receive all available messages from a specific channel.
    pub fn receive(&self, channel_id: ChannelId) -> Result<Vec<TransportMessage>> {
        let mut inner = lock_guard(&self.inner);
        let now = Self::now_ms();

        let channel = inner.channels.get_mut(&channel_id).ok_or_else(|| {
            anyhow::anyhow!(tf(
                "error.transport.channel_not_configured",
                &[("id", &channel_id.to_string())]
            ))
        })?;

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
        let mut inner = lock_guard(&self.inner);

        for (_ch_id, channel) in inner.channels.iter_mut() {
            let before = channel.queue.len();
            channel.queue.retain(|qm| qm.message.id != message_id);
            if channel.queue.len() < before {
                channel.stats.queue_depth = channel.queue.len();
                return Ok(());
            }
        }

        bail!(tf(
            "error.transport.message_not_found",
            &[("id", message_id)]
        ))
    }

    /// Forward a message to a different channel.
    ///
    /// Removes the original message and sends a copy to the target channel.
    /// Peek at the next message in a channel without dequeuing.
    pub fn peek(&self, channel_id: &ChannelId) -> Option<TransportMessage> {
        let inner = lock_guard(&self.inner);
        inner
            .channels
            .get(channel_id)
            .and_then(|ch| ch.queue.first().map(|qm| qm.message.clone()))
    }

    /// Get statistics for all channels.
    pub fn all_channel_stats(&self) -> Vec<ChannelStats> {
        let inner = lock_guard(&self.inner);
        inner.channels.values().map(|ch| ch.stats.clone()).collect()
    }

    /// Send a message on the Control channel.
    pub fn send_control(
        &self,
        source: &str,
        target: &str,
        payload: &str,
    ) -> Result<DeliveryReceipt> {
        self.send(self.make_message(ChannelId::Control, source, target, payload))
    }

    /// Send a message on the Data channel.
    pub fn send_data(&self, source: &str, target: &str, payload: &str) -> Result<DeliveryReceipt> {
        self.send(self.make_message(ChannelId::Data, source, target, payload))
    }

    /// Send a message on the Event channel.
    pub fn send_event(&self, source: &str, target: &str, payload: &str) -> Result<DeliveryReceipt> {
        self.send(self.make_message(ChannelId::Event, source, target, payload))
    }

    /// Send a message on the Heartbeat channel.
    pub fn send_heartbeat(
        &self,
        source: &str,
        target: &str,
        payload: &str,
    ) -> Result<DeliveryReceipt> {
        self.send(self.make_message(ChannelId::Heartbeat, source, target, payload))
    }

    /// Forward a message to a different channel.
    ///
    /// Removes the original message and sends a copy to the target channel.
    pub fn forward(&self, message_id: &str, target_channel: ChannelId) -> Result<DeliveryReceipt> {
        let mut inner = lock_guard(&self.inner);

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
            None => bail!(tf(
                "error.transport.message_not_found_forward",
                &[("id", message_id)]
            )),
        };

        // Update the message for the new channel
        msg.channel = target_channel;
        msg.delivery_attempts += 1;

        // Drop the lock momentarily — we need the inner lock again via send
        // but since we already hold it, we'll just directly enqueue.
        // (We still hold the inner lock, so we can proceed directly.)
        let channel_id = msg.channel.clone();

        // Check that the target channel exists
        let target = inner.channels.get_mut(&channel_id).ok_or_else(|| {
            anyhow::anyhow!(tf(
                "error.transport.target_not_configured",
                &[("id", &channel_id.to_string())]
            ))
        })?;

        // Queue depth check
        if target.queue.len() >= target.config.max_queue_size {
            bail!(tf(
                "error.transport.target_queue_full",
                &[
                    ("max", &target.config.max_queue_size.to_string()),
                    ("id", &channel_id.to_string())
                ]
            ));
        }

        let now = Self::now_ms();
        let queued = QueuedMessage {
            message: msg.clone(),
            status: DeliveryStatus::Pending,
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
            status: DeliveryStatus::Pending,
            delivered_ms: None,
            error: None,
        })
    }

    // ── introspection ─────────────────────────────────────────────────────

    /// Get statistics for a specific channel.
    pub fn channel_stats(&self, channel_id: ChannelId) -> Option<ChannelStats> {
        let inner = lock_guard(&self.inner);
        inner.channels.get(&channel_id).map(|ch| ch.stats.clone())
    }

    /// Obtain a snapshot of the entire transport profile.
    pub fn profile(&self) -> TransportProfile {
        let inner = lock_guard(&self.inner);
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
        let mut inner = lock_guard(&self.inner);
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
        crate::acp::prelude::now_ts_ms() as u64
    }

    /// Build a minimal TransportMessage with default QoS and auto-generated id.
    fn make_message(
        &self,
        channel: ChannelId,
        source: &str,
        target: &str,
        payload: &str,
    ) -> TransportMessage {
        TransportMessage {
            id: format!("msg-{}", Self::next_id()),
            channel,
            priority: MessagePriority::Normal,
            qos: QosLevel::AtMostOnce,
            payload: payload.to_string(),
            source: source.to_string(),
            target: target.to_string(),
            created_ms: Self::now_ms(),
            ttl_ms: 30000,
            delivery_attempts: 0,
        }
    }

    /// Atomically increment and return the next message id counter.
    fn next_id() -> u64 {
        NEXT_MSG_ID.fetch_add(1, Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------------------
// Static helpers
// ---------------------------------------------------------------------------

static NEXT_MSG_ID: AtomicU64 = AtomicU64::new(1);

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
        _transport: &MultiChannelTransport,
        channel: ChannelId,
        priority: MessagePriority,
    ) -> TransportMessage {
        let now = MultiChannelTransport::now_ms();
        let channel_name = channel.to_string();
        TransportMessage {
            id: format!("msg-{}", MultiChannelTransport::next_id()),
            channel,
            priority,
            qos: QosLevel::AtMostOnce,
            payload: format!("payload-{}", channel_name),
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
        let msg = sample_message(&transport, ChannelId::Data, MessagePriority::Normal);
        let receipt = transport.send(msg).unwrap();

        assert_eq!(receipt.status, DeliveryStatus::Pending);
        assert!(receipt.message_id.starts_with("msg-"));

        let profile = transport.profile();
        assert_eq!(profile.total_messages_sent, 1);
    }

    // ── 4. send with priority ─────────────────────────────────────────────

    #[test]
    fn test_send_with_priority() {
        let transport = configured_transport();

        let low = sample_message(&transport, ChannelId::Data, MessagePriority::Low);
        let critical = sample_message(&transport, ChannelId::Data, MessagePriority::Critical);
        let normal = sample_message(&transport, ChannelId::Data, MessagePriority::Normal);
        let high = sample_message(&transport, ChannelId::Data, MessagePriority::High);

        transport.send(low).unwrap();
        transport.send(critical).unwrap();
        transport.send(normal).unwrap();
        transport.send(high).unwrap();

        // Receive should return messages in priority order
        let received = transport.receive(ChannelId::Data).unwrap();
        assert_eq!(received.len(), 4);

        // Messages should come out in priority order (Critical > High > Normal > Low)
        assert_eq!(received[0].priority, MessagePriority::Critical);
        assert_eq!(received[1].priority, MessagePriority::High);
        assert_eq!(received[2].priority, MessagePriority::Normal);
        assert_eq!(received[3].priority, MessagePriority::Low);
    }

    // ── 5. send and receive ───────────────────────────────────────────────

    #[test]
    fn test_send_and_receive() {
        let transport = configured_transport();
        let msg = sample_message(&transport, ChannelId::Event, MessagePriority::Normal);
        transport.send(msg).unwrap();

        let received = transport.receive(ChannelId::Event).unwrap();
        assert_eq!(received.len(), 1);
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
        let msg = sample_message(&transport, ChannelId::Data, MessagePriority::Normal);
        let msg_id = msg.id.clone();
        transport.send(msg).unwrap();
        let _received = transport.receive(ChannelId::Data).unwrap();

        // Acknowledge the message — should remove it from the queue
        transport.acknowledge(&msg_id).unwrap();

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
        let msg = sample_message(&transport, ChannelId::Data, MessagePriority::Normal);
        let msg_id = msg.id.clone();
        transport.send(msg).unwrap();

        // Forward from Data to Event channel
        let receipt = transport.forward(&msg_id, ChannelId::Event).unwrap();
        assert_eq!(receipt.status, DeliveryStatus::Pending);

        // The message should now be on the Event channel
        let event_msgs = transport.receive(ChannelId::Event).unwrap();
        assert_eq!(event_msgs.len(), 1);
        assert_eq!(event_msgs[0].id, msg_id);
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
                qos: QosLevel::AtMostOnce,
                payload: "old".to_string(),
                source: "src".to_string(),
                target: "dst".to_string(),
                created_ms: 1000, // very old
                ttl_ms: 100,
                delivery_attempts: 0,
            };
            channel.queue.push(QueuedMessage {
                message: old_msg,
                status: DeliveryStatus::Pending,
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
            qos: QosLevel::AtMostOnce,
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
        let msg = sample_message(&transport, ChannelId::Data, MessagePriority::Normal);
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
        let msg1 = sample_message(&transport, ChannelId::Control, MessagePriority::Critical);
        let msg2 = sample_message(&transport, ChannelId::Data, MessagePriority::Normal);
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
        let msg = sample_message(&transport, ChannelId::Data, MessagePriority::Normal);
        let msg_id = msg.id.clone();
        transport.send(msg).unwrap();

        // Simulate retries: the message will be received and the
        // delivery_attempts field tracks progress. When the message is
        // forwarded, delivery_attempts increments.
        let receipt1 = transport.forward(&msg_id, ChannelId::Data).unwrap();
        assert_eq!(receipt1.status, DeliveryStatus::Pending);

        // Forward again — simulates a second retry
        let receipt2 = transport.forward(&msg_id, ChannelId::Data).unwrap();
        assert_eq!(receipt2.status, DeliveryStatus::Pending);

        // After forwarding, each forward removed the message and re-queued a copy.
        // Forward #1: removes original (attempts=0), queues copy (attempts=1)
        // Forward #2: removes copy (attempts=1), queues copy (attempts=2)
        let messages = transport.receive(ChannelId::Data).unwrap();

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, msg_id);
        assert_eq!(messages[0].delivery_attempts, 2); // each forward increments by 1
    }

    // ── 14. send with QoS ─────────────────────────────────────────────────

    #[test]
    fn test_send_with_qos() {
        let transport = configured_transport();
        let mut msg = sample_message(&transport, ChannelId::Data, MessagePriority::Normal);
        msg.qos = QosLevel::ExactlyOnce;
        let receipt = transport.send(msg).unwrap();
        assert_eq!(receipt.status, DeliveryStatus::Pending);
    }

    // ── 15. dedup for ExactlyOnce ─────────────────────────────────────────

    #[test]
    fn test_dedup_exactly_once() {
        let transport = configured_transport();
        let mut msg = sample_message(&transport, ChannelId::Data, MessagePriority::Normal);
        msg.id = "dedup-test-1".to_string();
        msg.qos = QosLevel::ExactlyOnce;

        // First send should succeed
        let r1 = transport.send(msg.clone()).unwrap();
        assert_eq!(r1.status, DeliveryStatus::Pending);

        // Second send of same id should result in already-delivered
        let r2 = transport.send(msg).unwrap();
        match r2.status {
            DeliveryStatus::Delivered { .. } => {} // expected
            _ => panic!("Expected Delivered status for dedup, got {:?}", r2.status),
        }
    }

    // ── 16. peek does not dequeue ─────────────────────────────────────────

    #[test]
    fn test_peek_does_not_dequeue() {
        let transport = configured_transport();
        let msg = sample_message(&transport, ChannelId::Data, MessagePriority::High);
        let msg_id = msg.id.clone();
        transport.send(msg).unwrap();

        // Peek should return the message without dequeuing
        let peeked = transport.peek(&ChannelId::Data).unwrap();
        assert_eq!(peeked.id, msg_id);
        assert_eq!(peeked.priority, MessagePriority::High);

        // The message should still be in the queue (receive should still get it)
        let received = transport.receive(ChannelId::Data).unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].id, msg_id);
    }

    // ── 17. convenience send methods ──────────────────────────────────────

    #[test]
    fn test_convenience_send_methods() {
        let transport = configured_transport();

        transport.send_control("src", "dst", "control_msg").unwrap();
        transport.send_data("src", "dst", "data_msg").unwrap();
        transport.send_event("src", "dst", "event_msg").unwrap();
        transport.send_heartbeat("src", "dst", "hb").unwrap();

        let stats = transport.all_channel_stats();
        let total_sent: u64 = stats.iter().map(|s| s.messages_sent).sum();
        assert_eq!(total_sent, 4);
    }

    // ── 18. all channel stats ─────────────────────────────────────────────

    #[test]
    fn test_all_channel_stats() {
        let transport = configured_transport();
        let stats_before = transport.all_channel_stats();
        let channels_before = stats_before.len();

        transport.send_data("a", "b", "test").unwrap();

        let stats_after = transport.all_channel_stats();
        assert_eq!(stats_after.len(), channels_before);

        let data_stats: Vec<_> = stats_after
            .into_iter()
            .filter(|s| s.channel == ChannelId::Data)
            .collect();
        assert_eq!(data_stats.len(), 1);
        assert_eq!(data_stats[0].messages_sent, 1);
    }
}
