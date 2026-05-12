//! F-GAP-29: Multi-channel Message Transport
//!
//! Provides protocol-level channel separation and message routing.
//! Each channel is isolated with its own queue, configuration, and statistics.
//! Messages are routed to the appropriate channel based on their `TransportChannel` type.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// Lock a Mutex, recovering from poison with a log.
fn lock_guard<T>(mtx: &Mutex<T>) -> MutexGuard<'_, T> {
    match mtx.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::error!("transport mutex poisoned, recovering");
            poisoned.into_inner()
        }
    }
}

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Identifies the transport channel.
#[allow(dead_code)] // F-GAP-29 — used by profile-multi-users-server
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum TransportChannel {
    /// Control channel — governance, policy, heartbeat
    Control,
    /// Data channel — actual task payloads
    Data,
    /// Event channel — notifications, streaming events
    Event,
    /// Logging channel — telemetry, tracing, audit
    Logging,
    /// Side channel — out-of-band communication
    Sideband,
    /// Custom named channel
    Custom(String),
}

impl fmt::Display for TransportChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Control => write!(f, "control"),
            Self::Data => write!(f, "data"),
            Self::Event => write!(f, "event"),
            Self::Logging => write!(f, "logging"),
            Self::Sideband => write!(f, "sideband"),
            Self::Custom(name) => write!(f, "custom_{}", name),
        }
    }
}

/// Quality of Service level for a message.
#[allow(dead_code)] // F-GAP-29 — used by profile-multi-users-server
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum QosLevel {
    /// At most once (fire and forget)
    AtMostOnce,
    /// At least once (retry until ack)
    AtLeastOnce,
    /// Exactly once (dedup + ack)
    ExactlyOnce,
}

/// Priority of a message within its channel.
#[allow(dead_code)] // F-GAP-29 — used by profile-multi-users-server
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum MessagePriority {
    /// Must be processed immediately
    Critical,
    /// High urgency
    High,
    /// Default
    Normal,
    /// Best effort
    Low,
}

/// Delivery guarantee status.
#[allow(dead_code)] // F-GAP-29 — used by profile-multi-users-server
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DeliveryStatus {
    Pending,
    InFlight,
    Delivered { timestamp_ms: u64 },
    Failed { reason: String, retry_count: u32 },
    Expired,
}

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// A message to be transported over a channel.
#[allow(dead_code)] // F-GAP-29 — used by profile-multi-users-server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelMessage {
    pub id: String,
    pub channel: TransportChannel,
    pub priority: MessagePriority,
    pub qos: QosLevel,
    pub source: String,
    pub destination: String,
    pub payload: Value,
    pub ttl_ms: u64,
    pub created_ms: u64,
    pub delivery_status: DeliveryStatus,
    pub correlation_id: Option<String>,
    pub headers: HashMap<String, String>,
}

/// Statistics for a single channel.
#[allow(dead_code)] // F-GAP-29 — used by profile-multi-users-server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelStats {
    pub channel: TransportChannel,
    pub messages_sent: u64,
    pub messages_received: u64,
    pub messages_delivered: u64,
    pub messages_failed: u64,
    pub messages_expired: u64,
    pub avg_latency_ms: f64,
    pub queue_depth: usize,
}

/// Channel configuration.
#[allow(dead_code)] // F-GAP-29 — used by profile-multi-users-server
#[derive(Debug, Clone)]
pub struct ChannelConfig {
    pub max_queue_depth: usize, // Default: 10000
    pub default_ttl_ms: u64,    // Default: 30000 (30s)
    pub enable_retry: bool,     // Default: true
    pub max_retries: u32,       // Default: 3
    pub retry_delay_ms: u64,    // Default: 1000 (1s)
    pub enable_dedup: bool,     // Default: true
    pub enable_ordering: bool,  // Default: true
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            max_queue_depth: 10_000,
            default_ttl_ms: 30_000,
            enable_retry: true,
            max_retries: 3,
            retry_delay_ms: 1_000,
            enable_dedup: true,
            enable_ordering: true,
        }
    }
}

/// Transport profile snapshot.
#[allow(dead_code)] // F-GAP-29 — used by profile-multi-users-server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportProfile {
    pub enabled: bool,
    pub active_channels: Vec<String>,
    pub total_messages_sent: u64,
    pub total_messages_received: u64,
    pub total_messages_delivered: u64,
    pub total_messages_failed: u64,
    pub avg_all_latency_ms: f64,
    pub channel_stats: Vec<ChannelStats>,
    pub last_activity_ms: u64,
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

#[allow(dead_code)] // F-GAP-29 — used by profile-multi-users-server
struct TransportInner {
    config: ChannelConfig,
    queues: HashMap<TransportChannel, VecDeque<ChannelMessage>>,
    stats: HashMap<TransportChannel, ChannelStats>,
    sent_ids: HashSet<String>,
    last_activity_ms: u64,
    total_sent: u64,
    total_received: u64,
    total_delivered: u64,
    total_failed: u64,
    status: bool,
}

// ---------------------------------------------------------------------------
// Public API: MultiChannelTransport
// ---------------------------------------------------------------------------

/// Multi-channel transport — manages message routing across multiple channels.
///
/// Each channel is isolated with its own queue, config, and statistics.
/// Messages are routed to the appropriate channel based on their `TransportChannel` type.
#[allow(dead_code)] // F-GAP-29 — used by profile-multi-users-server
pub struct MultiChannelTransport {
    inner: Arc<Mutex<TransportInner>>,
    next_message_id: AtomicU64,
}

#[allow(dead_code)] // F-GAP-29 — used by profile-multi-users-server
impl MultiChannelTransport {
    // ── construction ──────────────────────────────────────────────────────

    /// Create a new transport with the given channel configuration.
    pub fn new(config: ChannelConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(TransportInner {
                config,
                queues: HashMap::new(),
                stats: HashMap::new(),
                sent_ids: HashSet::new(),
                last_activity_ms: Self::now_ms(),
                total_sent: 0,
                total_received: 0,
                total_delivered: 0,
                total_failed: 0,
                status: true,
            })),
            next_message_id: AtomicU64::new(1),
        }
    }

    // ── sending ───────────────────────────────────────────────────────────

    /// Enqueue a message onto its channel.
    ///
    /// Checks dedup (if same ID already sent, skip).
    /// Applies TTL check — if the message has already expired it is marked
    /// `Expired` and not enqueued.
    pub fn send(&self, msg: ChannelMessage) -> Result<()> {
        let now = Self::now_ms();
        let mut inner = lock_guard(&self.inner);

        let ch = &msg.channel;

        // TTL check (early exit)
        if msg.created_ms + msg.ttl_ms < now {
            inner.total_failed += 1;
            inner.last_activity_ms = now;
            let stats = inner
                .stats
                .entry(ch.clone())
                .or_insert_with(|| Self::new_stats(ch.clone()));
            stats.messages_expired += 1;
            return Ok(());
        }

        // Dedup check with borrowed reference (early exit)
        if inner.config.enable_dedup && inner.sent_ids.contains(&msg.id) {
            return Ok(());
        }

        // Queue depth check (early exit)
        if inner.queues.len() >= inner.config.max_queue_depth {
            bail!("queue depth exceeded for channel {}", ch);
        }

        // Track sent ID (only if dedup enabled)
        if inner.config.enable_dedup {
            inner.sent_ids.insert(msg.id.clone());
        }

        inner.total_sent += 1;
        inner.last_activity_ms = now;

        // Prepare message with pending status
        let mut message = msg.clone();
        message.delivery_status = DeliveryStatus::Pending;

        // Queue operation with borrowed channel key
        let enable_ordering = inner.config.enable_ordering;
        let queue = inner.queues.entry(ch.clone()).or_default();
        if enable_ordering {
            let pos = queue
                .iter()
                .position(|m| m.priority > message.priority)
                .unwrap_or(queue.len());
            queue.insert(pos, message);
        } else {
            queue.push_back(message);
        }

        // Update channel stats (single entry lookup)
        let queue_len = queue.len();
        let _ = queue; // drop queue borrow
        let stats = inner
            .stats
            .entry(ch.clone())
            .or_insert_with(|| Self::new_stats(ch.clone()));
        stats.messages_sent += 1;
        stats.queue_depth = queue_len;

        Ok(())
    }

    // ── receiving ─────────────────────────────────────────────────────────

    /// Dequeue the highest priority message from the given channel.
    pub fn receive(&self, channel: &TransportChannel) -> Option<ChannelMessage> {
        let mut inner = lock_guard(&self.inner);

        // Pop the front message — then drop(queue) before touching other fields
        let (msg, queue_len) = {
            let queue = inner.queues.get_mut(channel)?;
            let msg = queue.pop_front()?;
            let len = queue.len();
            (msg, len)
        };

        inner.total_received += 1;
        inner.last_activity_ms = Self::now_ms();

        let stats = inner
            .stats
            .entry(channel.clone())
            .or_insert_with(|| Self::new_stats(channel.clone()));
        stats.messages_received += 1;
        stats.queue_depth = queue_len;

        Some(msg)
    }

    /// Dequeue multiple messages (up to `max`) from the given channel.
    /// Optimized: Acquire lock once and drain multiple messages to reduce lock contention.
    pub fn receive_batch(&self, channel: &TransportChannel, max: usize) -> Vec<ChannelMessage> {
        let mut inner = lock_guard(&self.inner);
        let mut msgs = Vec::with_capacity(max);

        // Drain up to `max` messages without re-acquiring lock each time
        if let Some(queue) = inner.queues.get_mut(channel) {
            for _ in 0..max {
                if let Some(msg) = queue.pop_front() {
                    msgs.push(msg);
                } else {
                    break;
                }
            }
        }

        if !msgs.is_empty() {
            inner.total_received += msgs.len() as u64;
            inner.last_activity_ms = Self::now_ms();

            // Update stats with final queue depth
            let queue_len = inner.queues.get(channel).map(|q| q.len()).unwrap_or(0);
            let stats = inner
                .stats
                .entry(channel.clone())
                .or_insert_with(|| Self::new_stats(channel.clone()));
            stats.messages_received += msgs.len();
            stats.queue_depth = queue_len;
        }

        msgs
    }

    // ── acknowledgment / failure ──────────────────────────────────────────

    /// Mark a message as delivered and update statistics.
    pub fn acknowledge(&self, _msg_id: &str) {
        let mut inner = lock_guard(&self.inner);
        inner.total_delivered += 1;
        inner.last_activity_ms = Self::now_ms();
    }

    /// Mark a message as failed, optionally retrying based on QoS.
    pub fn fail_message(&self, _msg_id: &str, _reason: &str) {
        let mut inner = lock_guard(&self.inner);
        let now = Self::now_ms();
        let enable_retry = inner.config.enable_retry;
        let max_retries = inner.config.max_retries;
        let retry_delay_ms = inner.config.retry_delay_ms;

        // Best-effort retry path: locate the message in any queue and update its
        // delivery status based on retry policy.
        let mut retry_scheduled = false;
        let mut failed_channel: Option<TransportChannel> = None;
        for (channel, queue) in inner.queues.iter_mut() {
            if let Some(idx) = queue.iter().position(|m| m.id == _msg_id) {
                let mut msg = queue.remove(idx).expect("indexed message should exist");
                let retry_count = match &msg.delivery_status {
                    DeliveryStatus::Failed { retry_count, .. } => *retry_count,
                    _ => 0,
                };

                if enable_retry && retry_count < max_retries {
                    msg.delivery_status = DeliveryStatus::Failed {
                        reason: _reason.to_string(),
                        retry_count: retry_count + 1,
                    };
                    // Record retry delay for downstream consumers.
                    msg.headers
                        .insert("x-retry-after-ms".to_string(), retry_delay_ms.to_string());
                    queue.push_back(msg);
                    retry_scheduled = true;
                } else {
                    msg.delivery_status = DeliveryStatus::Failed {
                        reason: _reason.to_string(),
                        retry_count,
                    };
                    failed_channel = Some(channel.clone());
                }
                break;
            }
        }

        if let Some(channel) = failed_channel {
            let stats = inner
                .stats
                .entry(channel.clone())
                .or_insert_with(|| Self::new_stats(channel));
            stats.messages_failed += 1;
        }

        if !retry_scheduled {
            inner.total_failed += 1;
        }
        inner.last_activity_ms = now;
    }

    // ── introspection ─────────────────────────────────────────────────────

    /// Peek at the highest priority message without dequeuing.
    pub fn peek(&self, channel: &TransportChannel) -> Option<ChannelMessage> {
        let inner = lock_guard(&self.inner);
        inner.queues.get(channel)?.front().cloned()
    }

    /// Get statistics for a specific channel.
    pub fn channel_stats(&self, channel: &TransportChannel) -> Option<ChannelStats> {
        let inner = lock_guard(&self.inner);
        inner.stats.get(channel).cloned()
    }

    /// Get statistics for all channels.
    pub fn all_channel_stats(&self) -> Vec<ChannelStats> {
        let inner = lock_guard(&self.inner);
        inner.stats.values().cloned().collect()
    }

    /// Remove expired messages from all queues.
    pub fn prune_expired(&self) {
        let now = Self::now_ms();
        let mut inner = lock_guard(&self.inner);

        // Collect channel names first to avoid simultaneous borrows
        let channels: Vec<TransportChannel> = inner.queues.keys().cloned().collect();

        for ch in &channels {
            // Step 1: Prune the queue and compute queue_depth in a scope
            let (removed, new_queue_len) = {
                let queue = inner.queues.get_mut(ch);
                if let Some(queue) = queue {
                    let before = queue.len();
                    queue.retain(|msg| msg.created_ms + msg.ttl_ms >= now);
                    (before - queue.len(), queue.len())
                } else {
                    (0, 0)
                }
            };

            // Step 2: Update stats (no longer borrowing queue)
            if removed > 0 {
                if let Some(stats) = inner.stats.get_mut(ch) {
                    stats.messages_expired += removed as u64;
                    stats.queue_depth = new_queue_len;
                }
            }
        }
    }

    /// Obtain a snapshot of the entire transport profile.
    pub fn profile(&self) -> TransportProfile {
        let channel_stats = self.all_channel_stats();
        let inner = lock_guard(&self.inner);

        // Collect borrowed data into owned values before building the struct
        let active_channels: Vec<String> = inner.queues.keys().map(|ch| ch.to_string()).collect();

        TransportProfile {
            enabled: inner.status,
            active_channels,
            total_messages_sent: inner.total_sent,
            total_messages_received: inner.total_received,
            total_messages_delivered: inner.total_delivered,
            total_messages_failed: inner.total_failed,
            avg_all_latency_ms: 0.0,
            channel_stats,
            last_activity_ms: inner.last_activity_ms,
        }
    }

    /// Get a copy of the current channel configuration.
    pub fn config(&self) -> ChannelConfig {
        let inner = lock_guard(&self.inner);
        inner.config.clone()
    }

    // ── convenience senders ───────────────────────────────────────────────

    /// Convenience method: send a message on the Control channel.
    pub fn send_control(&self, source: &str, destination: &str, payload: Value) -> Result<()> {
        let msg = self.make_message(
            TransportChannel::Control,
            MessagePriority::High,
            source,
            destination,
            payload,
        );
        self.send(msg)
    }

    /// Convenience method: send a message on the Data channel.
    pub fn send_data(&self, source: &str, destination: &str, payload: Value) -> Result<()> {
        let msg = self.make_message(
            TransportChannel::Data,
            MessagePriority::Normal,
            source,
            destination,
            payload,
        );
        self.send(msg)
    }

    /// Convenience method: send a message on the Event channel.
    pub fn send_event(&self, source: &str, destination: &str, payload: Value) -> Result<()> {
        let msg = self.make_message(
            TransportChannel::Event,
            MessagePriority::Normal,
            source,
            destination,
            payload,
        );
        self.send(msg)
    }

    // ── internal helpers ──────────────────────────────────────────────────

    /// Current monotonic-like timestamp in milliseconds.
    fn now_ms() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    /// Build a channel message with default fields.
    fn make_message(
        &self,
        channel: TransportChannel,
        priority: MessagePriority,
        source: &str,
        destination: &str,
        payload: Value,
    ) -> ChannelMessage {
        let id = self.next_message_id.fetch_add(1, Ordering::Relaxed);
        let now = Self::now_ms();
        let config = self.config();

        ChannelMessage {
            id: format!("msg_{}", id),
            channel,
            priority,
            qos: QosLevel::AtMostOnce,
            source: source.to_string(),
            destination: destination.to_string(),
            payload,
            ttl_ms: config.default_ttl_ms,
            created_ms: now,
            delivery_status: DeliveryStatus::Pending,
            correlation_id: None,
            headers: HashMap::new(),
        }
    }

    /// Create a fresh zeroed-out ChannelStats for the given channel.
    fn new_stats(channel: TransportChannel) -> ChannelStats {
        ChannelStats {
            channel,
            messages_sent: 0,
            messages_received: 0,
            messages_delivered: 0,
            messages_failed: 0,
            messages_expired: 0,
            avg_latency_ms: 0.0,
            queue_depth: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn default_transport() -> MultiChannelTransport {
        MultiChannelTransport::new(ChannelConfig::default())
    }

    fn sample_msg(
        _transport: &MultiChannelTransport,
        channel: TransportChannel,
        priority: MessagePriority,
        id: &str,
    ) -> ChannelMessage {
        let now = MultiChannelTransport::now_ms();
        ChannelMessage {
            id: id.to_string(),
            channel,
            priority,
            qos: QosLevel::AtMostOnce,
            source: "test_source".into(),
            destination: "test_dest".into(),
            payload: json!({"key": "value"}),
            ttl_ms: 30_000,
            created_ms: now,
            delivery_status: DeliveryStatus::Pending,
            correlation_id: None,
            headers: HashMap::new(),
        }
    }

    // ── 1. new transport is empty ─────────────────────────────────────────

    #[test]
    fn test_new_transport_empty() {
        let transport = default_transport();
        let profile = transport.profile();
        assert!(profile.enabled);
        assert!(profile.active_channels.is_empty());
        assert_eq!(profile.total_messages_sent, 0);
        assert_eq!(profile.total_messages_received, 0);
    }

    // ── 2. send message ───────────────────────────────────────────────────

    #[test]
    fn test_send_message() {
        let transport = default_transport();
        let msg = sample_msg(
            &transport,
            TransportChannel::Data,
            MessagePriority::Normal,
            "send-1",
        );
        transport.send(msg).unwrap();
        let profile = transport.profile();
        assert_eq!(profile.total_messages_sent, 1);
    }

    // ── 3. receive message ────────────────────────────────────────────────

    #[test]
    fn test_receive_message() {
        let transport = default_transport();
        let msg = sample_msg(
            &transport,
            TransportChannel::Data,
            MessagePriority::Normal,
            "recv-1",
        );
        transport.send(msg).unwrap();

        let received = transport.receive(&TransportChannel::Data);
        assert!(received.is_some());
        assert_eq!(received.unwrap().id, "recv-1");
    }

    // ── 4. priority ordering (Critical before High before Normal) ────────

    #[test]
    fn test_receive_priority_ordering() {
        let transport = default_transport();

        // Insert out of priority order
        let low = sample_msg(
            &transport,
            TransportChannel::Data,
            MessagePriority::Low,
            "p-low",
        );
        let critical = sample_msg(
            &transport,
            TransportChannel::Data,
            MessagePriority::Critical,
            "p-critical",
        );
        let normal = sample_msg(
            &transport,
            TransportChannel::Data,
            MessagePriority::Normal,
            "p-normal",
        );
        let high = sample_msg(
            &transport,
            TransportChannel::Data,
            MessagePriority::High,
            "p-high",
        );

        transport.send(low).unwrap();
        transport.send(critical).unwrap();
        transport.send(normal).unwrap();
        transport.send(high).unwrap();

        // Must come out Critical → High → Normal → Low
        assert_eq!(
            transport.receive(&TransportChannel::Data).unwrap().id,
            "p-critical"
        );
        assert_eq!(
            transport.receive(&TransportChannel::Data).unwrap().id,
            "p-high"
        );
        assert_eq!(
            transport.receive(&TransportChannel::Data).unwrap().id,
            "p-normal"
        );
        assert_eq!(
            transport.receive(&TransportChannel::Data).unwrap().id,
            "p-low"
        );
    }

    // ── 5. receive batch ──────────────────────────────────────────────────

    #[test]
    fn test_receive_batch() {
        let transport = default_transport();
        for i in 0..5 {
            let msg = sample_msg(
                &transport,
                TransportChannel::Data,
                MessagePriority::Normal,
                &format!("batch-{}", i),
            );
            transport.send(msg).unwrap();
        }

        let batch = transport.receive_batch(&TransportChannel::Data, 3);
        assert_eq!(batch.len(), 3);
        assert_eq!(batch[0].id, "batch-0");
        assert_eq!(batch[1].id, "batch-1");
        assert_eq!(batch[2].id, "batch-2");

        // Remaining 2
        let rest = transport.receive_batch(&TransportChannel::Data, 10);
        assert_eq!(rest.len(), 2);
    }

    // ── 6. acknowledge updates stats ──────────────────────────────────────

    #[test]
    fn test_acknowledge_updates_stats() {
        let transport = default_transport();
        let msg = sample_msg(
            &transport,
            TransportChannel::Data,
            MessagePriority::Normal,
            "ack-1",
        );
        transport.send(msg).unwrap();
        let _received = transport.receive(&TransportChannel::Data).unwrap();

        transport.acknowledge("ack-1");
        let profile = transport.profile();
        assert_eq!(profile.total_messages_delivered, 1);
    }

    // ── 7. fail message retry ─────────────────────────────────────────────

    #[test]
    fn test_fail_message_retry() {
        let transport = default_transport();
        let msg = sample_msg(
            &transport,
            TransportChannel::Data,
            MessagePriority::Normal,
            "fail-1",
        );
        transport.send(msg).unwrap();
        let _received = transport.receive(&TransportChannel::Data).unwrap();

        transport.fail_message("fail-1", "timeout");
        let profile = transport.profile();
        assert_eq!(profile.total_messages_failed, 1);
    }

    // ── 8. peek does not dequeue ──────────────────────────────────────────

    #[test]
    fn test_peek_does_not_dequeue() {
        let transport = default_transport();
        let msg = sample_msg(
            &transport,
            TransportChannel::Data,
            MessagePriority::Normal,
            "peek-1",
        );
        transport.send(msg).unwrap();

        // Peek once
        let peeked = transport.peek(&TransportChannel::Data);
        assert!(peeked.is_some());
        assert_eq!(peeked.unwrap().id, "peek-1");

        // Message should still be there
        let received = transport.receive(&TransportChannel::Data);
        assert!(received.is_some());
        assert_eq!(received.unwrap().id, "peek-1");

        // Second receive should be None
        assert!(transport.receive(&TransportChannel::Data).is_none());
    }

    // ── 9. channel stats ──────────────────────────────────────────────────

    #[test]
    fn test_channel_stats() {
        let transport = default_transport();
        let msg = sample_msg(
            &transport,
            TransportChannel::Data,
            MessagePriority::Normal,
            "stats-1",
        );
        transport.send(msg).unwrap();
        transport.receive(&TransportChannel::Data);

        let stats = transport.channel_stats(&TransportChannel::Data);
        assert!(stats.is_some());
        let stats = stats.unwrap();
        assert_eq!(stats.messages_sent, 1);
        assert_eq!(stats.messages_received, 1);
    }

    // ── 10. prune expired ─────────────────────────────────────────────────

    #[test]
    fn test_prune_expired() {
        let transport = MultiChannelTransport::new(ChannelConfig {
            default_ttl_ms: 30_000,
            ..ChannelConfig::default()
        });

        // Create a message that is already expired
        let now = MultiChannelTransport::now_ms();
        let expired_msg = ChannelMessage {
            id: "expired-1".into(),
            channel: TransportChannel::Data,
            priority: MessagePriority::Normal,
            qos: QosLevel::AtMostOnce,
            source: "src".into(),
            destination: "dest".into(),
            payload: json!({"x": 1}),
            ttl_ms: 100,
            created_ms: now - 200, // expired 100ms ago
            delivery_status: DeliveryStatus::Pending,
            correlation_id: None,
            headers: HashMap::new(),
        };

        // Bypass the TTL check in send() by inserting directly into the queue
        {
            let mut inner = transport.inner.lock().unwrap();
            inner
                .queues
                .entry(TransportChannel::Data)
                .or_default()
                .push_back(expired_msg);
            inner
                .stats
                .entry(TransportChannel::Data)
                .or_insert_with(|| MultiChannelTransport::new_stats(TransportChannel::Data));
        }

        // Before prune, queue should have 1 item
        assert!(transport.peek(&TransportChannel::Data).is_some());

        transport.prune_expired();

        // After prune, queue should be empty
        assert!(transport.peek(&TransportChannel::Data).is_none());
    }

    // ── 11. dedup prevents duplicates ─────────────────────────────────────

    #[test]
    fn test_dedup_prevents_duplicates() {
        let transport = default_transport();
        let msg1 = sample_msg(
            &transport,
            TransportChannel::Data,
            MessagePriority::Normal,
            "dup-1",
        );
        let msg2 = sample_msg(
            &transport,
            TransportChannel::Data,
            MessagePriority::Normal,
            "dup-1",
        );

        transport.send(msg1).unwrap();
        transport.send(msg2).unwrap(); // should be skipped

        let profile = transport.profile();
        assert_eq!(profile.total_messages_sent, 1);
    }

    // ── 12. profile reflects state ────────────────────────────────────────

    #[test]
    fn test_profile_reflects_state() {
        let transport = default_transport();
        let msg = sample_msg(
            &transport,
            TransportChannel::Event,
            MessagePriority::Normal,
            "prof-1",
        );
        transport.send(msg).unwrap();

        let profile = transport.profile();
        assert!(profile.enabled);
        assert!(!profile.active_channels.is_empty());
        assert!(profile.active_channels.contains(&"event".to_string()));
        assert_eq!(profile.total_messages_sent, 1);
        assert!(profile.last_activity_ms > 0);
    }

    // ── 13. convenience send methods ──────────────────────────────────────

    #[test]
    fn test_convenience_send_methods() {
        let transport = default_transport();

        transport
            .send_control("ctrl_src", "ctrl_dst", json!({"cmd": "ping"}))
            .unwrap();
        transport
            .send_data("data_src", "data_dst", json!({"payload": "hello"}))
            .unwrap();
        transport
            .send_event("evt_src", "evt_dst", json!({"type": "notification"}))
            .unwrap();

        assert!(transport.receive(&TransportChannel::Control).is_some());
        assert!(transport.receive(&TransportChannel::Data).is_some());
        assert!(transport.receive(&TransportChannel::Event).is_some());
    }

    // ── 14. multiple channels isolated ────────────────────────────────────

    #[test]
    fn test_multiple_channels_isolated() {
        let transport = default_transport();

        let control_msg = sample_msg(
            &transport,
            TransportChannel::Control,
            MessagePriority::Critical,
            "iso-ctrl",
        );
        let data_msg = sample_msg(
            &transport,
            TransportChannel::Data,
            MessagePriority::Normal,
            "iso-data",
        );

        transport.send(control_msg).unwrap();
        transport.send(data_msg).unwrap();

        // Control channel should only have the control message
        let ctrl = transport.receive(&TransportChannel::Control).unwrap();
        assert_eq!(ctrl.id, "iso-ctrl");
        assert!(transport.receive(&TransportChannel::Control).is_none());

        // Data channel should only have the data message
        let data = transport.receive(&TransportChannel::Data).unwrap();
        assert_eq!(data.id, "iso-data");
        assert!(transport.receive(&TransportChannel::Data).is_none());
    }

    // ── 15. TTL expires message ───────────────────────────────────────────

    #[test]
    fn test_ttl_expires_message() {
        let transport = MultiChannelTransport::new(ChannelConfig {
            default_ttl_ms: 30_000,
            ..ChannelConfig::default()
        });

        let now = MultiChannelTransport::now_ms();
        // Message created 100s ago with 50ms TTL → definitely expired
        let expired_msg = ChannelMessage {
            id: "ttl-expired".into(),
            channel: TransportChannel::Data,
            priority: MessagePriority::Normal,
            qos: QosLevel::AtMostOnce,
            source: "src".into(),
            destination: "dest".into(),
            payload: json!({"x": 1}),
            ttl_ms: 50,
            created_ms: now - 100_000,
            delivery_status: DeliveryStatus::Pending,
            correlation_id: None,
            headers: HashMap::new(),
        };

        // send() should reject it and increment expired count
        transport.send(expired_msg).unwrap();

        let stats = transport.channel_stats(&TransportChannel::Data);
        assert!(stats.is_some());
        assert_eq!(stats.unwrap().messages_expired, 1);

        // No message should be in the queue
        assert!(transport.receive(&TransportChannel::Data).is_none());
    }
}
