//! AgentMessage — structured inter-agent message types (BLUE70 §3.3)
//!
//! Defines message types for agent-to-agent communication,
//! including task delegation, results, progress updates, cancellation,
//! status queries, and custom messages.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use uuid::Uuid;

use crate::agents::communication::path::AgentPath;

/// Unique message ID generator.
fn new_msg_id() -> String {
    Uuid::new_v4().to_string()
}

/// Message delivery target — replaces the AgentPathPattern+Channel enum.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentTarget {
    /// Direct message to a specific path.
    Direct(AgentPath),
    /// Broadcast to all descendant agents.
    Broadcast,
    /// Send to the parent agent.
    ToParent,
    /// Simplified wildcard pattern: root/*/coder.
    Pattern {
        prefix: Vec<String>,
        suffix: Vec<String>,
    },
}

impl fmt::Display for AgentTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentTarget::Direct(p) => write!(f, "direct:{}", p),
            AgentTarget::Broadcast => write!(f, "broadcast"),
            AgentTarget::ToParent => write!(f, "to_parent"),
            AgentTarget::Pattern { prefix, suffix } => {
                let pat = if prefix.is_empty() && suffix.is_empty() {
                    "*".to_string()
                } else {
                    let p = prefix.join("/");
                    let s = suffix.join("/");
                    if p.is_empty() {
                        format!("*/{}", s)
                    } else if s.is_empty() {
                        format!("{}/*", p)
                    } else {
                        format!("{}/*/{}", p, s)
                    }
                };
                write!(f, "pattern:{}", pat)
            }
        }
    }
}

/// Structured inter-agent message (BLUE70 §3.3).
///
/// Design notes (simplified vs original):
/// - No `priority` field — ordering is handled by Messenger layer internally.
/// - No `DeliveryGuarantee` enum — only AtMostOnce and AtLeastOnce are used,
///   selected by the Messenger send method.
/// - `AgentTarget` replaces the separate `AgentChannel` enum + `AgentPathPattern`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    /// Message ID (UUID v4).
    pub id: String,
    /// Sender agent path.
    pub from: AgentPath,
    /// Receiver target.
    pub to: AgentTarget,
    /// Message timestamp (ms since epoch).
    pub timestamp_ms: u64,
    /// Message type.
    pub kind: AgentMessageKind,
    /// Message payload (arbitrary JSON).
    pub payload: Value,
    /// Parent message ID (for reply chains).
    pub in_reply_to: Option<String>,
}

impl AgentMessage {
    /// Create a new message with auto-generated ID and timestamp.
    pub fn new(from: AgentPath, to: AgentTarget, kind: AgentMessageKind) -> Self {
        Self {
            id: new_msg_id(),
            from,
            to,
            timestamp_ms: crate::shared::timestamps::now_ts_ms() as u64,
            kind,
            payload: Value::Null,
            in_reply_to: None,
        }
    }

    /// Create a delegate (task assignment) message.
    pub fn delegate(
        from: AgentPath,
        to: AgentTarget,
        task: String,
        role: Option<String>,
        token_budget: Option<u64>,
        timeout_secs: u64,
    ) -> Self {
        Self::new(
            from,
            to,
            AgentMessageKind::Delegate {
                task,
                role,
                token_budget,
                timeout_secs,
            },
        )
    }

    /// Create a result message.
    #[allow(clippy::too_many_arguments)]
    pub fn result(
        from: AgentPath,
        to: AgentTarget,
        success: bool,
        summary: Option<String>,
        changes: Option<String>,
        evidence: Option<String>,
        risks: Option<String>,
        blockers: Option<String>,
        response: String,
        actual_tokens: u64,
    ) -> Self {
        Self::new(
            from,
            to,
            AgentMessageKind::Result {
                success,
                summary,
                changes,
                evidence,
                risks,
                blockers,
                response,
                actual_tokens,
            },
        )
    }

    /// Create a progress update message.
    pub fn progress(from: AgentPath, to: AgentTarget, tokens: String, partial: bool) -> Self {
        Self::new(from, to, AgentMessageKind::Progress { tokens, partial })
    }

    /// Create a cancel request message.
    pub fn cancel(from: AgentPath, to: AgentTarget, reason: String) -> Self {
        Self::new(from, to, AgentMessageKind::Cancel { reason })
    }

    /// Create a status query message.
    pub fn status_query(from: AgentPath, to: AgentTarget) -> Self {
        Self::new(from, to, AgentMessageKind::StatusQuery)
    }

    /// Set the payload of this message.
    pub fn with_payload(mut self, payload: Value) -> Self {
        self.payload = payload;
        self
    }

    /// Set the in_reply_to field.
    pub fn in_reply_to(mut self, reply_to: String) -> Self {
        self.in_reply_to = Some(reply_to);
        self
    }

    /// Check if this message kind is a result.
    pub fn is_result(&self) -> bool {
        matches!(self.kind, AgentMessageKind::Result { .. })
    }

    /// Check if this message kind is a cancel.
    pub fn is_cancel(&self) -> bool {
        matches!(self.kind, AgentMessageKind::Cancel { .. })
    }

    /// Check if this message kind is a delegate.
    pub fn is_delegate(&self) -> bool {
        matches!(self.kind, AgentMessageKind::Delegate { .. })
    }
}

/// Message type variants (BLUE70 §3.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AgentMessageKind {
    /// Task delegation: parent → child.
    Delegate {
        task: String,
        role: Option<String>,
        token_budget: Option<u64>,
        timeout_secs: u64,
    },
    /// Task result: child → parent.
    Result {
        success: bool,
        summary: Option<String>,
        changes: Option<String>,
        evidence: Option<String>,
        risks: Option<String>,
        blockers: Option<String>,
        response: String,
        actual_tokens: u64,
    },
    /// Progress update: child → parent (streaming intermediate results).
    Progress { tokens: String, partial: bool },
    /// Cancel request: parent → child.
    Cancel { reason: String },
    /// Status query: any → any.
    StatusQuery,
    /// Status response.
    StatusResponse {
        phase: String,
        elapsed_ms: u64,
        tokens_used: u64,
    },
    /// Custom event.
    Custom { event: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_message() {
        let from = AgentPath::parse("root").unwrap();
        let to = AgentTarget::Direct(AgentPath::parse("root/research").unwrap());
        let msg = AgentMessage::new(from.clone(), to, AgentMessageKind::StatusQuery);
        assert_eq!(msg.from, from);
        assert!(matches!(msg.kind, AgentMessageKind::StatusQuery));
        assert!(!msg.id.is_empty());
        assert!(msg.timestamp_ms > 0);
    }

    #[test]
    fn test_delegate_message() {
        let msg = AgentMessage::delegate(
            AgentPath::parse("root").unwrap(),
            AgentTarget::Direct(AgentPath::parse("root/coder").unwrap()),
            "implement feature".to_string(),
            Some("engineer".to_string()),
            Some(10000),
            300,
        );
        assert!(msg.is_delegate());
        if let AgentMessageKind::Delegate {
            task,
            role,
            token_budget,
            timeout_secs,
        } = &msg.kind
        {
            assert_eq!(task, "implement feature");
            assert_eq!(role.as_deref(), Some("engineer"));
            assert_eq!(*token_budget, Some(10000));
            assert_eq!(*timeout_secs, 300);
        } else {
            panic!("Expected Delegate");
        }
    }

    #[test]
    fn test_result_message() {
        let msg = AgentMessage::result(
            AgentPath::parse("root/research").unwrap(),
            AgentTarget::ToParent,
            true,
            Some("task completed".to_string()),
            None,
            None,
            None,
            None,
            "detailed response".to_string(),
            5000,
        );
        assert!(msg.is_result());
        if let AgentMessageKind::Result {
            success,
            summary,
            response,
            actual_tokens,
            ..
        } = &msg.kind
        {
            assert!(*success);
            assert_eq!(summary.as_deref(), Some("task completed"));
            assert_eq!(response, "detailed response");
            assert_eq!(*actual_tokens, 5000);
        } else {
            panic!("Expected Result");
        }
    }

    #[test]
    fn test_cancel_message() {
        let msg = AgentMessage::cancel(
            AgentPath::parse("root").unwrap(),
            AgentTarget::Broadcast,
            "timeout".to_string(),
        );
        assert!(msg.is_cancel());
    }

    #[test]
    fn test_progress_message() {
        let msg = AgentMessage::progress(
            AgentPath::parse("root/research").unwrap(),
            AgentTarget::ToParent,
            "thinking...".to_string(),
            true,
        );
        if let AgentMessageKind::Progress { tokens, partial } = &msg.kind {
            assert_eq!(tokens, "thinking...");
            assert!(*partial);
        } else {
            panic!("Expected Progress");
        }
    }

    #[test]
    fn test_with_payload() {
        let msg = AgentMessage::new(
            AgentPath::parse("root").unwrap(),
            AgentTarget::ToParent,
            AgentMessageKind::Custom {
                event: "test".to_string(),
            },
        )
        .with_payload(serde_json::json!({"key": "value"}));
        assert_eq!(msg.payload["key"], "value");
    }

    #[test]
    fn test_in_reply_to() {
        let parent_id = "parent-uuid".to_string();
        let msg = AgentMessage::status_query(
            AgentPath::parse("root/child").unwrap(),
            AgentTarget::ToParent,
        )
        .in_reply_to(parent_id.clone());
        assert_eq!(msg.in_reply_to, Some(parent_id));
    }

    #[test]
    fn test_target_display() {
        let path = AgentPath::parse("root/coder").unwrap();
        assert_eq!(AgentTarget::Direct(path).to_string(), "direct:root/coder");
        assert_eq!(AgentTarget::Broadcast.to_string(), "broadcast");
        assert_eq!(AgentTarget::ToParent.to_string(), "to_parent");
        assert_eq!(
            AgentTarget::Pattern {
                prefix: vec!["root".to_string()],
                suffix: vec!["coder".to_string()]
            }
            .to_string(),
            "pattern:root/*/coder"
        );
    }
}
