//! Chat parameter types and request context
//!
//! This module contains the core parameter and context types used
//! throughout the chat request lifecycle. These were extracted from
//! the parent `chat.rs` to reduce the monolithic file size.

use std::collections::HashMap;
use std::sync::{Mutex as StdMutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::acp::r#impl::UserSession;
use crate::agent::Message;
use crate::config::PhaseOptions;
use crate::reinforcement::{
    ExecutionDecisionCandidate, RequirementContractArtifact, TaskPlanArtifact,
};

/// Chat parameters structure
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatParams {
    /// Chat mode (e.g., "ask", "edit", "agent", "safeguard", "full_auto").
    /// When absent or empty (e.g., from external clients like Zed),
    /// defaults to "ask" (the safest general-purpose mode).
    #[serde(default)]
    pub mode: String,
    /// Messages to process
    pub messages: Vec<Message>,
    /// Optional conversation ID for continuation
    pub conversation_id: Option<String>,
    /// Optional branch ID for tree-based history
    pub branch_id: Option<String>,
    /// Optional phase to force
    pub phase: Option<String>,
    /// Optional options for phase configuration
    pub options: Option<PhaseOptions>,
    /// Optional requirement contract
    pub requirement_contract: Option<RequirementContractArtifact>,
    /// Optional task plan
    pub plan: Option<TaskPlanArtifact>,
    /// Optional vector search hits
    pub vector_hits: Option<Vec<Value>>,
    /// Optional execution decision candidate
    pub execution_decision_candidate: Option<ExecutionDecisionCandidate>,
}

/// Context for a chat request, including authentication and tenant info.
#[derive(Debug, Clone)]
pub struct ChatRequestContext {
    /// Authenticated user session, if user auth is enabled.
    #[allow(dead_code)] // F-GAP-49 — Public API — reserved for audit logging and in-chat RBAC
    pub user_session: Option<UserSession>,
    /// Resolved tenant ID (from user session, or conversation_id, or default).
    pub tenant_id: String,
}

impl ChatRequestContext {
    /// Create a new context with optional user session.
    pub fn new(user_session: Option<UserSession>) -> Self {
        let tenant_id = user_session
            .as_ref()
            .and_then(|s| s.tenant_id.clone())
            .unwrap_or_else(|| "default-tenant".to_string());
        Self {
            user_session,
            tenant_id,
        }
    }
}

/// Tracks per-phase agent overrides for the Agent Switch mechanism.
#[derive(Default)]
pub(crate) struct AgentSwitchState {
    pub forced_agent_by_phase: HashMap<String, String>,
    #[allow(dead_code)] // F-GAP-49 — reserved for agent switch state extensibility
    pub primary_agent_by_phase: HashMap<String, String>,
}

static AGENT_SWITCH_STATE: OnceLock<StdMutex<AgentSwitchState>> = OnceLock::new();

pub(crate) fn agent_switch_state() -> &'static StdMutex<AgentSwitchState> {
    AGENT_SWITCH_STATE.get_or_init(|| StdMutex::new(AgentSwitchState::default()))
}
