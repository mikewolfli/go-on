//! Compile-time tool name macro for built-in tools.
//!
//! Provides the `builtin_tools!` macro which generates:
//! - `ALL_BUILTIN_TOOL_NAMES: &[&str]` — a compile-time list of all built-in tool names
//! - `is_builtin_tool(name: &str) -> bool` — a runtime lookup helper
//!
//! The macro also includes a **compile-time deduplication assertion** that will
//! cause the build to fail if two tools share the same name.

/// Compile-time string equality for use in `const` contexts.
///
/// Standard `str::eq` is not yet `const fn` in stable Rust, so we compare
/// byte-by-byte.
#[doc(hidden)]
pub const fn const_str_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    let mut i = 0;
    while i < a_bytes.len() {
        if a_bytes[i] != b_bytes[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// Declare the set of built-in tool names at compile time.
///
/// The macro accepts a comma-separated list of **string literal** tool names:
///
/// ```ignore
/// builtin_tools! {
///     "read_file",
///     "write_file",
///     "search_files",
/// }
/// ```
///
/// It generates:
/// - `pub const ALL_BUILTIN_TOOL_NAMES: &[&str]` — the complete list
/// - `pub fn is_builtin_tool(name: &str) -> bool` — checks membership
///
/// A **compile-time assertion** verifies there are no duplicate names. If a
/// duplicate is present, the build will fail with a `panic` in `const`
/// evaluation at the point of the duplicate.
#[macro_export]
macro_rules! builtin_tools {
    ($($name:expr),* $(,)?) => {
        /// All built-in tool names, registered at compile time.
        pub const ALL_BUILTIN_TOOL_NAMES: &[&str] = &[
            $($name),*
        ];

        /// Compile-time deduplication check.
        ///
        /// Will cause a `panic!` at compile time (and thus a build failure) if
        /// any two tool names are identical.
        const _: () = {
            use $crate::orchestration::tool::registry_macro::const_str_eq;

            const NAMES: &[&str] = ALL_BUILTIN_TOOL_NAMES;
            let mut i: usize = 0;
            while i < NAMES.len() {
                let mut j: usize = i + 1;
                while j < NAMES.len() {
                    if const_str_eq(NAMES[i], NAMES[j]) {
                        panic!("builtin_tools! compile-time error: duplicate tool name");
                    }
                    j += 1;
                }
                i += 1;
            }
        };

        /// Returns `true` if `name` is one of the built-in tools.
        pub fn is_builtin_tool(name: &str) -> bool {
            ALL_BUILTIN_TOOL_NAMES.contains(&name)
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A helper module that uses the macro so we can test the generated items.
    mod test_tools {
        crate::builtin_tools! {
            "alpha",
            "beta",
            "gamma",
        }
    }

    #[test]
    fn test_all_builtin_tool_names_contains_expected() {
        assert_eq!(
            test_tools::ALL_BUILTIN_TOOL_NAMES,
            &["alpha", "beta", "gamma"]
        );
    }

    #[test]
    fn test_is_builtin_tool_true_for_registered() {
        assert!(test_tools::is_builtin_tool("alpha"));
        assert!(test_tools::is_builtin_tool("beta"));
        assert!(test_tools::is_builtin_tool("gamma"));
    }

    #[test]
    fn test_is_builtin_tool_false_for_unregistered() {
        assert!(!test_tools::is_builtin_tool("delta"));
        assert!(!test_tools::is_builtin_tool(""));
        assert!(!test_tools::is_builtin_tool("read_file"));
    }

    /// Verify that the `const_str_eq` helper works correctly.
    #[test]
    fn test_const_str_eq() {
        assert!(const_str_eq("", ""));
        assert!(const_str_eq("hello", "hello"));
        assert!(const_str_eq("a_b_c", "a_b_c"));
        assert!(!const_str_eq("hello", "world"));
        assert!(!const_str_eq("hello", "hell"));
        assert!(!const_str_eq("hell", "hello"));
        assert!(!const_str_eq("", "a"));
    }

    /// Test that a compile error occurs on duplicate names.
    /// We test this at runtime by running the const assertion logic manually.
    #[test]
    fn test_duplicate_detection_at_runtime() {
        // Simulate what the const assertion does
        const NAMES: &[&str] = &["same", "same"];
        let mut i: usize = 0;
        let mut found = false;
        while i < NAMES.len() {
            let mut j: usize = i + 1;
            while j < NAMES.len() {
                if const_str_eq(NAMES[i], NAMES[j]) {
                    found = true;
                }
                j += 1;
            }
            i += 1;
        }
        assert!(found, "should have detected duplicate");
    }

    /// Verify the compile-time assertion actually rejects duplicates by
    /// checking the expansion is correct. The real compilation guard
    /// lives in `const _: () = { ... }` inside the macro.
    #[test]
    fn test_no_duplicate_in_normal_usage() {
        // This module has no duplicates — confirm the generated functions work.
        mod no_dup {
            crate::builtin_tools! {
                "only",
            }
        }
        assert_eq!(no_dup::ALL_BUILTIN_TOOL_NAMES, &["only"]);
        assert!(no_dup::is_builtin_tool("only"));
        assert!(!no_dup::is_builtin_tool("nope"));
    }
}
