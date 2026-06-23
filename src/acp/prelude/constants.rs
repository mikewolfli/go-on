//! ACP Prelude - Constants
//!
//! Shared constants used throughout the ACP system.

/// Maximum checkpoints per conversation
pub const MAX_CHECKPOINTS_PER_CONVERSATION: usize = 256;

// Slow lock wait threshold removed (log-20260622-5): lock monitor stats
// were never queried in production. The constant ACP_LOCK_SLOW_WAIT_THRESHOLD
// and all wait-time instrumentation were eliminated. See lock_monitor.rs docs.
