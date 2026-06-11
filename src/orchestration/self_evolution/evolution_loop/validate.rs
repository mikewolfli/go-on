//! Validation phase — approval types and decisions.
//!
//! Contains [`ApprovalMode`], [`Approval`], and [`ApprovalStatus`] types
//! that control how evolution cycles are approved or rejected.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// ApprovalMode
// ---------------------------------------------------------------------------

/// Describes how approval is handled for an evolution cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalMode {
    /// Automatically approve all evolution cycles.
    AutoApproval,
    /// Require explicit approval (from a trusted subsystem) before applying.
    RequireApproval,
    /// Require human sign-off before applying.
    RequireHuman,
}

impl ApprovalMode {
    /// Returns true if this mode requires some form of approval.
    pub fn requires_approval(&self) -> bool {
        !matches!(self, ApprovalMode::AutoApproval)
    }

    /// Returns true if this mode specifically requires human intervention.
    pub fn requires_human(&self) -> bool {
        matches!(self, ApprovalMode::RequireHuman)
    }
}

// ---------------------------------------------------------------------------
// Approval
// ----------------------------------------------------------------------------

/// Record of an approval decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Approval {
    /// Who or what approved the evolution.
    pub by: String,
    /// The approval status.
    pub status: ApprovalStatus,
    /// Optional comment explaining the decision.
    pub comment: Option<String>,
    /// Timestamp in milliseconds.
    pub timestamp_ms: u64,
}

impl Approval {
    /// Create a new approved approval.
    pub fn approved(by: String, comment: Option<String>) -> Self {
        Self {
            by,
            status: ApprovalStatus::Approved,
            comment,
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }

    /// Create a new rejected approval.
    pub fn rejected(by: String, comment: Option<String>) -> Self {
        Self {
            by,
            status: ApprovalStatus::Rejected,
            comment,
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }

    /// Returns true if this approval is approved.
    pub fn is_approved(&self) -> bool {
        self.status == ApprovalStatus::Approved
    }
}

/// Approval status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalStatus {
    /// The evolution was approved.
    Approved,
    /// The evolution was rejected.
    Rejected,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_approval_modes() {
        assert!(!ApprovalMode::AutoApproval.requires_approval());
        assert!(ApprovalMode::RequireApproval.requires_approval());
        assert!(ApprovalMode::RequireHuman.requires_human());
    }

    #[test]
    fn test_approval_approved() {
        let a = Approval::approved("tester".to_string(), Some("looks good".to_string()));
        assert!(a.is_approved());
        assert_eq!(a.by, "tester");
    }

    #[test]
    fn test_approval_rejected() {
        let a = Approval::rejected("tester".to_string(), Some("not now".to_string()));
        assert!(!a.is_approved());
        assert_eq!(a.status, ApprovalStatus::Rejected);
    }
}
