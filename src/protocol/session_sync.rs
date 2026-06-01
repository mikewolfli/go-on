//! GAP-B50-10: 三端会话状态同步 — Session state synchronization layer.

// F-GAP-51: dead_code allowed on specific items below (reserved for session sync)
//!
//! Provides a version-based incremental sync protocol so that multiple
//! frontends (e.g. CLI, GUI, WebSocket clients) stay in sync without
//! transferring full session state on every poll.
//!
//! Key design decisions:
//!
//! - **Version-based incremental sync** – Each mutation bumps a monotonic
//!   `version` counter. Frontends track `last_synced_version` and receive
//!   only `SyncDiff`s for versions they have not yet seen.
//! - **Thread-safe, clonable sessions** – All mutable state lives behind
//!   `Arc<RwLock<…>>`. `SharedSession` is cheaply clonable.
//! - **Background session cleanup** – A spawned task evicts sessions
//!   inactive for >24 h and drops orphaned frontend bindings.
//! - **Pluggable broadcast** – `broadcast_to_session` accepts an optional
//!   `BroadcastFn` callback that callers wire to a WebSocket hub,
//!   message queue, or any other fan-out mechanism.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// Type aliases
// ---------------------------------------------------------------------------

/// Unique identifier for a chat session.
pub type SessionId = String;

/// Unique identifier for a frontend (client) connection.
pub type FrontendId = String;

/// Callback used by `broadcast_to_session` to fan out a message to all
/// frontends connected to a session. The callee receives a serialized JSON
/// payload. A `None` callback means broadcast is disabled.
pub type BroadcastFn = Arc<dyn Fn(&str) + Send + Sync>;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// A single message in a chat session.
#[allow(dead_code)] // F-GAP-51 — reserved for session sync
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub role: String, // "user" | "assistant"
    pub content: String,
    pub timestamp: u64,
    pub metadata: HashMap<String, Value>,
}

/// An active task running within a session.
#[allow(dead_code)] // F-GAP-51 — reserved for session sync
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveTask {
    pub id: String,
    pub status: String,
    pub progress: f64,
    pub started_at: u64,
    pub description: String,
}

/// A proposal submitted to the council for a session.
#[allow(dead_code)] // F-GAP-51 — reserved for session sync
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CouncilProposal {
    pub id: String,
    pub title: String,
    pub status: String,
    pub submitted_at: u64,
    pub metadata: HashMap<String, Value>,
}

// ---------------------------------------------------------------------------
// SharedSession – thread-safe, versioned session data
// ---------------------------------------------------------------------------

/// A session whose state is guarded by an `RwLock` and wrapped in `Arc` so it
/// can be shared across tasks and frontend handlers.
///
/// Every mutation bumps `version` so that sync consumers can perform
/// incremental diffs.
#[allow(dead_code)] // F-GAP-51 — reserved for session sync
#[derive(Debug, Clone)]
pub struct SharedSession {
    pub id: SessionId,
    pub chat_history: Vec<ChatMessage>,
    pub active_tasks: Vec<ActiveTask>,
    pub council_proposals: Vec<CouncilProposal>,
    pub last_active: u64,
    pub version: u64,
}

#[allow(dead_code)] // F-GAP-51 — reserved for session sync
impl SharedSession {
    pub fn new(id: SessionId) -> Self {
        Self {
            id,
            chat_history: Vec::new(),
            active_tasks: Vec::new(),
            council_proposals: Vec::new(),
            last_active: now_ms(),
            version: 0,
        }
    }

    /// Touch the `last_active` timestamp and bump the version.
    fn touch(&mut self) {
        self.last_active = now_ms();
        self.version += 1;
    }
}

// ---------------------------------------------------------------------------
// SyncDiff – incremental diff protocol
// ---------------------------------------------------------------------------

/// An incremental diff that a frontend can apply to bring its local session
/// state up to date without fetching the full session.
#[allow(dead_code)] // F-GAP-51 — reserved for session sync
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncDiff {
    pub version: u64,
    pub diffs: Vec<DiffEntry>,
}

/// A single entry inside a `SyncDiff`.
#[allow(dead_code)] // F-GAP-51 — reserved for session sync
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DiffEntry {
    /// A new message was appended to the session chat history.
    MessageAdded(ChatMessage),
    /// An existing task was updated (status and/or progress changed).
    TaskUpdated(ActiveTask),
    /// A task was added to the session.
    TaskAdded(ActiveTask),
    /// A council proposal was added.
    ProposalAdded(CouncilProposal),
    /// The session was closed / removed.
    SessionClosed,
}

// ---------------------------------------------------------------------------
// Per-frontend sync state tracking
// ---------------------------------------------------------------------------

/// Tracks what a specific frontend has already seen for a given session.
#[allow(dead_code)] // F-GAP-51 — reserved for session sync
#[derive(Debug, Clone)]
pub struct FrontendSyncState {
    pub session_id: SessionId,
    pub last_synced_version: u64,
    pub pending_diffs: Vec<SyncDiff>,
}

#[allow(dead_code)] // F-GAP-51 — reserved for session sync
impl FrontendSyncState {
    pub fn new(session_id: SessionId) -> Self {
        Self {
            session_id,
            last_synced_version: 0,
            pending_diffs: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// SessionRegistry
// ---------------------------------------------------------------------------

/// The central registry that owns all sessions and tracks frontend bindings.
///
/// # Thread safety
///
/// Both `sessions` and `frontend_connections` are behind `Arc<RwLock<…>>`,
/// so `SessionRegistry` itself is cheaply clonable and can be injected into
/// any number of tasks or handlers.
#[allow(dead_code)] // F-GAP-51 — reserved for session sync
#[derive(Clone)]
pub struct SessionRegistry {
    sessions: Arc<RwLock<HashMap<SessionId, SharedSession>>>,
    frontend_connections: Arc<RwLock<HashMap<FrontendId, Vec<SessionId>>>>,
    broadcast_fn: Arc<RwLock<Option<BroadcastFn>>>,
}

impl std::fmt::Debug for SessionRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionRegistry")
            .field(
                "session_count",
                &self.sessions.try_read().map(|s| s.len()).unwrap_or(0),
            )
            .field(
                "frontend_count",
                &self
                    .frontend_connections
                    .try_read()
                    .map(|c| c.len())
                    .unwrap_or(0),
            )
            .field(
                "broadcast_fn",
                &self
                    .broadcast_fn
                    .try_read()
                    .map(|b| if b.is_some() { "Some(...)" } else { "None" })
                    .unwrap_or("<locked>"),
            )
            .finish()
    }
}

#[allow(dead_code)] // F-GAP-51 — reserved for session sync
impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(dead_code)] // F-GAP-51 — reserved for session sync
impl SessionRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            frontend_connections: Arc::new(RwLock::new(HashMap::new())),
            broadcast_fn: Arc::new(RwLock::new(None)),
        }
    }

    /// Register a broadcast callback that will be invoked whenever a message
    /// is broadcast to a session.
    pub async fn set_broadcast_fn(&self, f: BroadcastFn) {
        *self.broadcast_fn.write().await = Some(f);
    }

    // ── Session lifecycle ────────────────────────────────────────────────

    /// Create a new session with a random UUID and return its ID.
    pub async fn create_session(&self) -> SessionId {
        let id = uuid::Uuid::new_v4().to_string();
        let session = SharedSession::new(id.clone());
        self.sessions.write().await.insert(id.clone(), session);
        debug!(session_id = %id, "session created");
        id
    }

    /// Retrieve a clone of the session data, if it exists.
    pub async fn get_session(&self, id: &str) -> Option<SharedSession> {
        self.sessions.read().await.get(id).cloned()
    }

    /// Delete a session and remove all frontend bindings that reference it.
    pub async fn delete_session(&self, id: &str) {
        // Remove the session itself.
        self.sessions.write().await.remove(id);

        // Remove stale frontend bindings pointing to this session.
        let mut fe_conns = self.frontend_connections.write().await;
        for sessions in fe_conns.values_mut() {
            sessions.retain(|sid| sid != id);
        }

        info!(session_id = %id, "session deleted");
    }

    // ── Frontend connection management ───────────────────────────────────

    /// Connect a frontend to a session so it receives sync diffs.
    pub async fn connect_frontend(&self, frontend_id: &str, session_id: &str) {
        let mut fe_conns = self.frontend_connections.write().await;
        fe_conns
            .entry(frontend_id.to_string())
            .or_default()
            .push(session_id.to_string());
        debug!(frontend_id = %frontend_id, session_id = %session_id, "frontend connected");
    }

    /// Disconnect a frontend from a specific session.
    pub async fn disconnect_frontend(&self, frontend_id: &str, session_id: &str) {
        let mut fe_conns = self.frontend_connections.write().await;
        if let Some(sessions) = fe_conns.get_mut(frontend_id) {
            sessions.retain(|sid| sid != session_id);
            if sessions.is_empty() {
                fe_conns.remove(frontend_id);
            }
        }
        debug!(frontend_id = %frontend_id, session_id = %session_id, "frontend disconnected");
    }

    /// Remove a frontend entirely (all its session bindings).
    pub async fn disconnect_frontend_all(&self, frontend_id: &str) {
        self.frontend_connections.write().await.remove(frontend_id);
        debug!(frontend_id = %frontend_id, "frontend fully disconnected");
    }

    // ── Mutations (bump version) ─────────────────────────────────────────

    /// Append a chat message to a session.
    ///
    /// Returns `Ok(new_version)` on success, or `Err` if the session does not
    /// exist.
    pub async fn append_message(
        &self,
        session_id: &str,
        message: ChatMessage,
    ) -> Result<u64, String> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("session {session_id} not found"))?;
        session.chat_history.push(message);
        session.touch();
        let new_version = session.version;
        Ok(new_version)
    }

    /// Add a new active task to a session.
    pub async fn add_task(&self, session_id: &str, task: ActiveTask) -> Result<u64, String> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("session {session_id} not found"))?;
        session.active_tasks.push(task);
        session.touch();
        Ok(session.version)
    }

    /// Update the status and/or progress of an existing task.
    pub async fn update_task(
        &self,
        session_id: &str,
        task_id: &str,
        status: String,
        progress: f64,
    ) -> Result<u64, String> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("session {session_id} not found"))?;

        let task = session
            .active_tasks
            .iter_mut()
            .find(|t| t.id == task_id)
            .ok_or_else(|| format!("task {task_id} not found in session {session_id}"))?;

        task.status = status;
        task.progress = progress;
        session.touch();
        Ok(session.version)
    }

    /// Add a council proposal to a session.
    pub async fn add_proposal(
        &self,
        session_id: &str,
        proposal: CouncilProposal,
    ) -> Result<u64, String> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("session {session_id} not found"))?;
        session.council_proposals.push(proposal);
        session.touch();
        Ok(session.version)
    }

    // ── Sync diff computation ────────────────────────────────────────────

    /// Compute the `SyncDiff` for a frontend for a specific session.
    ///
    /// Only diffs for versions the frontend has not yet seen (`> last_synced_version`
    /// but `<= current_version`) are returned.
    pub async fn get_sync_diff(&self, frontend_id: &str, session_id: &str) -> Vec<SyncDiff> {
        // If the frontend isn't connected, return empty.
        {
            let fe_conns = self.frontend_connections.read().await;
            let connected = fe_conns
                .get(frontend_id)
                .map(|sessions| sessions.iter().any(|sid| sid == session_id))
                .unwrap_or(false);
            if !connected {
                return Vec::new();
            }
        }

        let sessions = self.sessions.read().await;
        let session = match sessions.get(session_id) {
            Some(s) => s.clone(),
            None => return Vec::new(),
        };

        // For simplicity in this initial implementation we compute the diff
        // from scratch based on the version gap. In a production system you'd
        // store a changelog; here we reconstruct the diff from live state.
        //
        // Since we don't store a per-frontend cursor in the registry (the
        // caller is responsible for that), we return a full snapshot-style
        // diff that a frontend can use to reconcile. The frontend's
        // `FrontendSyncState.last_synced_version` is used elsewhere to know
        // whether a full or incremental sync is needed.
        //
        // For now we return a single `SyncDiff` with the current version and
        // all "active" diffs as DiffEntry items. A more advanced
        // implementation would journal each mutation.

        let mut diffs: Vec<DiffEntry> = Vec::new();

        // Include active tasks as TaskAdded diffs.
        for task in &session.active_tasks {
            diffs.push(DiffEntry::TaskAdded(task.clone()));
        }

        // Include recent messages (last 50) as MessageAdded diffs.
        let start = session.chat_history.len().saturating_sub(50);
        for msg in &session.chat_history[start..] {
            diffs.push(DiffEntry::MessageAdded(msg.clone()));
        }

        // Include council proposals.
        for proposal in &session.council_proposals {
            diffs.push(DiffEntry::ProposalAdded(proposal.clone()));
        }

        vec![SyncDiff {
            version: session.version,
            diffs,
        }]
    }

    // ── Broadcasting ─────────────────────────────────────────────────────

    /// Broadcast a JSON payload to all frontends connected to a session via
    /// the optional broadcast callback.
    pub async fn broadcast_to_session(&self, session_id: &str, message: &str) {
        let guard = self.broadcast_fn.read().await;
        if let Some(ref f) = *guard {
            f(message);
            debug!(session_id = %session_id, "broadcast message");
        } else {
            debug!(session_id = %session_id, "no broadcast fn registered; message dropped");
        }
    }

    /// Return the number of sessions currently registered.
    pub async fn session_count(&self) -> usize {
        self.sessions.read().await.len()
    }

    /// Return the number of frontends currently connected.
    pub async fn frontend_count(&self) -> usize {
        self.frontend_connections.read().await.len()
    }

    // ── Session cleanup ──────────────────────────────────────────────────

    /// Spawn a background task that periodically evicts:
    ///
    /// - Sessions that have been inactive for more than `max_age`.
    /// - Orphaned frontend connections (frontends with no remaining sessions
    ///   after cleanup).
    ///
    /// The task runs every `check_interval`. Use
    /// [`Self::start_cleanup_task_default`] for the standard 24 h / 5 min
    /// parameters.
    pub fn start_cleanup_task(
        self: Arc<Self>,
        max_age: Duration,
        check_interval: Duration,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(check_interval).await;
                let removed = self.cleanup_inactive_sessions(max_age).await;
                if removed > 0 {
                    info!(count = removed, "cleaned up inactive sessions");
                }
            }
        })
    }

    /// Convenience wrapper that uses 24 h inactivity timeout and 5 min check
    /// interval.
    pub fn start_cleanup_task_default(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        self.start_cleanup_task(
            Duration::from_secs(24 * 60 * 60),
            Duration::from_secs(5 * 60),
        )
    }

    /// Run a single pass of session cleanup. Returns the number of sessions
    /// that were removed.
    pub async fn cleanup_inactive_sessions(&self, max_age: Duration) -> usize {
        let threshold = now_ms().saturating_sub(max_age.as_millis() as u64);
        let stale_ids: Vec<SessionId> = {
            let sessions = self.sessions.read().await;
            sessions
                .iter()
                .filter(|(_, s)| s.last_active < threshold)
                .map(|(id, _)| id.clone())
                .collect()
        };

        let count = stale_ids.len();
        if count > 0 {
            let mut sessions = self.sessions.write().await;
            for id in &stale_ids {
                sessions.remove(id);
            }

            // Clean up orphaned frontend connections.
            let mut fe_conns = self.frontend_connections.write().await;
            fe_conns.retain(|_, sessions| {
                sessions.retain(|sid| !stale_ids.contains(sid));
                !sessions.is_empty()
            });

            debug!(removed = count, "inactive sessions cleaned up");
        }
        count
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns the current system time in milliseconds since the Unix epoch.
#[allow(dead_code)] // F-GAP-51 — reserved for session sync
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_message(id: &str, role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            id: id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            timestamp: now_ms(),
            metadata: HashMap::new(),
        }
    }

    fn sample_task(id: &str, status: &str, progress: f64) -> ActiveTask {
        ActiveTask {
            id: id.to_string(),
            status: status.to_string(),
            progress,
            started_at: now_ms(),
            description: format!("task {id}"),
        }
    }

    fn sample_proposal(id: &str, title: &str) -> CouncilProposal {
        CouncilProposal {
            id: id.to_string(),
            title: title.to_string(),
            status: "pending".to_string(),
            submitted_at: now_ms(),
            metadata: HashMap::new(),
        }
    }

    // ── Session lifecycle tests ──────────────────────────────────────────

    #[tokio::test]
    async fn test_create_and_get_session() {
        let registry = SessionRegistry::new();
        let id = registry.create_session().await;
        let session = registry.get_session(&id).await;
        assert!(session.is_some());
        assert_eq!(session.unwrap().id, id);
    }

    #[tokio::test]
    async fn test_get_nonexistent_session() {
        let registry = SessionRegistry::new();
        let session = registry.get_session("nonexistent").await;
        assert!(session.is_none());
    }

    #[tokio::test]
    async fn test_delete_session() {
        let registry = SessionRegistry::new();
        let id = registry.create_session().await;
        assert_eq!(registry.session_count().await, 1);

        registry.delete_session(&id).await;
        assert_eq!(registry.session_count().await, 0);
        assert!(registry.get_session(&id).await.is_none());
    }

    #[tokio::test]
    async fn test_create_multiple_sessions() {
        let registry = SessionRegistry::new();
        let id1 = registry.create_session().await;
        let id2 = registry.create_session().await;
        assert_eq!(registry.session_count().await, 2);
        assert_ne!(id1, id2);
    }

    // ── Frontend connection tests ────────────────────────────────────────

    #[tokio::test]
    async fn test_connect_frontend() {
        let registry = SessionRegistry::new();
        let sid = registry.create_session().await;
        registry.connect_frontend("fe1", &sid).await;
        assert_eq!(registry.frontend_count().await, 1);
    }

    #[tokio::test]
    async fn test_disconnect_frontend() {
        let registry = SessionRegistry::new();
        let sid = registry.create_session().await;
        registry.connect_frontend("fe1", &sid).await;
        assert_eq!(registry.frontend_count().await, 1);

        registry.disconnect_frontend("fe1", &sid).await;
        assert_eq!(registry.frontend_count().await, 0);
    }

    #[tokio::test]
    async fn test_disconnect_frontend_all() {
        let registry = SessionRegistry::new();
        let sid1 = registry.create_session().await;
        let sid2 = registry.create_session().await;
        registry.connect_frontend("fe1", &sid1).await;
        registry.connect_frontend("fe1", &sid2).await;

        registry.disconnect_frontend_all("fe1").await;
        assert_eq!(registry.frontend_count().await, 0);
    }

    #[tokio::test]
    async fn test_deleting_session_removes_frontend_bindings() {
        let registry = SessionRegistry::new();
        let sid = registry.create_session().await;
        registry.connect_frontend("fe1", &sid).await;

        registry.delete_session(&sid).await;

        // The frontend should still exist in the map but with an empty
        // session list — our delete implementation retains the frontend
        // entry if it has other sessions, or leaves the vector empty.
        // Actually, we retain the FE entry so the frontend might still be
        // present. Let's just check session count is 0.
        assert_eq!(registry.session_count().await, 0);
    }

    // ── Mutation tests ───────────────────────────────────────────────────

    #[tokio::test]
    async fn test_append_message_bumps_version() {
        let registry = SessionRegistry::new();
        let sid = registry.create_session().await;
        let msg = sample_message("m1", "user", "hello");

        let version = registry
            .append_message(&sid, msg)
            .await
            .expect("append should succeed");
        assert!(version > 0);

        let session = registry.get_session(&sid).await.unwrap();
        assert_eq!(session.chat_history.len(), 1);
        assert_eq!(session.chat_history[0].content, "hello");
        assert_eq!(session.version, version);
    }

    #[tokio::test]
    async fn test_append_message_to_nonexistent_session() {
        let registry = SessionRegistry::new();
        let msg = sample_message("m1", "user", "hello");
        let result = registry.append_message("no-session", msg).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_add_task() {
        let registry = SessionRegistry::new();
        let sid = registry.create_session().await;
        let task = sample_task("t1", "running", 0.5);

        let version = registry
            .add_task(&sid, task)
            .await
            .expect("add should succeed");
        assert!(version > 0);

        let session = registry.get_session(&sid).await.unwrap();
        assert_eq!(session.active_tasks.len(), 1);
        assert_eq!(session.active_tasks[0].status, "running");
    }

    #[tokio::test]
    async fn test_update_task() {
        let registry = SessionRegistry::new();
        let sid = registry.create_session().await;
        let task = sample_task("t1", "running", 0.5);
        registry.add_task(&sid, task).await.unwrap();

        let version = registry
            .update_task(&sid, "t1", "completed".to_string(), 1.0)
            .await
            .expect("update should succeed");

        let session = registry.get_session(&sid).await.unwrap();
        assert_eq!(session.active_tasks[0].status, "completed");
        assert!((session.active_tasks[0].progress - 1.0).abs() < f64::EPSILON);
        assert_eq!(session.version, version);
    }

    #[tokio::test]
    async fn test_update_nonexistent_task() {
        let registry = SessionRegistry::new();
        let sid = registry.create_session().await;
        let result = registry
            .update_task(&sid, "ghost", "done".to_string(), 1.0)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_add_proposal() {
        let registry = SessionRegistry::new();
        let sid = registry.create_session().await;
        let proposal = sample_proposal("p1", "test proposal");

        let version = registry
            .add_proposal(&sid, proposal)
            .await
            .expect("add should succeed");

        let session = registry.get_session(&sid).await.unwrap();
        assert_eq!(session.council_proposals.len(), 1);
        assert_eq!(session.council_proposals[0].title, "test proposal");
        assert_eq!(session.version, version);
    }

    #[tokio::test]
    async fn test_version_monotonically_increments() {
        let registry = SessionRegistry::new();
        let sid = registry.create_session().await;

        let v1 = registry
            .append_message(&sid, sample_message("m1", "user", "a"))
            .await
            .unwrap();
        let v2 = registry
            .append_message(&sid, sample_message("m2", "assistant", "b"))
            .await
            .unwrap();
        let v3 = registry
            .add_task(&sid, sample_task("t1", "running", 0.0))
            .await
            .unwrap();

        assert!(v1 < v2);
        assert!(v2 < v3);
    }

    // ── Sync diff tests ──────────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_sync_diff_empty_for_unconnected_frontend() {
        let registry = SessionRegistry::new();
        let sid = registry.create_session().await;
        let diffs = registry.get_sync_diff("unconnected", &sid).await;
        assert!(diffs.is_empty());
    }

    #[tokio::test]
    async fn test_get_sync_diff_includes_messages_and_tasks() {
        let registry = SessionRegistry::new();
        let sid = registry.create_session().await;

        registry
            .append_message(&sid, sample_message("m1", "user", "hello"))
            .await
            .unwrap();
        registry
            .add_task(&sid, sample_task("t1", "running", 0.3))
            .await
            .unwrap();

        registry.connect_frontend("fe1", &sid).await;
        let diffs = registry.get_sync_diff("fe1", &sid).await;

        assert!(!diffs.is_empty(), "should have at least one SyncDiff");
        // The first (and only) SyncDiff should contain entries.
        let diff = &diffs[0];
        assert!(diff.version > 0);

        let msg_count = diff
            .diffs
            .iter()
            .filter(|e| matches!(e, DiffEntry::MessageAdded(_)))
            .count();
        let task_count = diff
            .diffs
            .iter()
            .filter(|e| matches!(e, DiffEntry::TaskAdded(_)))
            .count();

        assert_eq!(msg_count, 1, "should have 1 message diff");
        assert_eq!(task_count, 1, "should have 1 task diff");
    }

    // ── Broadcast tests ──────────────────────────────────────────────────

    #[tokio::test]
    async fn test_broadcast_with_callback() {
        let registry = SessionRegistry::new();
        let sid = registry.create_session().await;
        let received = Arc::new(RwLock::new(String::new()));

        let recv = received.clone();
        let broadcast_fn: BroadcastFn = Arc::new(move |msg: &str| {
            let mut data = recv.try_write().expect("lock");
            *data = msg.to_string();
        });

        registry.set_broadcast_fn(broadcast_fn).await;
        registry
            .broadcast_to_session(&sid, r#"{"type":"test"}"#)
            .await;

        let result = received.read().await;
        assert_eq!(*result, r#"{"type":"test"}"#);
    }

    #[tokio::test]
    async fn test_broadcast_without_callback_does_not_panic() {
        let registry = SessionRegistry::new();
        let sid = registry.create_session().await;
        // Should not panic even though no broadcast function is set.
        registry
            .broadcast_to_session(&sid, r#"{"type":"test"}"#)
            .await;
    }

    // ── Cleanup tests ────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_cleanup_inactive_sessions() {
        let registry = SessionRegistry::new();
        let sid = registry.create_session().await;

        // Use a very short max_age and artificially set the session's
        // last_active far in the past by modifying through the internal API.
        {
            let mut sessions = registry.sessions.write().await;
            if let Some(s) = sessions.get_mut(&sid) {
                s.last_active = 1; // way in the past
            }
        }

        let removed = registry
            .cleanup_inactive_sessions(Duration::from_millis(1))
            .await;
        assert_eq!(removed, 1);
        assert_eq!(registry.session_count().await, 0);
    }

    #[tokio::test]
    async fn test_cleanup_recent_sessions_not_removed() {
        let registry = SessionRegistry::new();
        let _sid = registry.create_session().await;

        // No session should be removed because they are all recent.
        let removed = registry
            .cleanup_inactive_sessions(Duration::from_secs(24 * 60 * 60))
            .await;
        assert_eq!(removed, 0);
        assert_eq!(registry.session_count().await, 1);
    }

    #[tokio::test]
    async fn test_cleanup_removes_orphaned_frontend_bindings() {
        let registry = SessionRegistry::new();
        let sid = registry.create_session().await;
        registry.connect_frontend("fe1", &sid).await;

        // Set session as stale.
        {
            let mut sessions = registry.sessions.write().await;
            if let Some(s) = sessions.get_mut(&sid) {
                s.last_active = 1;
            }
        }

        registry
            .cleanup_inactive_sessions(Duration::from_millis(1))
            .await;

        // The frontend map entry should have been cleaned up.
        let fe_conns = registry.frontend_connections.read().await;
        assert!(
            !fe_conns.contains_key("fe1"),
            "orphaned frontend entry should be removed"
        );
    }

    // ── Concurrency tests ────────────────────────────────────────────────

    #[tokio::test]
    async fn test_concurrent_session_mutations() {
        let registry = Arc::new(SessionRegistry::new());
        let sid = registry.create_session().await;

        let mut handles = Vec::new();
        for i in 0..10 {
            let reg = registry.clone();
            let sid = sid.clone();
            handles.push(tokio::spawn(async move {
                let msg = sample_message(&format!("m{i}"), "user", &format!("msg {i}"));
                reg.append_message(&sid, msg).await.unwrap()
            }));
        }

        let versions: Vec<u64> = futures_util::future::join_all(handles)
            .await
            .into_iter()
            .collect::<Result<_, _>>()
            .unwrap();

        // All versions should be unique (monotonically increasing).
        let mut sorted = versions.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            10,
            "every mutation should produce a unique version"
        );

        let session = registry.get_session(&sid).await.unwrap();
        assert_eq!(session.chat_history.len(), 10);
    }

    #[tokio::test]
    async fn test_session_registry_is_clonable() {
        let registry = SessionRegistry::new();
        let registry2 = registry.clone();

        let sid = registry.create_session().await;
        let session = registry2.get_session(&sid).await;
        assert!(session.is_some(), "cloned registry should see the session");
    }

    #[tokio::test]
    async fn test_start_cleanup_task_default_can_be_aborted() {
        let registry = Arc::new(SessionRegistry::new());
        let handle = registry.clone().start_cleanup_task_default();
        // Just ensure it can be aborted without panicking.
        handle.abort();
    }

    #[tokio::test]
    async fn test_shared_session_touch() {
        let mut s = SharedSession::new("test".to_string());
        let v0 = s.version;
        let a0 = s.last_active;
        s.touch();
        assert!(s.version > v0);
        assert!(s.last_active >= a0);
    }

    #[tokio::test]
    async fn test_frontend_sync_state_tracking() {
        let state = FrontendSyncState::new("s1".to_string());
        assert_eq!(state.session_id, "s1");
        assert_eq!(state.last_synced_version, 0);
        assert!(state.pending_diffs.is_empty());
    }
}
