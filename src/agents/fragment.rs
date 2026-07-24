//! ContextFragment — structured context injection system (BLUE71 §9)
//!
//! Provides a trait-based interface for injecting context fragments into
//! agent conversations. Each fragment is role-tagged, priority-ranked, and
//! token-budget-aware. The `FragmentRegistry` collects all fragments and
//! builds the final context according to priorities and budget.
//!
//! Architecture:
//! - `ContextFragment` trait — defines a single injectable context piece
//! - `FragmentRole` — System, Developer, or User role
//! - `FragmentPriority` — Low, Normal, High, Critical (token budget aware)
//! - `FragmentRegistry` — collects fragments and builds prioritized context
//! - `BuiltinFragments` — built-in implementations for common use cases

use std::sync::Arc;

// ---------------------------------------------------------------------------
// FragmentRole — where the fragment appears in the message list
// ---------------------------------------------------------------------------

/// Role of a context fragment — determines where it appears in the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FragmentRole {
    /// Injected as a system message.
    System,
    /// Injected as a developer message (if supported by the model).
    Developer,
    /// Injected as a user message.
    User,
}

// ---------------------------------------------------------------------------
// FragmentPriority — budget-aware inclusion priority
// ---------------------------------------------------------------------------

/// Priority of a context fragment — determines inclusion order under budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FragmentPriority {
    /// Low priority — may be dropped under token pressure.
    Low = 0,
    /// Normal priority — included unless budget is tight.
    Normal = 1,
    /// High priority — preferentially retained.
    High = 2,
    /// Critical priority — must be included regardless of budget.
    Critical = 3,
}

// ---------------------------------------------------------------------------
// ContextFragment trait
// ---------------------------------------------------------------------------

/// A single injectable context fragment (BLUE71 §9.2).
///
/// Each fragment is role-tagged, priority-ranked, and produces text content
/// via `body()`. The `FragmentRegistry` collects all fragments and builds
/// the final context respecting priorities and token budget.
pub trait ContextFragment: Send + Sync {
    /// Role for this fragment (System, Developer, or User).
    fn role(&self) -> FragmentRole;

    /// Priority for budget-aware inclusion.
    fn priority(&self) -> FragmentPriority;

    /// The text body of this fragment (called each time context is built).
    fn body(&self) -> String;

    /// Relative weight for token budget calculations (default 1).
    fn weight(&self) -> u32 {
        1
    }
}

// ---------------------------------------------------------------------------
// FragmentRegistry — collects and builds context
// ---------------------------------------------------------------------------

/// Registry of context fragments that builds prioritized context (BLUE71 §9.2).
///
/// Usage:
/// ```
/// let mut registry = FragmentRegistry::new();
/// registry.register(Arc::new(MyFragment));
/// let context = registry.build_context(2000); // 2000 char budget
/// ```
pub struct FragmentRegistry {
    fragments: Vec<Arc<dyn ContextFragment>>,
}

impl FragmentRegistry {
    /// Create an empty fragment registry.
    pub fn new() -> Self {
        Self {
            fragments: Vec::new(),
        }
    }

    /// Create with an initial set of fragments.
    pub fn with_fragments(fragments: Vec<Arc<dyn ContextFragment>>) -> Self {
        Self { fragments }
    }

    /// Register a new fragment.
    pub fn register(&mut self, fragment: Arc<dyn ContextFragment>) {
        self.fragments.push(fragment);
    }

    /// Number of registered fragments.
    pub fn len(&self) -> usize {
        self.fragments.len()
    }

    /// Whether no fragments are registered.
    pub fn is_empty(&self) -> bool {
        self.fragments.is_empty()
    }

    /// Build the context string — fragments sorted by priority, then by role,
    /// respecting the character budget. Critical fragments are always included.
    ///
    /// Returns the concatenated context string with role-prefixed sections.
    pub fn build_context(&self, budget: usize) -> String {
        // Sort by priority (ascending: Low first, Critical last),
        // then by role (System first, User last).
        let mut sorted: Vec<&Arc<dyn ContextFragment>> = self.fragments.iter().collect();
        sorted.sort_by(|a, b| {
            let prio_cmp = a.priority().cmp(&b.priority());
            if prio_cmp != std::cmp::Ordering::Equal {
                return prio_cmp;
            }
            a.role().cmp(&b.role())
        });

        let mut result = String::new();
        let mut budget_remaining = budget;

        for fragment in sorted {
            let body = fragment.body();
            let cost = body.len();

            // Critical fragments are always included.
            // Other fragments are included only if budget allows.
            if fragment.priority() >= FragmentPriority::High || cost <= budget_remaining {
                if !result.is_empty() {
                    result.push('\n');
                }
                let role_tag = match fragment.role() {
                    FragmentRole::System => "[System]",
                    FragmentRole::Developer => "[Developer]",
                    FragmentRole::User => "[User]",
                };
                result.push_str(role_tag);
                result.push('\n');
                result.push_str(&body);
                budget_remaining = budget_remaining.saturating_sub(cost);
            }
        }

        result
    }

    /// Build context as structured role-content pairs (for message construction).
    pub fn build_context_pairs(&self, budget: usize) -> Vec<(FragmentRole, String)> {
        let mut sorted: Vec<&Arc<dyn ContextFragment>> = self.fragments.iter().collect();
        sorted.sort_by(|a, b| {
            let prio_cmp = a.priority().cmp(&b.priority());
            if prio_cmp != std::cmp::Ordering::Equal {
                return prio_cmp;
            }
            a.role().cmp(&b.role())
        });

        let mut result = Vec::new();
        let mut budget_remaining = budget;

        for fragment in sorted {
            let body = fragment.body();
            let cost = body.len();

            if fragment.priority() >= FragmentPriority::High || cost <= budget_remaining {
                result.push((fragment.role(), body));
                budget_remaining = budget_remaining.saturating_sub(cost);
            }
        }

        result
    }
}

impl Default for FragmentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Built-in fragment implementations
// ---------------------------------------------------------------------------

/// A simple string-based fragment with fixed role and priority.
pub struct SimpleFragment {
    role: FragmentRole,
    priority: FragmentPriority,
    content: String,
}

impl SimpleFragment {
    /// Create a new simple fragment.
    pub fn new(role: FragmentRole, priority: FragmentPriority, content: String) -> Self {
        Self {
            role,
            priority,
            content,
        }
    }
}

impl ContextFragment for SimpleFragment {
    fn role(&self) -> FragmentRole {
        self.role
    }

    fn priority(&self) -> FragmentPriority {
        self.priority
    }

    fn body(&self) -> String {
        self.content.clone()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    struct TestFragment {
        role: FragmentRole,
        priority: FragmentPriority,
        content: String,
    }

    impl ContextFragment for TestFragment {
        fn role(&self) -> FragmentRole {
            self.role
        }
        fn priority(&self) -> FragmentPriority {
            self.priority
        }
        fn body(&self) -> String {
            self.content.clone()
        }
    }

    fn make_fragment(role: FragmentRole, priority: FragmentPriority, content: &str) -> Arc<dyn ContextFragment> {
        Arc::new(TestFragment {
            role,
            priority,
            content: content.to_string(),
        })
    }

    #[test]
    fn test_empty_registry() {
        let reg = FragmentRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert_eq!(reg.build_context(1000), "");
        assert!(reg.build_context_pairs(1000).is_empty());
    }

    #[test]
    fn test_single_fragment() {
        let mut reg = FragmentRegistry::new();
        reg.register(make_fragment(FragmentRole::System, FragmentPriority::Normal, "Hello world"));
        assert_eq!(reg.len(), 1);

        let context = reg.build_context(1000);
        assert!(context.contains("[System]"));
        assert!(context.contains("Hello world"));
    }

    #[test]
    fn test_priority_ordering() {
        let mut reg = FragmentRegistry::new();
        reg.register(make_fragment(FragmentRole::System, FragmentPriority::Low, "low priority"));
        reg.register(make_fragment(FragmentRole::System, FragmentPriority::Critical, "critical priority"));
        reg.register(make_fragment(FragmentRole::System, FragmentPriority::Normal, "normal priority"));

        let context = reg.build_context(1000);
        // Critical should appear last (sorted ascending by priority)
        let critical_pos = context.find("critical").unwrap();
        let low_pos = context.find("low").unwrap();
        assert!(low_pos < critical_pos, "Low priority should appear before Critical");
    }

    #[test]
    fn test_budget_truncation() {
        let mut reg = FragmentRegistry::new();
        reg.register(make_fragment(FragmentRole::System, FragmentPriority::Normal, "short"));
        reg.register(make_fragment(FragmentRole::System, FragmentPriority::Normal, "very long content that should be excluded due to budget"));

        let context = reg.build_context(20);
        // "short" is 5 chars + tags, should fit in 20 chars
        assert!(context.contains("short"));
        // The long content should be excluded
        assert!(!context.contains("very long"),
            "long content should be excluded when budget is tight");
    }

    #[test]
    fn test_critical_always_included() {
        let mut reg = FragmentRegistry::new();
        reg.register(make_fragment(FragmentRole::System, FragmentPriority::Critical, "must include"));
        reg.register(make_fragment(FragmentRole::System, FragmentPriority::Low, "small"));

        // Tiny budget — only Critical should be included
        let context = reg.build_context(5);
        assert!(context.contains("must include"), "Critical must be included even with tiny budget");
    }

    #[test]
    fn test_build_context_pairs() {
        let mut reg = FragmentRegistry::new();
        reg.register(make_fragment(FragmentRole::System, FragmentPriority::Normal, "system content"));
        reg.register(make_fragment(FragmentRole::User, FragmentPriority::Normal, "user content"));

        let pairs = reg.build_context_pairs(1000);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, FragmentRole::System);
        assert!(pairs[0].1.contains("system"));
        assert_eq!(pairs[1].0, FragmentRole::User);
        assert!(pairs[1].1.contains("user"));
    }

    #[test]
    fn test_simple_fragment() {
        let fragment = SimpleFragment::new(
            FragmentRole::Developer,
            FragmentPriority::High,
            "developer instructions".to_string(),
        );
        assert_eq!(fragment.role(), FragmentRole::Developer);
        assert_eq!(fragment.priority(), FragmentPriority::High);
        assert_eq!(fragment.body(), "developer instructions");
        assert_eq!(fragment.weight(), 1);
    }

    #[test]
    fn test_in_use_fragment_with_pairs_respects_budget() {
        let mut reg = FragmentRegistry::new();
        reg.register(make_fragment(FragmentRole::System, FragmentPriority::Low, "aaaa")); // 4 chars
        reg.register(make_fragment(FragmentRole::System, FragmentPriority::Critical, "bbbb")); // 4 chars, always included

        // Budget of 10 should include Low (4 chars + tags) but not exceed
        let pairs = reg.build_context_pairs(10);
        // Critical is always included
        assert!(pairs.iter().any(|(_, c)| c == "bbbb"));
        // Low may or may not be included depending on exact cost calculation
        assert_eq!(pairs.len(), 2, "Both fragments should fit in 10 char budget");
    }
}
