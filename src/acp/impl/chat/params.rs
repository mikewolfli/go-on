//! Chat parameter types and request context
//!
//! This module contains the core parameter and context types used
//! throughout the chat request lifecycle. These were extracted from
//! the parent `chat.rs` to reduce the monolithic file size.

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
    /// defaults to "edit".
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

/// Context for a chat request, including tenant and user info.
#[derive(Debug, Clone)]
pub struct ChatRequestContext {
    /// Resolved tenant ID (from user session, or conversation_id, or default).
    pub tenant_id: String,
    /// Resolved user ID for multi-user isolation (from user session when auth is enabled).
    pub user_id: Option<String>,
}

impl ChatRequestContext {
    /// Create a new context with optional user session.
    pub fn new(user_session: Option<UserSession>) -> Self {
        let tenant_id = user_session
            .as_ref()
            .and_then(|s| s.tenant_id.clone())
            .unwrap_or_else(|| "default-tenant".to_string());
        let user_id = user_session.map(|s| s.user_id);
        Self { tenant_id, user_id }
    }
}
