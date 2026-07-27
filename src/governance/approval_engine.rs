//! Approval Engine (GAP-B52-19)
//!
//! Manages approval workflows with time-based auto-escalation,
//! multi-level escalation chains, and feedback to the PUA rule engine.

use super::approval_learning::ApprovalPreferenceLearner;
use super::pua::PuaRuleEngine;
use super::security_governor::{AuditEntry, PolicyVerdict, SecurityGovernor};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock as StdRwLock;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum ApprovalError {
    #[error("approval request not found: {0}")]
    NotFound(String),

    #[error("approval request {0} already finalized (status: {1:?})")]
    AlreadyFinalized(String, ApprovalStatus),

    #[error("invalid action: {0}")]
    InvalidAction(String),

    #[error("escalation failed: {0}")]
    EscalationFailed(String),

    #[error("internal error: {0}")]
    Internal(String),
}

// ---------------------------------------------------------------------------
// Risk levels
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl RiskLevel {
    /// Returns the escalation timeout for this risk level.
    /// Low/Medium: 5 min → EscalateToManager,  15 min → AutoDeny
    /// High/Critical: 2 min → EscalateToManager, 10 min → AutoDeny
    pub fn escalate_timeout(&self) -> Duration {
        match self {
            RiskLevel::Low | RiskLevel::Medium => Duration::from_secs(300), // 5 min
            RiskLevel::High | RiskLevel::Critical => Duration::from_secs(120), // 2 min
        }
    }

    /// Returns the auto-deny timeout for this risk level.
    pub fn deny_timeout(&self) -> Duration {
        match self {
            RiskLevel::Low | RiskLevel::Medium => Duration::from_secs(900), // 15 min
            RiskLevel::High | RiskLevel::Critical => Duration::from_secs(600), // 10 min
        }
    }
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ApprovalStatus {
    Pending,
    Approved {
        approver: String,
        comment: String,
        timestamp_ms: u64,
    },
    Rejected {
        approver: String,
        reason: String,
        timestamp_ms: u64,
    },
    EscalatedToManager {
        from_level: RiskLevel,
        timestamp_ms: u64,
    },
    AutoDenied {
        reason: String,
        timestamp_ms: u64,
    },
}

impl ApprovalStatus {
    pub fn is_finalized(&self) -> bool {
        matches!(
            self,
            ApprovalStatus::Approved { .. }
                | ApprovalStatus::Rejected { .. }
                | ApprovalStatus::AutoDenied { .. }
        )
    }
}

// ---------------------------------------------------------------------------
// ApprovalRequest
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: String,
    pub user: String,
    pub action: String,
    pub risk_level: RiskLevel,
    pub context: HashMap<String, String>,
    pub status: ApprovalStatus,
    pub escalated_from: Option<String>,
    pub created_at_ms: u64,
}

impl ApprovalRequest {
    pub fn new(
        user: String,
        action: String,
        risk_level: RiskLevel,
        context: HashMap<String, String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            user,
            action,
            risk_level,
            context,
            status: ApprovalStatus::Pending,
            escalated_from: None,
            created_at_ms: current_timestamp_ms(),
        }
    }

    /// Returns how long this request has been pending (in milliseconds).
    pub fn pending_duration_ms(&self) -> u64 {
        current_timestamp_ms().saturating_sub(self.created_at_ms)
    }
}

// ---------------------------------------------------------------------------
// TimeoutPolicy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeoutPolicy {
    /// Multiplier applied to the risk level's base escalate timeout.
    pub escalate_timeout_multiplier: f64,
    /// Multiplier applied to the risk level's base deny timeout.
    pub deny_timeout_multiplier: f64,
    /// Max number of escalation hops before forced deny.
    pub max_escalation_depth: u32,
    /// Optional callback URL / channel for escalation notifications.
    pub escalation_hook: Option<String>,
}

impl Default for TimeoutPolicy {
    fn default() -> Self {
        Self {
            escalate_timeout_multiplier: 1.0,
            deny_timeout_multiplier: 1.0,
            max_escalation_depth: 3,
            escalation_hook: None,
        }
    }
}

// ---------------------------------------------------------------------------
// EscalationChain
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscalationStep {
    pub level: u32,
    pub approver_role: String,
    pub approver_id: Option<String>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscalationChain {
    pub steps: Vec<EscalationStep>,
    pub current_step: u32,
}

impl EscalationChain {
    pub fn new(steps: Vec<EscalationStep>) -> Self {
        Self {
            steps,
            current_step: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// ApproverResolver trait + ApproverRegistry
// ---------------------------------------------------------------------------

/// Trait for resolving approver role names to approver identities.
///
/// Implementations can look up approver IDs from any source:
/// a directory service, a config file, or static mappings.
pub trait ApproverResolver: Send + Sync {
    /// Resolve an approver role (e.g. "manager", "director") to an approver
    /// identity string (e.g. "alice@example.com"). Return `None` if unknown.
    fn resolve_approver_id(&self, request: &ApprovalRequest) -> Option<String>;
}

/// A registry of multiple [`ApproverResolver`] instances.
///
/// When asked to resolve a role, each registered resolver is tried in
/// registration order until one returns `Some(...)`.
pub struct ApproverRegistry {
    resolvers: Vec<Box<dyn ApproverResolver>>,
}

impl ApproverRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            resolvers: Vec::new(),
        }
    }

    /// Register a new resolver.
    pub fn register(&mut self, resolver: Box<dyn ApproverResolver>) {
        self.resolvers.push(resolver);
    }

    /// Try each resolver in order, returning the first non-`None` result.
    pub fn resolve(&self, request: &ApprovalRequest) -> Option<String> {
        for resolver in &self.resolvers {
            if let Some(id) = resolver.resolve_approver_id(request) {
                return Some(id);
            }
        }
        None
    }
}

impl Default for ApproverRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// ApprovalEngine
// ---------------------------------------------------------------------------

pub struct ApprovalEngine {
    queue: Vec<ApprovalRequest>,
    escalation_chains: HashMap<String, EscalationChain>,
    timeout_policy: TimeoutPolicy,
    pua_engine: Arc<Mutex<PuaRuleEngine>>,
    /// Optional preference learner that records decisions for auto-approval analysis.
    learner: Option<Arc<StdRwLock<ApprovalPreferenceLearner>>>,
    /// Registry of approver ID resolvers.
    /// Called when building escalation chains to fill `approver_id`.
    approver_registry: ApproverRegistry,
    /// Optional SQLite database path for persistent approval queue.
    db_path: Option<String>,
    /// Optional SecurityGovernor for audit logging.
    security_governor: Option<Arc<SecurityGovernor>>,
}

impl std::fmt::Debug for ApprovalEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApprovalEngine")
            .field("queue", &self.queue)
            .field("escalation_chains", &self.escalation_chains)
            .field("timeout_policy", &self.timeout_policy)
            .field("pua_engine", &self.pua_engine)
            .field("learner", &self.learner)
            .field("approver_registry", &"<ApproverRegistry>")
            .field("db_path", &self.db_path)
            .field(
                "security_governor",
                &self
                    .security_governor
                    .as_ref()
                    .map(|_| "<SecurityGovernor>"),
            )
            .finish()
    }
}

impl ApprovalEngine {
    /// Create a new ApprovalEngine with the given PUA rule engine and timeout policy.
    pub fn new(pua_engine: Arc<Mutex<PuaRuleEngine>>, timeout_policy: TimeoutPolicy) -> Self {
        Self {
            queue: Vec::new(),
            escalation_chains: HashMap::new(),
            timeout_policy,
            pua_engine,
            learner: None,
            approver_registry: ApproverRegistry::new(),
            db_path: None,
            security_governor: None,
        }
    }

    /// Set the SQLite database path for persistent approval queue storage.
    /// When set, the engine will create the `approval_queue` table on init
    /// and reload any pending approvals from the database.
    #[allow(
        unused_mut,
        reason = "needed by backend-sqlite, unused by backend-postgres"
    )]
    pub fn with_db_path(mut self, db_path: String) -> Self {
        #[cfg(feature = "backend-sqlite")]
        {
            if let Err(e) = Self::init_sqlite(&db_path) {
                tracing::warn!(error = %e, db_path = %db_path, "Failed to initialize SQLite approval queue");
            } else if let Ok(requests) = Self::load_pending_from_sqlite(&db_path) {
                tracing::info!(
                    count = requests.len(),
                    "Reloaded pending approvals from SQLite"
                );
                self.queue = requests;
            }
            self.db_path = Some(db_path);
        }
        #[cfg(not(feature = "backend-sqlite"))]
        {
            tracing::warn!(
                "ApprovalEngine: db_path '{}' set but ignored without backend-sqlite feature",
                db_path
            );
        }
        self
    }

    // ── SQLite helpers (gated behind backend-sqlite) ────────────────────────

    /// Create the `approval_queue` table if it doesn't exist.
    #[cfg(feature = "backend-sqlite")]
    fn init_sqlite(db_path: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let conn = rusqlite::Connection::open(db_path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS approval_queue (
                id TEXT PRIMARY KEY,
                data TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending'
            );",
        )?;
        Ok(())
    }

    /// Load all pending (non-finalized) approval requests from SQLite.
    #[cfg(feature = "backend-sqlite")]
    fn load_pending_from_sqlite(
        db_path: &str,
    ) -> Result<Vec<ApprovalRequest>, Box<dyn std::error::Error + Send + Sync>> {
        let conn = rusqlite::Connection::open(db_path)?;
        let mut stmt = conn.prepare("SELECT data FROM approval_queue WHERE status = 'pending'")?;
        let rows = stmt.query_map([], |row| {
            let data: String = row.get(0)?;
            Ok(data)
        })?;

        let mut requests = Vec::new();
        for row in rows {
            let data = row?;
            if let Ok(req) = serde_json::from_str::<ApprovalRequest>(&data) {
                requests.push(req);
            }
        }
        Ok(requests)
    }

    /// Insert or update an approval request in SQLite.
    #[cfg(feature = "backend-sqlite")]
    fn upsert_sqlite(
        db_path: &str,
        request: &ApprovalRequest,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let conn = rusqlite::Connection::open(db_path)?;
        let data = serde_json::to_string(request)?;
        let status = match &request.status {
            ApprovalStatus::Pending => "pending",
            ApprovalStatus::Approved { .. } => "approved",
            ApprovalStatus::Rejected { .. } => "rejected",
            ApprovalStatus::EscalatedToManager { .. } => "escalated",
            ApprovalStatus::AutoDenied { .. } => "auto_denied",
        };
        conn.execute(
            "INSERT OR REPLACE INTO approval_queue (id, data, status) VALUES (?1, ?2, ?3)",
            rusqlite::params![request.id, data, status],
        )?;
        Ok(())
    }

    /// Update only the status column in SQLite (used on approve/reject).
    #[cfg(feature = "backend-sqlite")]
    fn update_status_sqlite(
        db_path: &str,
        request: &ApprovalRequest,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let conn = rusqlite::Connection::open(db_path)?;
        let data = serde_json::to_string(request)?;
        let status = match &request.status {
            ApprovalStatus::Pending => "pending",
            ApprovalStatus::Approved { .. } => "approved",
            ApprovalStatus::Rejected { .. } => "rejected",
            ApprovalStatus::EscalatedToManager { .. } => "escalated",
            ApprovalStatus::AutoDenied { .. } => "auto_denied",
        };
        conn.execute(
            "UPDATE approval_queue SET data = ?1, status = ?2 WHERE id = ?3",
            rusqlite::params![data, status, request.id],
        )?;
        Ok(())
    }

    /// Delete a finalized request from SQLite.
    #[cfg(feature = "backend-sqlite")]
    fn delete_from_sqlite(
        db_path: &str,
        id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let conn = rusqlite::Connection::open(db_path)?;
        conn.execute(
            "DELETE FROM approval_queue WHERE id = ?1",
            rusqlite::params![id],
        )?;
        Ok(())
    }

    /// Attach an approval preference learner to record decisions.
    pub fn with_learner(mut self, learner: Arc<StdRwLock<ApprovalPreferenceLearner>>) -> Self {
        self.learner = Some(learner);
        self
    }

    /// Attach a SecurityGovernor for audit logging.
    pub fn with_security_governor(mut self, governor: Arc<SecurityGovernor>) -> Self {
        self.security_governor = Some(governor);
        self
    }

    /// Set the approval preference learner (mutating variant for already-constructed engines).
    pub fn set_learner(&mut self, learner: Arc<StdRwLock<ApprovalPreferenceLearner>>) {
        self.learner = Some(learner);
    }

    /// Register an approver ID resolver.
    ///
    /// The resolver receives an `ApprovalRequest` reference and should return
    /// the corresponding approver identity, or `None` if unknown.
    ///
    /// Example:
    /// ```text
    /// use std::collections::HashMap;
    /// struct MyResolver { mapping: HashMap<String, String> }
    /// impl ApproverResolver for MyResolver {
    ///     fn resolve_approver_id(&self, request: &ApprovalRequest) -> Option<String> {
    ///         self.mapping.get(&request.user).cloned()
    ///     }
    /// }
    /// engine.register_approver_resolver(Box::new(MyResolver { mapping: HashMap::new() }));
    /// ```
    pub fn register_approver_resolver(&mut self, resolver: Box<dyn ApproverResolver>) {
        self.approver_registry.register(resolver);
    }

    // ── Submission ──────────────────────────────────────────────────────────

    /// Submit a request for approval. Returns the generated request id.
    pub fn submit_for_approval(&mut self, request: ApprovalRequest) -> String {
        let id = request.id.clone();
        debug!(%id, action = %request.action, "Approval request submitted");

        // Build a default escalation chain for this request.
        // Use the approver ID registry to fill `approver_id`.

        let steps = vec![
            EscalationStep {
                level: 1,
                approver_role: "manager".to_string(),
                approver_id: self.approver_registry.resolve(&request),
                comment: None,
            },
            EscalationStep {
                level: 2,
                approver_role: "director".to_string(),
                approver_id: self.approver_registry.resolve(&request),
                comment: None,
            },
        ];
        self.escalation_chains
            .insert(id.clone(), EscalationChain::new(steps));
        self.queue.push(request.clone());

        // Persist to SQLite if configured.
        if let Some(ref _db_path) = self.db_path {
            #[cfg(feature = "backend-sqlite")]
            if let Err(e) = Self::upsert_sqlite(_db_path, &request) {
                tracing::warn!(error = %e, id = %id, "Failed to persist approval request to SQLite");
            }
        }

        id
    }

    // ── Approval / Rejection ─────────────────────────────────────────────────

    /// Approve a pending request.
    pub fn approve(
        &mut self,
        id: &str,
        approver: &str,
        comment: &str,
    ) -> Result<(), ApprovalError> {
        let status = {
            let request = self
                .queue
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or_else(|| ApprovalError::NotFound(id.to_string()))?;

            if request.status.is_finalized() {
                return Err(ApprovalError::AlreadyFinalized(
                    id.to_string(),
                    request.status.clone(),
                ));
            }

            let now = current_timestamp_ms();
            request.status = ApprovalStatus::Approved {
                approver: approver.to_string(),
                comment: comment.to_string(),
                timestamp_ms: now,
            };
            request.clone()
        };

        info!(%id, approver = %approver, "Approval request approved");

        // Record audit entry for this approval.
        if let Some(ref governor) = self.security_governor {
            let audit_entry = AuditEntry::new(
                format!("approval-{}", id),
                PolicyVerdict::allow(),
                format!("approval:{}:{}", status.user, status.action),
                approver.to_string(),
                format!("Approved by {}: {}", approver, comment),
            );
            governor.record_audit(audit_entry);
        }

        self.feedback_to_pua(&status);
        self.feedback_to_learner(&status);

        // Update SQLite if configured.
        if let Some(ref _db_path) = self.db_path {
            #[cfg(feature = "backend-sqlite")]
            if let Err(e) = Self::update_status_sqlite(_db_path, &status) {
                tracing::warn!(error = %e, id = %id, "Failed to update approval in SQLite");
            }
        }

        Ok(())
    }

    /// Reject a pending request with a reason.
    pub fn reject(&mut self, id: &str, approver: &str, reason: &str) -> Result<(), ApprovalError> {
        let request_clone = {
            let request = self
                .queue
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or_else(|| ApprovalError::NotFound(id.to_string()))?;

            if request.status.is_finalized() {
                return Err(ApprovalError::AlreadyFinalized(
                    id.to_string(),
                    request.status.clone(),
                ));
            }

            let now = current_timestamp_ms();
            request.status = ApprovalStatus::Rejected {
                approver: approver.to_string(),
                reason: reason.to_string(),
                timestamp_ms: now,
            };
            request.clone()
        };

        warn!(%id, approver = %approver, reason = %reason, "Approval request rejected");

        // Record audit entry for this rejection.
        if let Some(ref governor) = self.security_governor {
            let audit_entry = AuditEntry::new(
                format!("approval-{}", id),
                PolicyVerdict::deny("approval-engine", "Rejected by approver"),
                format!("approval:{}:{}", request_clone.user, request_clone.action),
                approver.to_string(),
                format!("Rejected by {}: {}", approver, reason),
            );
            governor.record_audit(audit_entry);
        }

        self.feedback_to_pua(&request_clone);
        self.feedback_to_learner(&request_clone);

        // Update SQLite if configured.
        if let Some(ref _db_path) = self.db_path {
            #[cfg(feature = "backend-sqlite")]
            if let Err(e) = Self::update_status_sqlite(_db_path, &request_clone) {
                tracing::warn!(error = %e, id = %id, "Failed to update rejection in SQLite");
            }
        }

        Ok(())
    }

    // ── Timeout Processing ───────────────────────────────────────────────────

    /// Process timeouts for all pending requests.
    /// Should be called periodically (e.g., every 30 seconds via a tokio interval).
    /// Returns a list of request IDs whose status changed.
    pub async fn process_timeouts(&mut self) -> Vec<String> {
        let mut changed = Vec::new();
        let now = current_timestamp_ms();

        // Collect indices of requests needing processing (cannot mutate while iterating)
        let pending_indices: Vec<usize> = self
            .queue
            .iter()
            .enumerate()
            .filter(|(_, r)| !r.status.is_finalized())
            .map(|(i, _)| i)
            .collect();

        // Collect changes first, then apply feedback (avoids borrow conflicts)
        let mut actions: Vec<(usize, ApprovalRequest)> = Vec::new();

        for idx in pending_indices {
            let request = &self.queue[idx];
            let pending_ms = now.saturating_sub(request.created_at_ms);
            let escalate_ms = apply_multiplier(
                request.risk_level.escalate_timeout(),
                self.timeout_policy.escalate_timeout_multiplier,
            )
            .as_millis() as u64;
            let deny_ms = apply_multiplier(
                request.risk_level.deny_timeout(),
                self.timeout_policy.deny_timeout_multiplier,
            )
            .as_millis() as u64;

            if pending_ms >= deny_ms {
                let mut updated = request.clone();
                updated.status = ApprovalStatus::AutoDenied {
                    reason: format!("Timeout exceeded ({} ms)", deny_ms),
                    timestamp_ms: now,
                };
                warn!(id = %updated.id, "Approval request auto-denied due to timeout");
                changed.push(updated.id.clone());
                actions.push((idx, updated));
            } else if pending_ms >= escalate_ms {
                // Check if escalation chain allows further escalation
                let can_escalate = self
                    .escalation_chains
                    .get(&request.id)
                    .map(|chain| {
                        chain.current_step < chain.steps.len() as u32
                            && chain.current_step < self.timeout_policy.max_escalation_depth
                    })
                    .unwrap_or(false);

                if can_escalate {
                    let mut updated = request.clone();
                    updated.status = ApprovalStatus::EscalatedToManager {
                        from_level: request.risk_level.clone(),
                        timestamp_ms: now,
                    };
                    if let Some(chain) = self.escalation_chains.get(&request.id) {
                        updated.escalated_from = Some(
                            chain.steps[chain.current_step as usize]
                                .approver_role
                                .clone(),
                        );
                    }
                    debug!(
                        id = %updated.id,
                        step = self.escalation_chains.get(&request.id).map(|c| c.current_step).unwrap_or(0),
                        "Approval request escalated"
                    );
                    changed.push(updated.id.clone());
                    actions.push((idx, updated));

                    // Advance the escalation chain
                    if let Some(chain) = self.escalation_chains.get_mut(&request.id) {
                        chain.current_step += 1;
                    }
                }
            }
        }

        // Apply collected updates to queue (single-threaded, shared state).
        for (idx, updated_request) in &actions {
            self.queue[*idx].status = updated_request.status.clone();
            self.queue[*idx].escalated_from = updated_request.escalated_from.clone();
        }

        // ── G8: Concurrent feedback & persistence ────────────────────────
        // Fire feedback (already async via tokio::spawn) and persistence
        // calls concurrently so that multiple escalation notifications are
        // not serialised.  The feedback_to_pua method already spawns a
        // tokio task internally; for persistence we spawn per-request
        // threads to achieve parallel SQLite writes.
        let snapshots: Vec<ApprovalRequest> = actions.iter().map(|(_, r)| r.clone()).collect();
        let db_path = self.db_path.clone();
        // Persist to SQLite via spawn_blocking to avoid blocking the tokio runtime.
        if let Some(db_path) = db_path {
            let mut handles = Vec::with_capacity(snapshots.len());
            for req in &snapshots {
                let _db_path = db_path.clone();
                let _req = req.clone();
                handles.push(tokio::task::spawn_blocking(move || {
                    #[cfg(feature = "backend-sqlite")]
                    if let Err(e) = Self::update_status_sqlite(&_db_path, &_req) {
                        tracing::warn!(
                            error = %e,
                            id = %_req.id,
                            "Failed to persist timeout status to SQLite"
                        );
                    }
                }));
            }
            for handle in handles {
                let _ = handle.await;
            }
        }
        // Serial feedback calls — each `feedback_to_pua()` internally spawns
        // a tokio task, so they are effectively fire-and-forget.
        for req in &snapshots {
            self.feedback_to_pua(req);
            self.feedback_to_learner(req);
        }

        changed
    }

    // ── Queries ──────────────────────────────────────────────────────────────

    /// Get a request by id.
    pub fn get_request(&self, id: &str) -> Option<&ApprovalRequest> {
        self.queue.iter().find(|r| r.id == id)
    }

    /// Get all pending requests.
    pub fn pending_requests(&self) -> Vec<&ApprovalRequest> {
        self.queue
            .iter()
            .filter(|r| !r.status.is_finalized())
            .collect()
    }

    /// Get all requests for a specific user.
    pub fn requests_for_user(&self, user: &str) -> Vec<&ApprovalRequest> {
        self.queue.iter().filter(|r| r.user == user).collect()
    }

    /// Get the escalation chain for a request.
    pub fn get_escalation_chain(&self, id: &str) -> Option<&EscalationChain> {
        self.escalation_chains.get(id)
    }

    /// Remove finalized requests older than the specified duration.
    pub fn purge_old(&mut self, max_age: Duration) {
        let cutoff = current_timestamp_ms().saturating_sub(max_age.as_millis() as u64);
        let old_ids: Vec<String> = self
            .queue
            .iter()
            .filter(|r| r.created_at_ms < cutoff && r.status.is_finalized())
            .map(|r| r.id.clone())
            .collect();
        // G9: Use swap_remove for O(1) removal when order doesn't matter.
        // Iterate from the end so that swap_remove indices are stable.
        let mut i = self.queue.len();
        while i > 0 {
            i -= 1;
            let remove =
                self.queue[i].created_at_ms < cutoff && self.queue[i].status.is_finalized();
            if remove {
                self.escalation_chains.remove(&self.queue[i].id);
                self.queue.swap_remove(i);
            }
        }

        // Remove from SQLite if configured.
        if let Some(ref _db_path) = self.db_path {
            for _id in &old_ids {
                #[cfg(feature = "backend-sqlite")]
                if let Err(e) = Self::delete_from_sqlite(_db_path, _id) {
                    tracing::warn!(error = %e, id = %_id, "Failed to remove purged approval from SQLite");
                }
            }
        }
    }

    // ── Feedback ─────────────────────────────────────────────────────────────

    fn feedback_to_pua(&self, request: &ApprovalRequest) {
        // Provide feedback to the PUA rule engine about the approval outcome.
        // This is non-blocking; errors are logged but not propagated.
        let engine = self.pua_engine.clone();
        let request_clone = request.clone();
        tokio::spawn(async move {
            let engine = engine.lock().await;
            // The feedback is informational; the PUA engine may use it to
            // adjust enforcement plans based on approval outcomes.
            if let Err(e) = engine.evaluate_approval_feedback(&request_clone) {
                warn!(error = %e, "Failed to send approval feedback to PUA engine");
            }
        });
    }

    /// Record the approval decision in the preference learner (if attached).
    fn feedback_to_learner(&self, request: &ApprovalRequest) {
        if let Some(ref learner) = self.learner {
            if let Ok(mut guard) = learner.write() {
                match &request.status {
                    ApprovalStatus::Approved { approver, .. } => {
                        guard.record_approval(&request.action, approver);
                        debug!(
                            action = %request.action,
                            approver = %approver,
                            "Approval decision recorded in preference learner"
                        );
                    }
                    ApprovalStatus::Rejected {
                        approver, reason, ..
                    } => {
                        guard.record_rejection(&request.action, reason);
                        debug!(
                            action = %request.action,
                            approver = %approver,
                            reason = %reason,
                            "Rejection decision recorded in preference learner"
                        );
                    }
                    ApprovalStatus::EscalatedToManager { from_level, .. } => {
                        let level_str = format!("{:?}", from_level);
                        guard.record_escalation(&request.action, &level_str);
                        debug!(
                            action = %request.action,
                            from_level = %level_str,
                            "Escalation recorded in preference learner"
                        );
                    }
                    ApprovalStatus::AutoDenied { reason, .. } => {
                        guard.record_auto_denial(&request.action, reason);
                        debug!(
                            action = %request.action,
                            reason = %reason,
                            "Auto-denial recorded in preference learner"
                        );
                    }
                    ApprovalStatus::Pending => {}
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn current_timestamp_ms() -> u64 {
    crate::shared::timestamps::now_ts_ms() as u64
}

fn apply_multiplier(base: Duration, multiplier: f64) -> Duration {
    if multiplier <= 0.0 {
        return base;
    }
    let secs = base.as_secs_f64() * multiplier;
    Duration::from_secs_f64(secs)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governance::pua::PuaEnforcementPlan;

    fn dummy_engine() -> Arc<Mutex<PuaRuleEngine>> {
        let plan = PuaEnforcementPlan::default();
        let engine = PuaRuleEngine::new(Arc::new(std::sync::Mutex::new(plan)));
        Arc::new(Mutex::new(engine))
    }

    #[tokio::test]
    async fn test_submit_and_approve() {
        let mut engine = ApprovalEngine::new(dummy_engine(), TimeoutPolicy::default());
        let req = ApprovalRequest::new(
            "alice".into(),
            "deploy".into(),
            RiskLevel::Low,
            HashMap::new(),
        );
        let id = engine.submit_for_approval(req);
        assert!(engine.get_request(&id).is_some());
        assert_eq!(
            engine
                .get_request(&id)
                .expect("request should exist after submission")
                .status,
            ApprovalStatus::Pending
        );

        engine
            .approve(&id, "bob", "Looks good")
            .expect("approval should succeed");
        let status = &engine
            .get_request(&id)
            .expect("request should still exist after approval")
            .status;
        assert!(status.is_finalized());
        assert!(matches!(status, ApprovalStatus::Approved { .. }));
    }

    #[tokio::test]
    async fn test_submit_and_reject() {
        let mut engine = ApprovalEngine::new(dummy_engine(), TimeoutPolicy::default());
        let id = engine.submit_for_approval(ApprovalRequest::new(
            "alice".into(),
            "deploy".into(),
            RiskLevel::Medium,
            HashMap::new(),
        ));
        engine
            .reject(&id, "bob", "Not ready")
            .expect("rejection should succeed");
        let status = &engine
            .get_request(&id)
            .expect("request should exist after rejection")
            .status;
        assert!(matches!(status, ApprovalStatus::Rejected { .. }));
    }

    #[tokio::test]
    async fn test_double_approval_fails() {
        let mut engine = ApprovalEngine::new(dummy_engine(), TimeoutPolicy::default());
        let id = engine.submit_for_approval(ApprovalRequest::new(
            "alice".into(),
            "deploy".into(),
            RiskLevel::Low,
            HashMap::new(),
        ));
        engine
            .approve(&id, "bob", "ok")
            .expect("first approval should succeed");
        let err = engine.approve(&id, "charlie", "also ok").unwrap_err();
        assert!(matches!(err, ApprovalError::AlreadyFinalized(_, _)));
    }

    #[tokio::test]
    async fn test_auto_escalation_and_deny() {
        // Use large multipliers so the timeouts are in seconds we can wait for
        let policy = TimeoutPolicy {
            escalate_timeout_multiplier: 10.0, // 300s * 10 = 3000s → won't trigger
            deny_timeout_multiplier: 0.0001,   // 900s * 0.0001 = 0.09s = 90ms
            ..Default::default()
        };
        let mut engine = ApprovalEngine::new(dummy_engine(), policy);
        let id = engine.submit_for_approval(ApprovalRequest::new(
            "alice".into(),
            "deploy".into(),
            RiskLevel::Low,
            HashMap::new(),
        ));

        // Wait for auto-deny timeout
        tokio::time::sleep(Duration::from_millis(150)).await;
        let changed = engine.process_timeouts().await;
        assert!(!changed.is_empty(), "Expected auto-deny action");

        let status = &engine
            .get_request(&id)
            .expect("request should exist after timeout processing")
            .status;
        assert!(
            status.is_finalized(),
            "Request should be auto-denied after deny timeout"
        );
        assert!(matches!(status, ApprovalStatus::AutoDenied { .. }));
    }
}
