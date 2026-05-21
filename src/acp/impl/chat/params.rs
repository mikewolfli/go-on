//! Chat parameter types and request context
//!
//! This module contains the parameter and request context types used
//! by the chat handling implementation, along with token economy estimation.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::acp::r#impl::UserSession;
use crate::agent::Message;
use crate::config::PhaseOptions;
use crate::reinforcement::{
    ExecutionDecisionCandidate, RequirementContractArtifact, TaskPlanArtifact,
};

use super::helpers::round_metric;

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
    pub vector_hits: Option<Vec<serde_json::Value>>,
    /// Optional execution decision candidate
    pub execution_decision_candidate: Option<ExecutionDecisionCandidate>,
}

/// Context for a chat request, including authentication and tenant info.
#[derive(Debug, Clone)]
pub struct ChatRequestContext {
    /// Authenticated user session, if user auth is enabled.
    #[allow(dead_code)] // Public API — reserved for audit logging and in-chat RBAC
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

/// Estimate token economy from input messages and response text
pub(crate) fn estimate_token_economy(messages: &[Message], response_text: &str) -> Value {
    let input_chars = messages
        .iter()
        .map(|message| message.content.chars().count())
        .sum::<usize>();
    let output_chars = response_text.chars().count();
    let input_tokens = if input_chars == 0 {
        0_u64
    } else {
        input_chars.div_ceil(4) as u64
    };
    let output_tokens = if output_chars == 0 {
        0_u64
    } else {
        output_chars.div_ceil(4) as u64
    };
    let compression_ratio = if input_tokens == 0 {
        1.0
    } else {
        round_metric(output_tokens as f64 / input_tokens as f64)
    };
    let saving_ratio = if input_tokens == 0 {
        0.0
    } else {
        round_metric((1.0 - compression_ratio).clamp(0.0, 1.0))
    };

    json!({
        "schema_version": "blue25-stream-token-economy-v1",
        "round": 1,
        "input_chars": input_chars,
        "output_chars": output_chars,
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "total_tokens": input_tokens + output_tokens,
        "compression_ratio": compression_ratio,
        "saving_ratio": saving_ratio,
        "efficiency_class": if compression_ratio <= 0.60 {
            "strong"
        } else if compression_ratio <= 0.85 {
            "efficient"
        } else {
            "expanded"
        },
    })
}

#[cfg(test)]
mod tests {
    use crate::agent::Message;

    use super::estimate_token_economy;

    #[test]
    fn estimate_token_economy_reports_compression_ratio() {
        let payload = estimate_token_economy(
            &[Message {
                role: "user".to_string(),
                content: "Summarize this large body of implementation detail into one paragraph."
                    .to_string(),
            }],
            "Short summary.",
        );

        assert!(payload["input_tokens"].as_u64().unwrap_or(0) > 0);
        assert!(payload["output_tokens"].as_u64().unwrap_or(0) > 0);
        assert!(payload["compression_ratio"].as_f64().unwrap_or(2.0) <= 1.0);
    }
}
