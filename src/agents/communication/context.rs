//! ForkContext — parent-to-child context inheritance (BLUE70 §3.4, §6)
//!
//! Captures a snapshot of the parent agent's runtime state so child agents
//! inherit relevant context: conversation summary, active principles,
//! allowed file paths, and inherited memories.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::agents::communication::path::AgentPath;

// ── ForkContext ───────────────────────────────────────────────────

/// Context snapshot: child agents inherit the parent agent's runtime state.
///
/// Design notes:
/// - Parent path is preserved for lineage tracing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkContext {
    /// Parent agent's path in the agent tree.
    pub parent_path: AgentPath,
    /// Conversation history summary (last N rounds).
    pub conversation_summary: Option<String>,
    /// Active PUA rules (principles).
    pub principles: Vec<String>,
    /// Restricted file system base directory.
    pub allowed_base_dir: Option<PathBuf>,
    /// Inherited memory summaries (not full vector embeddings).
    pub inherited_memories: Vec<String>,
}

impl ForkContext {
    /// Create a new ForkContext with the given parent path.
    pub fn new(parent_path: AgentPath) -> Self {
        Self {
            parent_path,
            conversation_summary: None,
            principles: Vec::new(),
            allowed_base_dir: None,
            inherited_memories: Vec::new(),
        }
    }

    /// Set conversation summary.
    pub fn with_conversation_summary(mut self, summary: String) -> Self {
        self.conversation_summary = Some(summary);
        self
    }

    /// Add a principle (PUA rule).
    pub fn add_principle(mut self, principle: &str) -> Self {
        self.principles.push(principle.to_string());
        self
    }

    /// Set allowed base directory.
    pub fn with_allowed_base_dir(mut self, dir: PathBuf) -> Self {
        self.allowed_base_dir = Some(dir);
        self
    }

    /// Add an inherited memory.
    pub fn add_memory(mut self, memory: &str) -> Self {
        self.inherited_memories.push(memory.to_string());
        self
    }

    /// Whether this context has any inheritable state.
    pub fn is_empty(&self) -> bool {
        self.conversation_summary.is_none()
            && self.principles.is_empty()
            && self.allowed_base_dir.is_none()
            && self.inherited_memories.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fork_context_new() {
        let parent = AgentPath::parse("root/research").unwrap();
        let ctx = ForkContext::new(parent.clone());
        assert_eq!(ctx.parent_path, parent);
        assert!(ctx.is_empty());
    }

    #[test]
    fn test_fork_context_with_values() {
        let ctx = ForkContext::new(AgentPath::parse("root").unwrap())
            .with_conversation_summary("Research completed".to_string())
            .add_principle("be concise")
            .add_principle("use evidence")
            .add_memory("user prefers rust");

        assert!(!ctx.is_empty());
        assert_eq!(
            ctx.conversation_summary.as_deref(),
            Some("Research completed")
        );
        assert_eq!(ctx.principles.len(), 2);
        assert_eq!(ctx.inherited_memories.len(), 1);
    }
}
