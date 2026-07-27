//! ContextFragment — structured context injection system (BLUE71 §9)
//!
//! Provides a trait-based interface for injecting context fragments into
//! agent conversations. Each fragment is role-tagged, priority-ranked, and
//! token-budget-aware. The `FragmentRegistry` collects all fragments and
//! builds the final context according to priorities and budget.
//!
//! Architecture:
//! - `ContextFragment` trait — defines a single injectable context piece
//! - `FragmentRole` — System role
//! - `FragmentPriority` — Normal or High (budget aware)
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
}

// ---------------------------------------------------------------------------
// FragmentPriority — budget-aware inclusion priority
// ---------------------------------------------------------------------------

/// Priority of a context fragment — determines inclusion order under budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FragmentPriority {
    /// Normal priority — included unless budget is tight.
    Normal = 1,
    /// High priority — preferentially retained.
    High = 2,
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
    /// Role for this fragment (System).
    fn role(&self) -> FragmentRole;

    /// Priority for budget-aware inclusion.
    fn priority(&self) -> FragmentPriority;

    /// The text body of this fragment (called each time context is built).
    fn body(&self) -> String;
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

    /// Register a new fragment.
    pub fn register(&mut self, fragment: Arc<dyn ContextFragment>) {
        self.fragments.push(fragment);
    }

    /// Build the context string — fragments sorted by priority, then by role,
    /// respecting the character budget. High-priority fragments are always included.
    ///
    /// Returns the concatenated context string with role-prefixed sections.
    pub fn build_context(&self, budget: usize) -> String {
        // Sort by priority (ascending: Normal first, High last),
        // then by role (only System currently).
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

            // High-priority fragments are always included.
            // Other fragments are included only if budget allows.
            if fragment.priority() >= FragmentPriority::High || cost <= budget_remaining {
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str("[System]\n");
                result.push_str(&body);
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

    fn make_fragment(
        role: FragmentRole,
        priority: FragmentPriority,
        content: &str,
    ) -> Arc<dyn ContextFragment> {
        Arc::new(TestFragment {
            role,
            priority,
            content: content.to_string(),
        })
    }

    #[test]
    fn test_empty_registry() {
        let reg = FragmentRegistry::new();
        assert_eq!(reg.build_context(1000), "");
    }

    #[test]
    fn test_single_fragment() {
        let mut reg = FragmentRegistry::new();
        reg.register(make_fragment(
            FragmentRole::System,
            FragmentPriority::Normal,
            "Hello world",
        ));

        let context = reg.build_context(1000);
        assert!(context.contains("[System]"));
        assert!(context.contains("Hello world"));
    }

    #[test]
    fn test_priority_ordering() {
        let mut reg = FragmentRegistry::new();
        reg.register(make_fragment(
            FragmentRole::System,
            FragmentPriority::Normal,
            "normal priority",
        ));
        reg.register(make_fragment(
            FragmentRole::System,
            FragmentPriority::High,
            "high priority",
        ));

        let context = reg.build_context(1000);
        // Normal should appear before High (sorted ascending by priority)
        let normal_pos = context.find("normal").unwrap();
        let high_pos = context.find("high").unwrap();
        assert!(
            normal_pos < high_pos,
            "Normal priority should appear before High"
        );
    }

    #[test]
    fn test_budget_truncation() {
        let mut reg = FragmentRegistry::new();
        reg.register(make_fragment(
            FragmentRole::System,
            FragmentPriority::Normal,
            "short",
        ));
        reg.register(make_fragment(
            FragmentRole::System,
            FragmentPriority::Normal,
            "very long content that should be excluded due to budget",
        ));

        let context = reg.build_context(20);
        // "short" is 5 chars + tags, should fit in 20 chars
        assert!(context.contains("short"));
        // The long content should be excluded
        assert!(
            !context.contains("very long"),
            "long content should be excluded when budget is tight"
        );
    }

    #[test]
    fn test_high_always_included() {
        let mut reg = FragmentRegistry::new();
        reg.register(make_fragment(
            FragmentRole::System,
            FragmentPriority::High,
            "must include",
        ));
        reg.register(make_fragment(
            FragmentRole::System,
            FragmentPriority::Normal,
            "small",
        ));

        // Tiny budget — only High should be included
        let context = reg.build_context(5);
        assert!(
            context.contains("must include"),
            "High-priority must be included even with tiny budget"
        );
    }

    #[test]
    fn test_simple_fragment() {
        let fragment = SimpleFragment::new(
            FragmentRole::System,
            FragmentPriority::High,
            "system instructions".to_string(),
        );
        assert_eq!(fragment.role(), FragmentRole::System);
        assert_eq!(fragment.priority(), FragmentPriority::High);
        assert_eq!(fragment.body(), "system instructions");
    }
}
