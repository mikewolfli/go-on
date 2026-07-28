//! Tool execution progress and streaming event types.
//!
//! Provides [`ToolProgress`] for progress reporting during tool execution
//! and [`ProgressSender`] as the channel type used to deliver those events.
//!
//! These types are designed to be a non-breaking addition to the existing
//! [`Tool`](super::types::Tool) trait. Tools that don't use progress
//! reporting can ignore this module entirely.

use std::time::Duration;

/// Progress update emitted during tool execution.
///
/// Observers subscribe via the [`ProgressSender`] broadcast channel to
/// receive lifecycle and progress events while a tool is running.
#[derive(Debug, Clone)]
pub enum ToolProgress {
    /// Tool execution is starting with the given tool name.
    Started { tool_name: String },

    /// Intermediate progress update with a status message.
    ///
    /// `progress` is a value in the range `0.0`–`1.0` indicating estimated
    /// completion. Tools may emit multiple progress updates.
    Progress { progress: f64, message: String },

    /// Tool completed successfully.
    Completed { duration: Duration },

    /// Tool failed with an error message.
    Failed { error: String },
}

/// Channel sender for tool progress updates.
///
/// This is a [`tokio::sync::broadcast::Sender`] which allows multiple
/// consumers (e.g. UI, logging, orchestrator) to each receive a copy of
/// every progress event.
///
/// # Capacity
///
/// Use [`tokio::sync::broadcast::channel`] to create a sender with a
/// desired capacity. A capacity of 32 or 64 is a reasonable default for
/// most tools. When the channel is full, the oldest unconsumed message
/// is dropped (broadcast channel behaviour).
pub type ProgressSender = tokio::sync::broadcast::Sender<ToolProgress>;

impl ToolProgress {
    /// Returns `true` if this event indicates the tool is still running
    /// (i.e. `Started` or `Progress`).
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            ToolProgress::Started { .. } | ToolProgress::Progress { .. }
        )
    }

    /// Returns `true` if this event indicates the tool has finished
    /// (i.e. `Completed` or `Failed`).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ToolProgress::Completed { .. } | ToolProgress::Failed { .. }
        )
    }

    /// Returns the human-readable label for this event variant.
    pub fn kind(&self) -> &'static str {
        match self {
            ToolProgress::Started { .. } => "started",
            ToolProgress::Progress { .. } => "progress",
            ToolProgress::Completed { .. } => "completed",
            ToolProgress::Failed { .. } => "failed",
        }
    }
}
