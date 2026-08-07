//! ContextForker — parent-to-child context inheritance orchestrator (BLUE70 §6)
//!
//! Collects parent agent runtime state and creates ForkContext snapshots
//! that child agents inherit: conversation summary, active principles,
//! allowed file paths, inherited memories, and KV cache fingerprints.
//!
//! Architecture:
//! - `ContextForker::fork()` is the single entry point for context inheritance.
//! - Integrates with `KvCacheProvider` for model-specific cache reuse.
//! - Produces `ForkContext` instances consumed by `SpawnAgentTool`.

use std::path::PathBuf;
use std::sync::Arc;

#[cfg(test)]
use crate::agents::communication::context::NoOpKvCacheProvider;
use crate::agents::communication::context::{ForkContext, KvCacheProvider};
use crate::agents::communication::path::AgentPath;

/// Context forking configuration.
#[derive(Debug, Clone)]
pub struct ForkOptions {
    /// Maximum conversation rounds to include in summary.
    pub max_summary_rounds: usize,
    /// Whether to inherit the parent's principles.
    pub inherit_principles: bool,
    /// Whether to inherit the parent's allowed base directory.
    pub inherit_base_dir: bool,
    /// Whether to inherit the parent's memory summaries.
    pub inherit_memories: bool,
    /// Whether to attempt KV cache reuse.
    pub try_kv_cache_reuse: bool,
}

impl Default for ForkOptions {
    fn default() -> Self {
        Self {
            max_summary_rounds: 10,
            inherit_principles: true,
            inherit_base_dir: true,
            inherit_memories: true,
            try_kv_cache_reuse: true,
        }
    }
}

/// Context inheritance orchestrator (BLUE70 §6).
///
/// Collects parent agent state and produces ForkContext snapshots
/// that child agents use to inherit relevant runtime context.
pub struct ContextForker {
    /// Optional KV cache provider for cache reuse.
    kv_cache_provider: Option<Arc<dyn KvCacheProvider>>,
    /// Default forking options.
    default_options: ForkOptions,
}

impl ContextForker {
    /// Create a new ContextForker.
    pub fn new() -> Self {
        Self {
            kv_cache_provider: None,
            default_options: ForkOptions::default(),
        }
    }

    /// Set the default forking options.
    pub fn with_default_options(mut self, options: ForkOptions) -> Self {
        self.default_options = options;
        self
    }

    /// Fork context from parent to child (BLUE70 §6.1).
    ///
    /// Collects parent agent state and produces a ForkContext snapshot.
    /// The `parent_context_fn` is a callback that provides the parent's
    /// actual runtime state (conversation summary, principles, etc.).
    pub fn fork<F>(
        &self,
        parent_path: &AgentPath,
        child_path: &AgentPath,
        parent_context_fn: F,
        options: Option<&ForkOptions>,
    ) -> ForkContext
    where
        F: FnOnce(&AgentPath) -> ParentContext,
    {
        let opts = options.unwrap_or(&self.default_options);
        let parent_ctx = parent_context_fn(parent_path);

        let mut ctx = ForkContext::new(child_path.clone());

        // Inherit conversation summary
        if let Some(summary) = parent_ctx.conversation_summary {
            ctx = ctx.with_conversation_summary(summary);
        }

        // Inherit principles
        if opts.inherit_principles {
            for p in &parent_ctx.principles {
                ctx = ctx.add_principle(p);
            }
        }

        // Inherit base directory
        if opts.inherit_base_dir {
            if let Some(dir) = parent_ctx.allowed_base_dir {
                ctx = ctx.with_allowed_base_dir(dir);
            }
        }

        // Inherit memory summaries
        if opts.inherit_memories {
            for m in &parent_ctx.memories {
                ctx = ctx.add_memory(m);
            }
        }

        // Attempt KV cache reuse (BLUE70 §6.2)
        if opts.try_kv_cache_reuse {
            if let Some(ref provider) = self.kv_cache_provider {
                let fingerprint = provider.cache_fingerprint();
                if let Some(ref fp) = fingerprint {
                    if provider.try_attach_cache(fp) {
                        ctx = ctx.with_kv_cache_fingerprint(fp.clone());
                    }
                }
            }
        }

        ctx
    }

    /// Quick fork with minimal options — no callback needed.
    /// Uses empty parent context and default options.
    pub fn quick_fork(&self, parent_path: &AgentPath, child_path: &AgentPath) -> ForkContext {
        let empty_ctx = ParentContext {
            conversation_summary: None,
            principles: Vec::new(),
            allowed_base_dir: None,
            memories: Vec::new(),
        };
        self.fork(parent_path, child_path, |_| empty_ctx, None)
    }
}

impl Default for ContextForker {
    fn default() -> Self {
        Self::new()
    }
}

/// Parent runtime context collected by the ContextForker.
#[derive(Debug, Clone, Default)]
pub struct ParentContext {
    /// Conversation history summary (last N rounds).
    pub conversation_summary: Option<String>,
    /// Active principles (PUA rules).
    pub principles: Vec<String>,
    /// Restricted file system base directory.
    pub allowed_base_dir: Option<PathBuf>,
    /// Inherited memory summaries.
    pub memories: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_forker_new() {
        let forker = ContextForker::new();
        let parent = AgentPath::parse("root").unwrap();
        let child = AgentPath::parse("root/research").unwrap();

        let ctx = forker.fork(
            &parent,
            &child,
            |_| ParentContext {
                conversation_summary: Some("Research completed".to_string()),
                ..Default::default()
            },
            None,
        );

        assert_eq!(ctx.parent_path, child);
        assert_eq!(
            ctx.conversation_summary.as_deref(),
            Some("Research completed")
        );
    }

    #[test]
    fn test_context_forker_inherits_principles() {
        let forker = ContextForker::new();
        let parent = AgentPath::parse("root").unwrap();
        let child = AgentPath::parse("root/reviewer").unwrap();

        let ctx = forker.fork(
            &parent,
            &child,
            |_| ParentContext {
                principles: vec!["be concise".to_string(), "use evidence".to_string()],
                ..Default::default()
            },
            None,
        );

        assert_eq!(ctx.principles.len(), 2);
        assert!(ctx.principles.contains(&"be concise".to_string()));
    }

    #[test]
    fn test_context_forker_inherits_memories() {
        let forker = ContextForker::new();
        let parent = AgentPath::parse("root").unwrap();
        let child = AgentPath::parse("root/coder").unwrap();

        let ctx = forker.fork(
            &parent,
            &child,
            |_| ParentContext {
                memories: vec!["user prefers rust".to_string()],
                ..Default::default()
            },
            None,
        );

        assert_eq!(ctx.inherited_memories.len(), 1);
        assert_eq!(ctx.inherited_memories[0], "user prefers rust");
    }

    #[test]
    fn test_quick_fork() {
        let forker = ContextForker::new();
        let parent = AgentPath::parse("root").unwrap();
        let child = AgentPath::parse("root/task").unwrap();

        let ctx = forker.quick_fork(&parent, &child);
        assert_eq!(ctx.parent_path, child);
        assert!(ctx.is_empty());
    }

    #[test]
    fn test_noop_kv_provider() {
        let provider = NoOpKvCacheProvider;
        assert!(provider.cache_fingerprint().is_none());
        assert!(!provider.try_attach_cache("test_fp"));
    }

    #[test]
    fn test_fork_options_dont_inherit() {
        let forker = ContextForker::new();
        let parent = AgentPath::parse("root").unwrap();
        let child = AgentPath::parse("root/isolated").unwrap();

        let opts = ForkOptions {
            inherit_principles: false,
            inherit_memories: false,
            ..Default::default()
        };

        let ctx = forker.fork(
            &parent,
            &child,
            |_| ParentContext {
                principles: vec!["should not appear".to_string()],
                memories: vec!["should not appear".to_string()],
                ..Default::default()
            },
            Some(&opts),
        );

        assert!(ctx.principles.is_empty());
        assert!(ctx.inherited_memories.is_empty());
    }
}
