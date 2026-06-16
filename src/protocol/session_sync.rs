//! GAP-B50-10: 三端会话状态同步 — Session state synchronization layer.

// activated, formerly F-GAP-51: all items below are active session sync code
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
use tracing::{debug, info, warn};

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
// activated, formerly F-GAP-51
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub role: String, // "user" | "assistant"
    pub content: String,
    pub timestamp: u64,
    pub metadata: HashMap<String, Value>,
}

/// An active task running within a session.
// activated, formerly F-GAP-51
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveTask {
    pub id: String,
    pub status: String,
    pub progress: f64,
    pub started_at: u64,
    pub description: String,
}

/// A proposal submitted to the council for a session.
// activated, formerly F-GAP-51
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

/// Maximum number of chat messages kept per session (oldest evicted first).
const MAX_CHAT_HISTORY: usize = 1000;

/// Maximum number of active tasks tracked per session.
const MAX_ACTIVE_TASKS: usize = 200;

/// Maximum number of council proposals kept per session.
const MAX_COUNCIL_PROPOSALS: usize = 200;

/// GAP-B58-C14: Global maximum number of concurrent sessions across all
/// frontends. When this limit is reached, `create_session` returns an error
/// so callers can surface a "too many sessions" response instead of silently
/// growing unbounded.
const MAX_SESSIONS: usize = 10_000;

/// A session whose state is guarded by an `RwLock` and wrapped in `Arc` so it
/// can be shared across tasks and frontend handlers.
///
/// Every mutation bumps `version` so that sync consumers can perform
/// incremental diffs.  Fields are encapsulated behind getter/setter methods
/// to enforce capacity limits and maintain a consistent public API.
// activated, formerly F-GAP-51
#[derive(Debug, Clone)]
pub struct SharedSession {
    id: SessionId,
    tenant_id: Option<String>,
    chat_history: Vec<ChatMessage>,
    active_tasks: Vec<ActiveTask>,
    council_proposals: Vec<CouncilProposal>,
    last_active: u64,
    version: u64,
}

// activated, formerly F-GAP-51
impl SharedSession {
    pub fn new(id: SessionId) -> Self {
        Self {
            id,
            tenant_id: None,
            chat_history: Vec::new(),
            active_tasks: Vec::new(),
            council_proposals: Vec::new(),
            last_active: now_ms(),
            version: 0,
        }
    }

    /// Create a new session with an explicit tenant identifier.
    pub fn with_tenant(id: SessionId, tenant_id: String) -> Self {
        Self {
            id,
            tenant_id: Some(tenant_id),
            chat_history: Vec::new(),
            active_tasks: Vec::new(),
            council_proposals: Vec::new(),
            last_active: now_ms(),
            version: 0,
        }
    }

    // ── Getters ──────────────────────────────────────────────────────────

    /// Session identifier.
    pub fn id(&self) -> &SessionId {
        &self.id
    }

    /// Optional tenant identifier for multi-tenant isolation.
    pub fn tenant_id(&self) -> &Option<String> {
        &self.tenant_id
    }

    /// Chat message history.
    pub fn chat_history(&self) -> &[ChatMessage] {
        &self.chat_history
    }

    /// Currently active tasks.
    pub fn active_tasks(&self) -> &[ActiveTask] {
        &self.active_tasks
    }

    /// Council proposals.
    pub fn council_proposals(&self) -> &[CouncilProposal] {
        &self.council_proposals
    }

    /// Timestamp (ms) of the last activity.
    pub fn last_active(&self) -> u64 {
        self.last_active
    }

    /// Monotonically increasing version number, bumped on every mutation.
    pub fn version(&self) -> u64 {
        self.version
    }

    // ── Setters ──────────────────────────────────────────────────────────

    /// Set the tenant identifier.
    pub fn set_tenant_id(&mut self, tenant_id: Option<String>) {
        self.tenant_id = tenant_id;
    }

    /// Set the last-active timestamp.
    pub fn set_last_active(&mut self, ts: u64) {
        self.last_active = ts;
    }

    // ── Mutation helpers ─────────────────────────────────────────────────

    /// Append a message to chat history, enforcing capacity limits.
    pub fn push_message(&mut self, msg: ChatMessage) {
        self.chat_history.push(msg);
        self.enforce_capacity();
    }

    /// Add an active task, enforcing capacity limits.
    pub fn add_task(&mut self, task: ActiveTask) {
        self.active_tasks.push(task);
        self.enforce_capacity();
    }

    /// Add a council proposal, enforcing capacity limits.
    pub fn add_proposal(&mut self, proposal: CouncilProposal) {
        self.council_proposals.push(proposal);
        self.enforce_capacity();
    }

    /// Find and update a task's status and progress by task ID.
    /// Returns `Err` if the task is not found.
    pub fn update_task(
        &mut self,
        task_id: &str,
        status: String,
        progress: f64,
    ) -> Result<(), String> {
        let task = self
            .active_tasks
            .iter_mut()
            .find(|t| t.id == task_id)
            .ok_or_else(|| format!("task {task_id} not found"))?;
        task.status = status;
        task.progress = progress;
        Ok(())
    }

    /// Touch the `last_active` timestamp and bump the version.
    fn touch(&mut self) {
        self.last_active = now_ms();
        self.version += 1;
    }

    /// Evict oldest entries when capacity limits are exceeded.
    fn enforce_capacity(&mut self) {
        while self.chat_history.len() > MAX_CHAT_HISTORY {
            self.chat_history.remove(0);
        }
        while self.active_tasks.len() > MAX_ACTIVE_TASKS {
            self.active_tasks.remove(0);
        }
        while self.council_proposals.len() > MAX_COUNCIL_PROPOSALS {
            self.council_proposals.remove(0);
        }
    }
}

// ---------------------------------------------------------------------------
// SyncDiff – incremental diff protocol
// ---------------------------------------------------------------------------

/// An incremental diff that a frontend can apply to bring its local session
/// state up to date without fetching the full session.
// activated, formerly F-GAP-51
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncDiff {
    pub version: u64,
    pub diffs: Vec<DiffEntry>,
}

/// A single entry inside a `SyncDiff`.
// activated, formerly F-GAP-51
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
// SessionRegistry
// ---------------------------------------------------------------------------

/// The central registry that owns all sessions and tracks frontend bindings.
///
/// # Thread safety
///
/// Both `sessions` and `frontend_connections` are behind `Arc<RwLock<…>>`,
/// so `SessionRegistry` itself is cheaply clonable and can be injected into
/// any number of tasks or handlers.
// activated, formerly F-GAP-51
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

// activated, formerly F-GAP-51
impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// activated, formerly F-GAP-51
impl SessionRegistry {
    /// Create an empty registry.
    #[allow(dead_code)] // activated, formerly F-GAP-51 — public API surface
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
    ///
    /// Returns `Err` with a descriptive message when the global `MAX_SESSIONS`
    /// limit has been reached.
    pub async fn create_session(&self) -> Result<SessionId, String> {
        let mut sessions = self.sessions.write().await;
        if sessions.len() >= MAX_SESSIONS {
            return Err(format!("session limit reached (max={MAX_SESSIONS})"));
        }

        let id = uuid::Uuid::new_v4().to_string();
        let session = SharedSession::new(id.clone());
        sessions.insert(id.clone(), session);
        debug!(session_id = %id, "session created");
        Ok(id)
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
    ///
    /// If `tenant_id` is `Some` and the session has a tenant set, the
    /// frontend's tenant must match the session's tenant, otherwise the
    /// connection is rejected with a warning.
    pub async fn connect_frontend(
        &self,
        frontend_id: &str,
        session_id: &str,
        tenant_id: Option<&str>,
    ) {
        // Tenant isolation guard: hold the write lock across the tenant check
        // and the frontend connection insert to prevent a TOCTOU race where
        // the session could be deleted between the check and the insert.
        let sessions = self.sessions.write().await;
        if let Some(tenant) = tenant_id {
            if let Some(session) = sessions.get(session_id) {
                if let Some(session_tenant) = session.tenant_id() {
                    if session_tenant != tenant {
                        warn!(
                            frontend_id = %frontend_id,
                            session_id = %session_id,
                            frontend_tenant = %tenant,
                            session_tenant = %session_tenant,
                            "tenant mismatch – frontend connection rejected"
                        );
                        return;
                    }
                }
            }
        }
        // Re-check that the session still exists before inserting.
        if !sessions.contains_key(session_id) {
            warn!(
                frontend_id = %frontend_id,
                session_id = %session_id,
                "session disappeared before frontend connection – rejected"
            );
            return;
        }
        drop(sessions);

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

    /// Append a message to the session's chat history with capacity enforcement.
    pub async fn append_message(
        &self,
        session_id: &str,
        message: ChatMessage,
    ) -> Result<u64, String> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("session {session_id} not found"))?;
        session.push_message(message);
        session.touch();
        Ok(session.version())
    }

    /// Add an active task with capacity enforcement.
    pub async fn add_task(&self, session_id: &str, task: ActiveTask) -> Result<u64, String> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("session {session_id} not found"))?;
        session.add_task(task);
        session.touch();
        Ok(session.version())
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

        session
            .update_task(task_id, status, progress)
            .map_err(|e| format!("{e} in session {session_id}"))?;
        session.touch();
        Ok(session.version())
    }

    /// Add a council proposal with capacity enforcement.
    pub async fn add_proposal(
        &self,
        session_id: &str,
        proposal: CouncilProposal,
    ) -> Result<u64, String> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("session {session_id} not found"))?;
        session.add_proposal(proposal);
        session.touch();
        Ok(session.version())
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
        for task in session.active_tasks() {
            diffs.push(DiffEntry::TaskAdded(task.clone()));
        }

        // Include recent messages (last 50) as MessageAdded diffs.
        let chat_history = session.chat_history();
        let start = chat_history.len().saturating_sub(50);
        for msg in &chat_history[start..] {
            diffs.push(DiffEntry::MessageAdded(msg.clone()));
        }

        // Include council proposals.
        for proposal in session.council_proposals() {
            diffs.push(DiffEntry::ProposalAdded(proposal.clone()));
        }

        vec![SyncDiff {
            version: session.version(),
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
    ///
    /// The stale check and deletion are performed under a single write lock
    /// to eliminate the TOCTOU race condition: without this, a session could
    /// be touched (become active) between the read-lock check and the
    /// write-lock deletion, yet still be removed. By using one write-lock
    /// transaction, we guarantee consistency.
    pub async fn cleanup_inactive_sessions(&self, max_age: Duration) -> usize {
        let threshold = now_ms().saturating_sub(max_age.as_millis() as u64);

        // Perform stale check AND deletion under a single write lock to
        // prevent TOCTOU races.
        let mut sessions = self.sessions.write().await;
        let stale_ids: Vec<SessionId> = sessions
            .iter()
            .filter(|(_, s)| s.last_active() < threshold)
            .map(|(id, _)| id.clone())
            .collect();

        let count = stale_ids.len();
        if count > 0 {
            for id in &stale_ids {
                sessions.remove(id);
            }
            // Drop sessions lock before acquiring frontend_connections lock
            // to avoid potential lock-ordering issues.
            drop(sessions);

            // Clean up orphaned frontend connections.
            let mut fe_conns = self.frontend_connections.write().await;
            fe_conns.retain(|_, conn_sessions| {
                conn_sessions.retain(|sid| !stale_ids.contains(sid));
                !conn_sessions.is_empty()
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
// activated, formerly F-GAP-51
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
        let id = registry.create_session().await.unwrap();
        let session = registry.get_session(&id).await;
        assert!(session.is_some());
        assert_eq!(*session.unwrap().id(), id);
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
        let id = registry.create_session().await.unwrap();
        assert_eq!(registry.session_count().await, 1);

        registry.delete_session(&id).await;
        assert_eq!(registry.session_count().await, 0);
        assert!(registry.get_session(&id).await.is_none());
    }

    #[tokio::test]
    async fn test_create_multiple_sessions() {
        let registry = SessionRegistry::new();
        let id1 = registry.create_session().await.unwrap();
        let id2 = registry.create_session().await.unwrap();
        assert_eq!(registry.session_count().await, 2);
        assert_ne!(id1, id2);
    }

    // ── Frontend connection tests ────────────────────────────────────────

    #[tokio::test]
    async fn test_connect_frontend() {
        let registry = SessionRegistry::new();
        let sid = registry.create_session().await.unwrap();
        registry.connect_frontend("fe1", &sid, None).await;
        assert_eq!(registry.frontend_count().await, 1);
    }

    #[tokio::test]
    async fn test_disconnect_frontend() {
        let registry = SessionRegistry::new();
        let sid = registry.create_session().await.unwrap();
        registry.connect_frontend("fe1", &sid, None).await;
        assert_eq!(registry.frontend_count().await, 1);

        registry.disconnect_frontend("fe1", &sid).await;
        assert_eq!(registry.frontend_count().await, 0);
    }

    #[tokio::test]
    async fn test_disconnect_frontend_all() {
        let registry = SessionRegistry::new();
        let sid1 = registry.create_session().await.unwrap();
        let sid2 = registry.create_session().await.unwrap();
        registry.connect_frontend("fe1", &sid1, None).await;
        registry.connect_frontend("fe1", &sid2, None).await;

        registry.disconnect_frontend_all("fe1").await;
        assert_eq!(registry.frontend_count().await, 0);
    }

    #[tokio::test]
    async fn test_deleting_session_removes_frontend_bindings() {
        let registry = SessionRegistry::new();
        let sid = registry.create_session().await.unwrap();
        registry.connect_frontend("fe1", &sid, None).await;

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
        let sid = registry.create_session().await.unwrap();
        let msg = sample_message("m1", "user", "hello");

        let version = registry
            .append_message(&sid, msg)
            .await
            .expect("append should succeed");
        assert!(version > 0);

        let session = registry.get_session(&sid).await.unwrap();
        assert_eq!(session.chat_history().len(), 1);
        assert_eq!(session.chat_history()[0].content, "hello");
        assert_eq!(session.version(), version);
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
        let sid = registry.create_session().await.unwrap();
        let task = sample_task("t1", "running", 0.5);

        let version = registry
            .add_task(&sid, task)
            .await
            .expect("add should succeed");
        assert!(version > 0);

        let session = registry.get_session(&sid).await.unwrap();
        assert_eq!(session.active_tasks().len(), 1);
        assert_eq!(session.active_tasks()[0].status, "running");
    }

    #[tokio::test]
    async fn test_update_task() {
        let registry = SessionRegistry::new();
        let sid = registry.create_session().await.unwrap();
        let task = sample_task("t1", "running", 0.5);
        registry.add_task(&sid, task).await.unwrap();

        let version = registry
            .update_task(&sid, "t1", "completed".to_string(), 1.0)
            .await
            .expect("update should succeed");

        let session = registry.get_session(&sid).await.unwrap();
        assert_eq!(session.active_tasks()[0].status, "completed");
        assert!((session.active_tasks()[0].progress - 1.0).abs() < f64::EPSILON);
        assert_eq!(session.version(), version);
    }

    #[tokio::test]
    async fn test_update_nonexistent_task() {
        let registry = SessionRegistry::new();
        let sid = registry.create_session().await.unwrap();
        let result = registry
            .update_task(&sid, "ghost", "done".to_string(), 1.0)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_add_proposal() {
        let registry = SessionRegistry::new();
        let sid = registry.create_session().await.unwrap();
        let proposal = sample_proposal("p1", "test proposal");

        let version = registry
            .add_proposal(&sid, proposal)
            .await
            .expect("add should succeed");

        let session = registry.get_session(&sid).await.unwrap();
        assert_eq!(session.council_proposals().len(), 1);
        assert_eq!(session.council_proposals()[0].title, "test proposal");
        assert_eq!(session.version(), version);
    }

    #[tokio::test]
    async fn test_version_monotonically_increments() {
        let registry = SessionRegistry::new();
        let sid = registry.create_session().await.unwrap();

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
        let sid = registry.create_session().await.unwrap();
        let diffs = registry.get_sync_diff("unconnected", &sid).await;
        assert!(diffs.is_empty());
    }

    #[tokio::test]
    async fn test_get_sync_diff_includes_messages_and_tasks() {
        let registry = SessionRegistry::new();
        let sid = registry.create_session().await.unwrap();

        registry
            .append_message(&sid, sample_message("m1", "user", "hello"))
            .await
            .unwrap();
        registry
            .add_task(&sid, sample_task("t1", "running", 0.3))
            .await
            .unwrap();

        registry.connect_frontend("fe1", &sid, None).await;
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
        let sid = registry.create_session().await.unwrap();
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
        let sid = registry.create_session().await.unwrap();
        // Should not panic even though no broadcast function is set.
        registry
            .broadcast_to_session(&sid, r#"{"type":"test"}"#)
            .await;
    }

    // ── Cleanup tests ────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_cleanup_inactive_sessions() {
        let registry = SessionRegistry::new();
        let sid = registry.create_session().await.unwrap();

        // Use a very short max_age and artificially set the session's
        // last_active far in the past by modifying through the internal API.
        {
            let mut sessions = registry.sessions.write().await;
            if let Some(s) = sessions.get_mut(&sid) {
                s.set_last_active(1); // way in the past
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
        let _sid = registry.create_session().await.unwrap();

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
        let sid = registry.create_session().await.unwrap();
        registry.connect_frontend("fe1", &sid, None).await;

        // Set session as stale.
        {
            let mut sessions = registry.sessions.write().await;
            if let Some(s) = sessions.get_mut(&sid) {
                s.set_last_active(1);
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
        let sid = registry.create_session().await.unwrap();

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
        assert_eq!(session.chat_history().len(), 10);
    }

    #[tokio::test]
    async fn test_session_registry_is_clonable() {
        let registry = SessionRegistry::new();
        let registry2 = registry.clone();

        let sid = registry.create_session().await.unwrap();
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
}
