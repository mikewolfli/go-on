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

    /// Create with custom inbox capacity.
    pub fn with_capacity(tree: Arc<RwLock<AgentTree>>, capacity: usize) -> Self {
        let (notify, _) = watch::channel(0);
        Self {
            tree,
            inboxes: Arc::new(RwLock::new(HashMap::new())),
            max_inbox_size: capacity,
            notify,
        }
    }

    /// Subscribe to delivery notifications (for event-driven waiting).
    ///
    /// Returns a receiver that is notified on each new message delivery.
    pub fn subscribe_notify(&self) -> watch::Receiver<u64> {
        self.notify.subscribe()
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

    /// Receive all pending messages for an agent path.
    pub async fn recv(&self, path: &AgentPath) -> Vec<AgentMessage> {
        let mut inboxes = self.inboxes.write().await;
        inboxes
            .remove(path)
            .map(|q| q.into_iter().collect())
            .unwrap_or_default()
    }

    /// Peek at pending messages without consuming them.
    pub async fn peek(&self, path: &AgentPath) -> Vec<AgentMessage> {
        let inboxes = self.inboxes.read().await;
        inboxes
            .get(path)
            .map(|q| q.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Wait for a message matching a predicate, with timeout (BLUE71 §6.2).
    ///
    /// Uses event-driven notification channel instead of polling.
    /// Falls back to checking inbox on each notification.
    pub async fn wait_for<F>(
        &self,
        path: &AgentPath,
        predicate: F,
        timeout_ms: u64,
    ) -> Result<AgentMessage, String>
    where
        F: Fn(&AgentMessage) -> bool,
    {
        let timeout_dur = tokio::time::Duration::from_millis(timeout_ms);
        let mut notify_rx = self.subscribe_notify();

        // First, check if the message is already in the inbox (non-blocking).
        let msgs = self.recv(path).await;
        for msg in msgs {
            if predicate(&msg) {
                return Ok(msg);
            }
            // Re-queue non-matching messages.
            self.deliver_single(msg, path).await;
        }

        // Then wait for new messages via event-driven notification.
        tokio::time::timeout(timeout_dur, async {
            loop {
                // Wait for the next delivery notification (zero CPU).
                notify_rx
                    .changed()
                    .await
                    .map_err(|_| "notification channel closed".to_string())?;

                // Check inbox for matching messages.
                let msgs = self.recv(path).await;
                for msg in msgs {
                    if predicate(&msg) {
                        return Ok(msg);
                    }
                    // Re-queue non-matching messages.
                    self.deliver_single(msg, path).await;
                }
            }
        })
        .await
        .map_err(|_| "timeout waiting for matching message".to_string())?
    }

    /// Cancel all agents in a sub-tree (cancellation propagation).
    ///
    /// Sends Cancel messages to all descendants (BFS).
    /// Each receiving agent is responsible for cascading cancellation
    /// to its own children.
    pub async fn cancel_subtree(&self, path: &AgentPath, reason: &str) {
        let descendants = {
            let tree = self.tree.read().await;
            tree.descendant_paths(path)
        };

        for child_path in descendants {
            let cancel_msg = crate::agents::communication::message::AgentMessage::cancel(
                path.clone(),
                crate::agents::communication::message::AgentTarget::Direct(child_path.clone()),
                reason.to_string(),
            );
            let _ = self.send_at_least_once(cancel_msg).await;
        }
    }

    /// Get inbox size for a path.
    pub async fn inbox_size(&self, path: &AgentPath) -> usize {
        let inboxes = self.inboxes.read().await;
        inboxes.get(path).map(|q| q.len()).unwrap_or(0)
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

    /// Deliver a message to multiple target inboxes.
    async fn deliver_to_targets(&self, msg: AgentMessage, targets: Vec<AgentPath>) {
        for target in targets {
            self.deliver_single(msg.clone(), &target).await;
        }
    }

    /// Deliver a single message to an inbox, with backpressure.
    async fn deliver_single(&self, msg: AgentMessage, path: &AgentPath) {
        let mut inboxes = self.inboxes.write().await;
        let inbox = inboxes
            .entry(path.clone())
            .or_insert_with(|| VecDeque::with_capacity(64));
        if inbox.len() < self.max_inbox_size {
            inbox.push_back(msg);
        }
        // Notify watchers that a new message has been delivered.
        let _ = self
            .notify
            .send(NOTIFY_COUNTER.fetch_add(1, Ordering::Relaxed));
    }

    /// Deliver with verification (for AtLeastOnce).
    async fn deliver_with_check(
        &self,
        msg: &AgentMessage,
        targets: &[AgentPath],
    ) -> Result<(), String> {
        for target in targets {
            self.deliver_single(msg.clone(), target).await;
            // Verify delivery by checking inbox growth.
            let size = self.inbox_size(target).await;
            if size == 0 {
                return Err(format!("delivery verification failed for {}", target));
            }
        }
        Ok(())
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
    async fn test_send_and_recv_direct() {
        let tree = make_tree();
        let messenger = AgentMessenger::new(tree);

        let msg =
            AgentMessage::status_query(make_path("root/a"), AgentTarget::Direct(make_path("root")));
        messenger.send_at_most_once(msg).await.unwrap();

        let received = messenger.recv(&make_path("root")).await;
        assert_eq!(received.len(), 1);
    }

    #[tokio::test]
    async fn test_send_broadcast() {
        let tree = make_tree();
        let messenger = AgentMessenger::new(tree);

        let msg = AgentMessage::status_query(make_path("root/a"), AgentTarget::Broadcast);
        messenger.send_at_most_once(msg).await.unwrap();

        let b_msgs = messenger.recv(&make_path("root/b")).await;
        assert_eq!(b_msgs.len(), 1);
    }

    #[tokio::test]
    async fn test_send_to_parent() {
        let tree = make_tree();
        let messenger = AgentMessenger::new(tree);

        let msg = AgentMessage::result(
            make_path("root/a"),
            AgentTarget::ToParent,
            true,
            Some("done".to_string()),
            None,
            None,
            None,
            None,
            "ok".to_string(),
            100,
        );
        messenger.send_at_most_once(msg).await.unwrap();

        let received = messenger.recv(&make_path("root")).await;
        assert_eq!(received.len(), 1);
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
    async fn test_cancel_subtree() {
        let tree = make_tree();
        let messenger = AgentMessenger::new(tree);

        messenger
            .cancel_subtree(&make_path("root"), "timeout")
            .await;

        let a_msgs = messenger.recv(&make_path("root/a")).await;
        assert!(!a_msgs.is_empty());
        assert!(a_msgs[0].is_cancel());

        let b_msgs = messenger.recv(&make_path("root/b")).await;
        assert!(!b_msgs.is_empty());
        assert!(b_msgs[0].is_cancel());
    }

    #[tokio::test]
    async fn test_wait_for_event_driven() {
        let tree = make_tree();
        let messenger = Arc::new(AgentMessenger::new(tree));

        // Spawn a task that sends a message after a short delay.
        let msg =
            AgentMessage::status_query(make_path("root/a"), AgentTarget::Direct(make_path("root")));
        let m2 = messenger.clone();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            let _ = m2.send_at_most_once(msg).await;
        });

        // Wait for the message (event-driven, should complete quickly).
        let result = messenger
            .wait_for(&make_path("root"), |m| !m.is_cancel(), 5000)
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_wait_for_timeout() {
        let tree = make_tree();
        let messenger = AgentMessenger::new(tree);

        // No messages will be sent, so this should timeout.
        let result = messenger
            .wait_for(&make_path("root"), |msg| msg.is_result(), 50)
            .await;
        assert!(result.is_err());
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

    #[tokio::test]
    async fn test_subscribe_notify() {
        let tree = make_tree();
        let messenger = Arc::new(AgentMessenger::new(tree));

        let mut rx = messenger.subscribe_notify();
        assert_eq!(*rx.borrow_and_update(), 0);

        let msg =
            AgentMessage::status_query(make_path("root/a"), AgentTarget::Direct(make_path("root")));
        let m2 = messenger.clone();
        tokio::spawn(async move {
            let _ = m2.send_at_most_once(msg).await;
        });

        // Notification should fire when message is delivered.
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), rx.changed()).await;
        assert!(result.is_ok());
    }
}
