//! ACP Prelude - Shared Data Types
//!
//! Simple data types (structs, enums) used across the ACP system.
//! Each type family with significant `impl` blocks lives in its own module
//! (e.g. `circuit_breaker.rs`, `lifecycle.rs`, `runtime_metrics.rs`).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ============================================================================
// Conversation types
// ============================================================================

/// Conversation checkpoint — architectural placeholder.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
