//! AgentMessenger — message routing and delivery (BLUE70 §5)
//!
//! Provides inter-agent message sending and receiving with two delivery
//! levels: AtMostOnce (fire-and-forget) and AtLeastOnce (ack+retry).
//! Inbox-based message storage with optional ObservabilityBus integration.
//!
//! BLUE71 §6: Event-driven state propagation via watch channel.
//! `wait_for` now uses `notify.changed().await` instead of polling.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::watch;
use tokio::sync::RwLock;

use crate::agents::communication::message::AgentMessage;
use crate::agents::communication::path::AgentPath;
use crate::agents::communication::tree::AgentTree;

/// Default maximum messages per inbox.
const DEFAULT_INBOX_CAPACITY: usize = 1024;

/// Counter for notification sequence numbers.
static NOTIFY_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Message routing and delivery system (BLUE70 §5).
///
/// Design notes:
/// - Inbox-based: each agent path has a FIFO inbox.
/// - Two delivery levels (AtMostOnce / AtLeastOnce).
/// - ObservabilityBus integration optional via callbacks.
/// - Event-driven: `notify` watch channel signals new message arrivals (BLUE71 §6).
pub struct AgentMessenger {
    /// Agent tree reference (for route resolution).
    tree: Arc<RwLock<AgentTree>>,
    /// Per-path inboxes: path -> message queue.
    inboxes: Arc<RwLock<HashMap<AgentPath, VecDeque<AgentMessage>>>>,
    /// Maximum messages per inbox before backpressure.
    max_inbox_size: usize,
    /// Notification channel: incremented on each delivery (BLUE71 §6.2).
    notify: watch::Sender<u64>,
}

impl AgentMessenger {
    /// Create a new AgentMessenger with the given tree reference.
    pub fn new(tree: Arc<RwLock<AgentTree>>) -> Self {
        let (notify, _) = watch::channel(0);
        Self {
            tree,
            inboxes: Arc::new(RwLock::new(HashMap::new())),
            max_inbox_size: DEFAULT_INBOX_CAPACITY,
            notify,
        }
    }

    /// Send a message with AtMostOnce delivery (fire-and-forget).
    ///
    /// Messages are delivered to matching receivers' inboxes immediately.
    /// No acknowledgement or retry is performed.
    pub async fn send_at_most_once(&self, msg: AgentMessage) -> Result<(), String> {
        let targets = self.resolve_targets(&msg).await;
        if targets.is_empty() {
            return Err(format!(
                "no receivers matched for message target: {}",
                msg.to
            ));
        }
        self.deliver_to_targets(msg, targets).await;
        Ok(())
    }

    /// Send a message with AtLeastOnce delivery (acknowledgement + retry).
    ///
    /// Attempts delivery up to 3 times with a short delay between retries.
    /// Returns an error if all attempts fail.
    pub async fn send_at_least_once(&self, msg: AgentMessage) -> Result<(), String> {
        let targets = self.resolve_targets(&msg).await;
        if targets.is_empty() {
            return Err(format!(
                "no receivers matched for message target: {}",
                msg.to
            ));
        }

        let max_attempts = 3;
        let mut last_error = None;

        for attempt in 0..max_attempts {
            let result = self.deliver_with_check(&msg, &targets).await;
            match result {
                Ok(()) => return Ok(()),
                Err(e) => {
                    last_error = Some(e);
                    if attempt + 1 < max_attempts {
                        tokio::time::sleep(tokio::time::Duration::from_millis(
                            100 * (attempt + 1) as u64,
                        ))
                        .await;
                    }
                }
            }
        }

        Err(format!(
            "message delivery failed after {} attempts: {:?}",
            max_attempts, last_error
        ))
    }

    /// Send with automatic delivery guarantee selection based on message kind.
    ///
    /// - Delegate, Cancel, Result -> AtLeastOnce
    /// - Progress, StatusQuery, Custom -> AtMostOnce
    pub async fn send(&self, msg: AgentMessage) -> Result<(), String> {
        match msg.kind {
            crate::agents::communication::message::AgentMessageKind::Delegate { .. }
            | crate::agents::communication::message::AgentMessageKind::Cancel { .. }
            | crate::agents::communication::message::AgentMessageKind::Result { .. } => {
                self.send_at_least_once(msg).await
            }
            _ => self.send_at_most_once(msg).await,
        }
    }

    // -- Private helpers --------------------------------------------------

    /// Resolve target agents for a message.
    async fn resolve_targets(&self, msg: &AgentMessage) -> Vec<AgentPath> {
        let tree = self.tree.read().await;
        match &msg.to {
            crate::agents::communication::message::AgentTarget::ToParent => {
                let parent = msg.from.parent();
                parent.into_iter().collect()
            }
            target => {
                let nodes = tree.resolve_target(target);
                nodes.into_iter().map(|n| n.path.clone()).collect()
            }
        }
    }

    /// Deliver a message to multiple target inboxes (fire-and-forget — used
    /// for non-critical payloads like observability messages).
    async fn deliver_to_targets(&self, msg: AgentMessage, targets: Vec<AgentPath>) {
        for target in targets {
            self.deliver_single(msg.clone(), &target).await;
        }
    }

    /// Deliver a single message to an inbox, with backpressure.
    ///
    /// Returns `true` when the message was queued, `false` when the inbox was
    /// at capacity and the message was dropped (callers that need AtLeastOnce
    /// delivery must check the result).
    async fn deliver_single(&self, msg: AgentMessage, path: &AgentPath) -> bool {
        let mut inboxes = self.inboxes.write().await;
        let inbox = inboxes
            .entry(path.clone())
            .or_insert_with(|| VecDeque::with_capacity(64));
        if inbox.len() < self.max_inbox_size {
            inbox.push_back(msg);
            // Notify watchers that a new message has been delivered.
            let _ = self
                .notify
                .send(NOTIFY_COUNTER.fetch_add(1, Ordering::Relaxed));
            true
        } else {
            false
        }
    }

    /// Deliver with verification (for AtLeastOnce).
    ///
    /// Verifies that THIS message was actually queued. The previous check
    /// (inbox size > 0) passed whenever any earlier message existed, so a
    /// dropped message was still reported as delivered and retries were
    /// skipped.
    async fn deliver_with_check(
        &self,
        msg: &AgentMessage,
        targets: &[AgentPath],
    ) -> Result<(), String> {
        for target in targets {
            let delivered = self.deliver_single(msg.clone(), target).await;
            if !delivered {
                return Err(format!("delivery failed for {}: inbox at capacity", target));
            }
        }
        Ok(())
    }

    /// Get inbox size for a path.
    pub async fn inbox_size(&self, path: &AgentPath) -> usize {
        let inboxes = self.inboxes.read().await;
        inboxes.get(path).map(|q| q.len()).unwrap_or(0)
    }

    /// Remove the inbox for `path` (and any retained messages).
    ///
    /// Called when a spawned agent finishes so the inbox map does not
    /// accumulate one entry per completed spawn forever.
    pub async fn remove_inbox(&self, path: &AgentPath) {
        let mut inboxes = self.inboxes.write().await;
        inboxes.remove(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::communication::message::AgentTarget;
    use crate::agents::communication::tree::AgentNodeMetadata;

    fn make_path(s: &str) -> AgentPath {
        AgentPath::parse(s).unwrap()
    }

    fn make_tree() -> Arc<RwLock<AgentTree>> {
        let mut tree = AgentTree::new();
        tree.register(&make_path("root"), "main", AgentNodeMetadata::new())
            .unwrap();
        tree.register(&make_path("root/a"), "agent_a", AgentNodeMetadata::new())
            .unwrap();
        tree.register(&make_path("root/b"), "agent_b", AgentNodeMetadata::new())
            .unwrap();
        Arc::new(RwLock::new(tree))
    }

    #[tokio::test]
    async fn test_send_to_nonexistent() {
        let tree = make_tree();
        let messenger = AgentMessenger::new(tree);

        let msg = AgentMessage::status_query(
            make_path("root/a"),
            AgentTarget::Direct(make_path("root/nonexistent")),
        );
        assert!(messenger.send_at_most_once(msg).await.is_err());
    }

    #[tokio::test]
    async fn test_auto_delivery_guarantee() {
        let tree = make_tree();
        let messenger = AgentMessenger::new(tree);

        // Delegate -> AtLeastOnce
        let delegate = AgentMessage::delegate(
            make_path("root"),
            AgentTarget::Direct(make_path("root/a")),
            "task".to_string(),
            None,
            None,
            300,
        );
        assert!(messenger.send(delegate).await.is_ok());

        // Progress -> AtMostOnce
        let progress = AgentMessage::progress(
            make_path("root/a"),
            AgentTarget::ToParent,
            "working...".to_string(),
            true,
        );
        assert!(messenger.send(progress).await.is_ok());
    }
}
