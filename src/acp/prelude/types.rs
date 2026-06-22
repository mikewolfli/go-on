//! ACP Prelude - Shared Data Types
//!
//! Simple data types (structs, enums) used across the ACP system.
//! Each type family with significant `impl` blocks lives in its own module
//! (e.g. `circuit_breaker.rs`, `lifecycle.rs`, `runtime_metrics.rs`).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::Message;
use crate::config::PhaseOptions;
use crate::reinforcement::{RequirementContractArtifact, TaskPlanArtifact};
use crate::roles::AgentRole;

// ============================================================================
// Conversation types
// ============================================================================

/// Conversation checkpoint structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationCheckpoint {
    /// Unique checkpoint ID
    pub checkpoint_id: String,
    /// Conversation ID
    pub conversation_id: String,
    /// Branch ID
    pub branch_id: String,
    /// Parent checkpoint ID (for branching)
    pub parent_checkpoint_id: Option<String>,
    /// Creation timestamp
    pub created_at: i64,
    /// Optional note
    pub note: Option<String>,
    /// Persisted meta-cognition state for save/restore continuity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metacognitive_loop: Option<Value>,
    /// Messages in this checkpoint
    pub messages: Vec<Message>,
}

/// Conversation state structure
#[derive(Debug, Clone, Default)]
pub struct ConversationState {
    /// Checkpoints in this conversation
    pub checkpoints: Vec<ConversationCheckpoint>,
    /// Branch heads mapping
    pub branch_heads: HashMap<String, String>,
    /// Last touched timestamp
    pub last_touched_at: i64,
}

/// Conversation prune result
///
/// Public API type for ACP consumers.
#[expect(dead_code, reason = "public API surface for ACP consumers")]
#[derive(Debug, Clone, Serialize, Default)]
pub struct ConversationPruneResult {
    /// Number of conversations removed
    pub removed: usize,
    /// Number of branch heads repaired
    pub repaired_heads: usize,
}

// ============================================================================
// Lock monitor snapshot
// ============================================================================

#[derive(Debug, Clone, Serialize, Default)]
pub struct AcpLockSnapshot {
    pub name: String,
    pub acquisitions: u64,
    pub poisoned_total: u64,
    pub recovered_total: u64,
    pub slow_wait_total: u64,
    pub avg_wait_ms: f64,
    pub max_wait_ms: f64,
}

// ============================================================================
// Review types
// ============================================================================

/// Review decision
///
/// Public API type for ACP consumers.
#[expect(dead_code, reason = "public API surface for ACP consumers")]
#[derive(Debug, Clone, Serialize)]
pub struct ReviewDecision {
    /// Reviewer name
    pub reviewer: String,
    /// Verdict (pass/fail/invalid)
    pub verdict: String,
    /// Review response
    pub response: String,
}

/// Review verdict enum
///
/// This public enum uses `Pass`/`Fail`/`Invalid` semantics.
/// There is a separate governance-internal `ReviewVerdict` in
/// `crate::governance::review_controls` that uses `Approve`/`Reject`/`Invalid`.
#[expect(dead_code, reason = "public API surface for ACP consumers")]
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReviewVerdict {
    /// Review passed
    Pass,
    /// Review failed
    Fail,
    /// Invalid review response
    Invalid,
}

#[expect(dead_code, reason = "public API surface for ACP consumers")]
impl ReviewVerdict {
    /// Convert to string
    pub fn as_str(&self) -> &'static str {
        match self {
            ReviewVerdict::Pass => "pass",
            ReviewVerdict::Fail => "fail",
            ReviewVerdict::Invalid => "invalid",
        }
    }
}

// ============================================================================
// Chat / request types
// ============================================================================

/// Chat parameters structure
///
/// Public API type for ACP consumers.
#[expect(dead_code, reason = "public API surface for ACP consumers")]
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatParams {
    /// Chat mode (e.g., "ask", "edit", "agent", "safeguard", "full_auto")
    pub mode: String,
    /// Messages to process
    pub messages: Vec<Message>,
    /// Phase options
    pub phase_options: Option<PhaseOptions>,
    /// Requirement contract
    pub requirement_contract: Option<RequirementContractArtifact>,
    /// Task plan
    pub plan: Option<TaskPlanArtifact>,
    /// Additional parameters
    pub extras: Option<Value>,
}

/// Task characteristics
///
/// Public API type for ACP consumers.
#[expect(dead_code, reason = "public API surface for ACP consumers")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskCharacteristics {
    /// Task complexity (simple, medium, complex)
    pub complexity: String,
    /// Estimated duration in seconds
    pub estimated_duration_seconds: u32,
    /// Required expertise level
    pub required_expertise: String,
    /// Risk level (low, medium, high)
    pub risk_level: String,
    /// Whether parallel execution is possible
    pub can_parallelize: bool,
    /// Required safeguards
    pub required_safeguards: Vec<String>,
}

/// Routing decision
///
/// Public API type for ACP consumers.
#[expect(dead_code, reason = "public API surface for ACP consumers")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingDecision {
    /// Selected roles in execution order
    pub roles: Vec<AgentRole>,
    /// Detailed requirements for each role
    pub requirements: Vec<crate::orchestration::task_router::RoleRequirement>,
    /// Estimated probability of success with selected roles
    pub predicted_success_rate: f32,
    /// Estimated total execution time in seconds
    pub estimated_duration_seconds: u32,
    /// Whether parallel execution is recommended for any roles
    pub can_parallelize: Vec<(AgentRole, AgentRole)>,
    /// Key risk factors identified
    pub risk_factors: Vec<String>,
    /// Recommended safeguards
    pub recommended_safeguards: Vec<String>,
    /// PUA enforcement plan that must be honored downstream
    pub pua_enforcement: crate::pua::PuaEnforcementPlan,
}

// ============================================================================
// Server status (aggregate snapshot)
// ============================================================================

use crate::acp::prelude::circuit_breaker::CircuitBreakerSnapshot;
use crate::acp::prelude::lifecycle::LifecycleSnapshot;
use crate::acp::prelude::maintenance::MaintenanceSnapshot;
use crate::acp::prelude::runtime_metrics::MetricsSnapshot;

/// Server status structure
#[derive(Debug, Clone, Serialize)]
pub struct ServerStatus {
    /// Metrics snapshot
    pub metrics: MetricsSnapshot,
    /// Circuit breaker snapshots
    pub circuit_breakers: Vec<CircuitBreakerSnapshot>,
    /// Lifecycle snapshot
    pub lifecycle: LifecycleSnapshot,
    /// Maintenance snapshot
    pub maintenance: MaintenanceSnapshot,
    /// Governance subsystem health snapshot.
    /// Present when the harness bus is wired.
    pub governance: Option<crate::governance::status::GovernanceStatus>,
    /// Timestamp of this status
    pub timestamp: i64,
}
