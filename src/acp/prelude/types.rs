//! ACP Prelude - Shared Data Types
//!
//! Simple data types (structs, enums) used across the ACP system.
//! Each type family with significant `impl` blocks lives in its own module
//! (e.g. `circuit_breaker.rs`, `lifecycle.rs`, `runtime_metrics.rs`).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::acp::session_log::SessionLog;

// ============================================================================
// Conversation types
// ============================================================================

/// A point-in-time snapshot of a conversation (messages + metacognitive-loop
/// state) used for branching and rollback. Stored per-server in
/// `ConversationState.checkpoints` (created via
/// `checkpoint_pack::create_checkpoint_record`) and threaded through the
/// chat `act` phase so a conversation can be resumed from a prior checkpoint.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConversationCheckpoint {
    pub checkpoint_id: String,
    pub conversation_id: String,
    pub branch_id: String,
    pub parent_checkpoint_id: Option<String>,
    pub created_at: i64,
    pub note: Option<String>,
    pub metacognitive_loop: Option<serde_json::Value>,
    pub messages: Vec<crate::agent::Message>,
}

/// Conversation state — stored per-server, wired into SessionContext.
#[derive(Debug, Clone, Default)]
pub struct ConversationState {
    pub checkpoints: Vec<ConversationCheckpoint>,
    pub branch_heads: HashMap<String, String>,
    /// Append-only session logs keyed by `"{conversation_id}:{branch_id}"`
    /// (the same keying used for `branch_heads`). M1.4: the session log is the
    /// factual event source for a conversation — model-visible history must be
    /// rebuildable from it via `SessionLog::derive_messages`. Logs live
    /// alongside checkpoints and are never pruned when checkpoints are pruned,
    /// so a later fork/resume can derive the conversation history from them.
    pub session_logs: HashMap<String, SessionLog>,
    pub last_touched_at: i64,
}

// ============================================================================
// Server status (aggregate snapshot)
// ============================================================================

use crate::acp::prelude::lifecycle::LifecycleSnapshot;
use crate::acp::prelude::maintenance::MaintenanceSnapshot;
use crate::acp::prelude::runtime_metrics::MetricsSnapshot;

/// Server status structure
#[derive(Debug, Clone, Serialize)]
pub struct ServerStatus {
    pub metrics: MetricsSnapshot,
    pub circuit_breakers: Vec<crate::acp::prelude::CircuitBreakerSnapshot>,
    pub lifecycle: LifecycleSnapshot,
    pub maintenance: MaintenanceSnapshot,
    pub governance: Option<crate::governance::status::GovernanceStatus>,
    pub timestamp: i64,
}
