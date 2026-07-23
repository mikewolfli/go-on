//! ForkContext — parent-to-child context inheritance (BLUE70 §3.4, §6)
//!
//! Captures a snapshot of the parent agent's runtime state so child agents
//! inherit relevant context: conversation summary, active principles,
//! allowed file paths, inherited memories, and KV cache fingerprints.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::agents::communication::path::AgentPath;

/// Context snapshot: child agents inherit the parent agent's runtime state.
///
/// Design notes:
/// - Parent path is preserved for lineage tracing.
/// - KV cache fingerprint uses a generic interface (not DeepSeek-specific).
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
    /// KV cache fingerprint — generic interface for cache reuse.
    pub kv_cache_fingerprint: Option<String>,
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
            kv_cache_fingerprint: None,
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

    /// Set KV cache fingerprint.
    pub fn with_kv_cache_fingerprint(mut self, fingerprint: String) -> Self {
        self.kv_cache_fingerprint = Some(fingerprint);
        self
    }

    /// Whether this context has any inheritable state.
    pub fn is_empty(&self) -> bool {
        self.conversation_summary.is_none()
            && self.principles.is_empty()
            && self.allowed_base_dir.is_none()
            && self.inherited_memories.is_empty()
            && self.kv_cache_fingerprint.is_none()
    }
}

/// Generic trait for KV cache providers.
///
/// Allows any model provider to participate in cache reuse:
/// - DeepSeekProvider → native prefix caching
/// - AnthropicProvider → Prompt Caching API
/// - CacheBlendProvider → CacheBlend technique
pub trait KvCacheProvider: Send + Sync {
    /// Get the current KV cache fingerprint, if available.
    fn cache_fingerprint(&self) -> Option<String>;

    /// Try to attach to a cached prefix by fingerprint.
    /// Returns true if the cache was successfully attached.
    fn try_attach_cache(&self, fingerprint: &str) -> bool;
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
            .add_memory("user prefers rust")
            .with_kv_cache_fingerprint("fp_abc123".to_string());

        assert!(!ctx.is_empty());
        assert_eq!(ctx.conversation_summary.as_deref(), Some("Research completed"));
        assert_eq!(ctx.principles.len(), 2);
        assert_eq!(ctx.inherited_memories.len(), 1);
        assert_eq!(ctx.kv_cache_fingerprint.as_deref(), Some("fp_abc123"));
    }

    #[test]
    fn test_kv_cache_provider_trait_object() {
        // Compile-time check: KvCacheProvider can be used as a trait object.
        fn accepts_provider(_p: &dyn KvCacheProvider) {}
        // This test passes if it compiles.
        assert!(true);
    }
}
