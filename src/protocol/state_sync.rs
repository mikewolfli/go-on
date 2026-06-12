//! State sync event types and broadcaster for cross-client state synchronization.
//!
//! The backend exposes a [`StateSyncBroadcaster`] that publishes state change events
//! so that connected clients (GUI, VSCode addon) can react without polling.
//!
//! # Architecture
//!
//! ```text
//! Backend (config reload, model change, agent change)
//!   └── StateSyncBroadcaster (tokio::sync::broadcast)
//!         ├── /v1/state/events SSE endpoint (streams to connected clients)
//!         └── Each subscriber gets every event (fan-out via broadcast channel)
//! ```

use std::sync::LazyLock;
use tokio::sync::broadcast;

/// Maximum number of events buffered in the broadcast channel before old events
/// are dropped for slow consumers.
const BROADCAST_CAPACITY: usize = 256;

/// Events emitted by the backend when state changes.
///
/// Each variant carries a human-readable description for display in client UIs
/// and enough structured data for the client to react intelligently.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StateSyncEvent {
    /// The list of available models has changed.
    ModelsChanged {
        /// New model identifiers.
        models: Vec<String>,
    },
    /// Server configuration was hot-reloaded.
    ConfigReloaded {
        /// Keys (or sections) that were modified, if detectable.
        changed_keys: Vec<String>,
    },
    /// Agent definitions were added or removed.
    AgentsChanged {
        /// Agent names that were added.
        added: Vec<String>,
        /// Agent names that were removed.
        removed: Vec<String>,
    },
    /// Server is about to restart (configuration change requiring reboot).
    BackendRestarting {
        /// Reason for the restart.
        reason: String,
        /// Milliseconds until restart begins.
        restart_in_ms: u64,
    },
    /// Periodic keep-alive sent to connected SSE clients.
    Heartbeat {
        /// Unix timestamp in milliseconds.
        timestamp: u64,
    },
}

impl StateSyncEvent {
    /// Human-readable summary for display in client status bars / notifications.
    #[allow(dead_code)]
    pub fn summary(&self) -> String {
        match self {
            StateSyncEvent::ModelsChanged { models } => {
                format!("Models changed ({} models)", models.len())
            }
            StateSyncEvent::ConfigReloaded { changed_keys } => {
                if changed_keys.is_empty() {
                    "Config reloaded".to_string()
                } else {
                    format!("Config reloaded: {}", changed_keys.join(", "))
                }
            }
            StateSyncEvent::AgentsChanged { added, removed } => {
                let mut parts = vec![];
                if !added.is_empty() {
                    parts.push(format!("+{} agents", added.len()));
                }
                if !removed.is_empty() {
                    parts.push(format!("-{} agents", removed.len()));
                }
                format!("Agents changed ({})", parts.join(", "))
            }
            StateSyncEvent::BackendRestarting { reason, .. } => {
                format!("Backend restarting: {}", reason)
            }
            StateSyncEvent::Heartbeat { .. } => "heartbeat".to_string(),
        }
    }
}

/// Shared state sync broadcaster accessible from anywhere in the backend.
///
/// Uses a global static `broadcast::Sender` so that config reload observers,
/// agent registries, and model list updaters can all publish without needing
/// to thread a shared reference through every layer.
///
/// # Single-process limitation
///
/// This is a single-process in-memory broadcaster. The `static` ties all
/// publishers and subscribers to the same process, so it cannot coordinate
/// across multiple backend instances.
///
/// For multi-process deployments (horizontal scaling, blue/green), this
/// should be replaced with a distributed pub/sub system such as:
/// - **NATS** (JetStream) for lightweight messaging
/// - **Redis Pub/Sub** for simple fan-out
/// - **Kafka / Pulsar** for durable event logs
///
/// The capacity of 256 is set intentionally to provide backpressure — if a
/// consumer falls behind by more than 256 events, the oldest events are
/// dropped rather than unboundedly growing memory.
static BROADCASTER: LazyLock<broadcast::Sender<StateSyncEvent>> = LazyLock::new(|| {
    let (tx, _rx) = broadcast::channel(BROADCAST_CAPACITY);
    tx
});

/// Publish a state sync event to all connected subscribers.
pub fn publish_event(event: StateSyncEvent) {
    let _ = BROADCASTER.send(event);
}

/// Subscribe to state sync events.
///
/// Returns a `broadcast::Receiver` that will receive all future events.
/// If the receiver falls behind (more than [`BROADCAST_CAPACITY`] events
/// queued), old events will be dropped. The receiver should be polled
/// frequently or recreated.
pub fn subscribe() -> broadcast::Receiver<StateSyncEvent> {
    BROADCASTER.subscribe()
}

/// Number of active subscribers currently listening.
#[allow(dead_code)]
pub fn subscriber_count() -> usize {
    BROADCASTER.receiver_count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_sync_event_roundtrip() {
        let event = StateSyncEvent::ConfigReloaded {
            changed_keys: vec!["provider".to_string(), "agents".to_string()],
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "config_reloaded");
        assert_eq!(json["changed_keys"][0], "provider");

        let deserialized: StateSyncEvent = serde_json::from_value(json).unwrap();
        match deserialized {
            StateSyncEvent::ConfigReloaded { changed_keys } => {
                assert_eq!(changed_keys.len(), 2);
            }
            other => panic!("expected ConfigReloaded, got {:?}", other),
        }
    }

    #[test]
    fn test_subscribe_and_publish() {
        let mut rx = subscribe();
        let event = StateSyncEvent::Heartbeat { timestamp: 12345 };
        publish_event(event);

        match rx.try_recv() {
            Ok(StateSyncEvent::Heartbeat { timestamp }) => {
                assert_eq!(timestamp, 12345);
            }
            Ok(other) => panic!("expected Heartbeat, got {:?}", other),
            Err(e) => panic!("expected event, got {:?}", e),
        }
    }

    #[test]
    fn test_summary() {
        let e1 = StateSyncEvent::ModelsChanged {
            models: vec!["gpt-4".into(), "claude-3".into()],
        };
        assert!(e1.summary().contains("2 models"));

        let e2 = StateSyncEvent::ConfigReloaded {
            changed_keys: vec![],
        };
        assert_eq!(e2.summary(), "Config reloaded");

        let e3 = StateSyncEvent::Heartbeat { timestamp: 0 };
        assert_eq!(e3.summary(), "heartbeat");
    }
}
