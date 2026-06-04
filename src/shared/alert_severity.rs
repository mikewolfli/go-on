//! Shared `AlertSeverity` enum.
//!
//! Extracted from `observability::alert_manager` to break the circular
//! dependency: acp → observability → intelligence → acp.
//!
//! Both `acp` and `observability` should import from here.

use serde::{Deserialize, Serialize};

/// Severity level for an alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

impl std::fmt::Display for AlertSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AlertSeverity::Info => write!(f, "info"),
            AlertSeverity::Warning => write!(f, "warning"),
            AlertSeverity::Critical => write!(f, "critical"),
        }
    }
}
