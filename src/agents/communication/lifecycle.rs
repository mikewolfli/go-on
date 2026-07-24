//! AgentLifecycle — Finite State Machine for agent lifecycle (BLUE71 §7)
//!
//! Defines the complete lifecycle of an agent from registration through
//! to terminal states (completed, errored, cancelled). Each state carries
//! timing and token usage metadata for observability.

use serde::{Deserialize, Serialize};

/// Phase within the Active state (BLUE71 §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentPhase {
    /// Planning phase — agent is formulating a plan.
    Planning,
    /// Executing phase — agent is executing tools.
    Executing,
    /// Reflecting phase — agent is reflecting on results.
    Reflecting,
    /// Waiting phase — agent is waiting for sub-agents.
    Waiting,
}

/// Finite State Machine for agent lifecycle (BLUE71 §7.1).
///
/// Transitions:
/// - Registered → Idle → Active → Completed | Errored | Cancelled
/// - Any state can transition to Errored or Cancelled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentLifecycle {
    /// Agent created and registered in the tree.
    Registered {
        /// Timestamp of registration (ms since epoch).
        at_ms: u64,
    },
    /// Agent is idle (registered but not yet active).
    Idle {
        /// Timestamp when agent became idle (ms since epoch).
        since_ms: u64,
    },
    /// Agent is actively processing.
    Active {
        /// Current phase of execution.
        phase: AgentPhase,
        /// Timestamp when agent became active (ms since epoch).
        started_at_ms: u64,
        /// Tokens used so far in this active period.
        tokens_used: u64,
    },
    /// Agent completed successfully.
    Completed {
        /// Summary of the result.
        result: String,
        /// Total tokens used.
        tokens_used: u64,
        /// Wall-clock time in milliseconds.
        wall_time_ms: u64,
        /// Completion timestamp (ms since epoch).
        completed_at_ms: u64,
    },
    /// Agent terminated with an error.
    Errored {
        /// Error description.
        error: String,
        /// Tokens used before error.
        tokens_used: u64,
        /// Wall-clock time in milliseconds.
        wall_time_ms: u64,
    },
    /// Agent was cancelled.
    Cancelled {
        /// Reason for cancellation.
        reason: String,
        /// Tokens used before cancellation.
        tokens_used: u64,
    },
}

impl AgentLifecycle {
    /// Whether this state is terminal (no further transitions possible).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            AgentLifecycle::Completed { .. } | AgentLifecycle::Errored { .. } | AgentLifecycle::Cancelled { .. }
        )
    }

    /// Get the wall-clock time in milliseconds, if available.
    pub fn wall_time_ms(&self) -> Option<u64> {
        match self {
            AgentLifecycle::Completed { wall_time_ms, .. } => Some(*wall_time_ms),
            AgentLifecycle::Errored { wall_time_ms, .. } => Some(*wall_time_ms),
            _ => None,
        }
    }

    /// Get tokens used, if available.
    pub fn tokens_used(&self) -> u64 {
        match self {
            AgentLifecycle::Active { tokens_used, .. } => *tokens_used,
            AgentLifecycle::Completed { tokens_used, .. } => *tokens_used,
            AgentLifecycle::Errored { tokens_used, .. } => *tokens_used,
            AgentLifecycle::Cancelled { tokens_used, .. } => *tokens_used,
            _ => 0,
        }
    }

    /// Get a human-readable summary of the lifecycle state.
    pub fn summary(&self) -> String {
        match self {
            AgentLifecycle::Registered { at_ms } => format!("registered at {}", at_ms),
            AgentLifecycle::Idle { since_ms } => format!("idle since {}", since_ms),
            AgentLifecycle::Active { phase, started_at_ms, tokens_used } => {
                format!("active ({:?}) since {}, tokens={}", phase, started_at_ms, tokens_used)
            }
            AgentLifecycle::Completed { result, tokens_used, wall_time_ms, .. } => {
                format!("completed: {} ({} tokens, {}ms)", result, tokens_used, wall_time_ms)
            }
            AgentLifecycle::Errored { error, tokens_used, wall_time_ms } => {
                format!("errored: {} ({} tokens, {}ms)", error, tokens_used, wall_time_ms)
            }
            AgentLifecycle::Cancelled { reason, .. } => {
                format!("cancelled: {}", reason)
            }
        }
    }
}

impl Default for AgentLifecycle {
    fn default() -> Self {
        Self::Registered {
            at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }
}

/// Helper to get current timestamp in milliseconds.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ── AgentLifecycleBuilder — convenient construction ───────────────────

/// Builder for constructing lifecycle states with automatic timing.
pub struct AgentLifecycleBuilder;

impl AgentLifecycleBuilder {
    /// Create a Registered state.
    pub fn registered() -> AgentLifecycle {
        AgentLifecycle::Registered { at_ms: now_ms() }
    }

    /// Create an Idle state.
    pub fn idle() -> AgentLifecycle {
        AgentLifecycle::Idle { since_ms: now_ms() }
    }

    /// Create an Active state.
    pub fn active(phase: AgentPhase) -> AgentLifecycle {
        AgentLifecycle::Active {
            phase,
            started_at_ms: now_ms(),
            tokens_used: 0,
        }
    }

    /// Create a Completed state with automatic timing.
    pub fn completed(result: String, tokens_used: u64, started_at: u64) -> AgentLifecycle {
        let wall_time = now_ms().saturating_sub(started_at);
        AgentLifecycle::Completed {
            result,
            tokens_used,
            wall_time_ms: wall_time,
            completed_at_ms: now_ms(),
        }
    }

    /// Create an Errored state with automatic timing.
    pub fn errored(error: String, tokens_used: u64, started_at: u64) -> AgentLifecycle {
        let wall_time = now_ms().saturating_sub(started_at);
        AgentLifecycle::Errored {
            error,
            tokens_used,
            wall_time_ms: wall_time,
        }
    }

    /// Create a Cancelled state.
    pub fn cancelled(reason: String, tokens_used: u64) -> AgentLifecycle {
        AgentLifecycle::Cancelled { reason, tokens_used }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_registered() {
        let lifecycle = AgentLifecycle::default();
        assert!(matches!(lifecycle, AgentLifecycle::Registered { .. }));
    }

    #[test]
    fn test_terminal_states() {
        let completed = AgentLifecycle::Completed {
            result: "done".into(),
            tokens_used: 100,
            wall_time_ms: 500,
            completed_at_ms: 1000,
        };
        let errored = AgentLifecycle::Errored {
            error: "fail".into(),
            tokens_used: 50,
            wall_time_ms: 200,
        };
        let cancelled = AgentLifecycle::Cancelled {
            reason: "timeout".into(),
            tokens_used: 30,
        };

        assert!(completed.is_terminal());
        assert!(errored.is_terminal());
        assert!(cancelled.is_terminal());

        assert!(!AgentLifecycle::Registered { at_ms: 0 }.is_terminal());
        assert!(!AgentLifecycle::Idle { since_ms: 0 }.is_terminal());
        assert!(!AgentLifecycle::Active {
            phase: AgentPhase::Planning,
            started_at_ms: 0,
            tokens_used: 0,
        }.is_terminal());
    }

    #[test]
    fn test_wall_time_ms() {
        let completed = AgentLifecycle::Completed {
            result: "done".into(),
            tokens_used: 100,
            wall_time_ms: 500,
            completed_at_ms: 1000,
        };
        assert_eq!(completed.wall_time_ms(), Some(500));

        let registered = AgentLifecycle::Registered { at_ms: 0 };
        assert_eq!(registered.wall_time_ms(), None);
    }

    #[test]
    fn test_tokens_used() {
        let active = AgentLifecycle::Active {
            phase: AgentPhase::Executing,
            started_at_ms: 0,
            tokens_used: 42,
        };
        assert_eq!(active.tokens_used(), 42);

        let registered = AgentLifecycle::Registered { at_ms: 0 };
        assert_eq!(registered.tokens_used(), 0);
    }

    #[test]
    fn test_builder() {
        let completed = AgentLifecycleBuilder::completed("ok".into(), 100, now_ms());
        assert!(completed.is_terminal());
        assert!(completed.wall_time_ms().is_some());

        let errored = AgentLifecycleBuilder::errored("fail".into(), 50, now_ms());
        assert!(errored.is_terminal());

        let cancelled = AgentLifecycleBuilder::cancelled("timeout".into(), 30);
        assert!(cancelled.is_terminal());
    }

    #[test]
    fn test_summary() {
        let summary = AgentLifecycle::Registered { at_ms: 1000 }.summary();
        assert!(summary.contains("registered"));

        let summary = AgentLifecycle::Completed {
            result: "done".into(),
            tokens_used: 100,
            wall_time_ms: 500,
            completed_at_ms: 1500,
        }.summary();
        assert!(summary.contains("completed"));
        assert!(summary.contains("done"));
    }

    #[test]
    fn test_agent_phase_variants() {
        assert_ne!(AgentPhase::Planning, AgentPhase::Executing);
        assert_ne!(AgentPhase::Reflecting, AgentPhase::Waiting);
        assert_eq!(AgentPhase::Planning as u8, 0);
    }
}
