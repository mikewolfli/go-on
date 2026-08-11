//! Shared character-based text truncation helpers.
//!
//! Single source for "take at most N characters, append an ellipsis when the
//! input is longer" logic, which modules previously re-implemented inline
//! (`chars().take(n)` + `...`). Truncation is by character, so multi-byte
//! UTF-8 code points are never split.

/// Truncate `text` to at most `max_chars` characters (by character, never
/// splitting a UTF-8 code point), appending `ellipsis` when `text` has more
/// than `max_chars` characters. When `text` fits within the budget it is
/// returned unchanged. The result may exceed `max_chars` by the ellipsis
/// length (append-style semantics — callers that need a hard budget include
/// the ellipsis in `max_chars` themselves).
pub fn truncate_chars(text: &str, max_chars: usize, ellipsis: &str) -> String {
    let mut iter = text.chars();
    let truncated: String = iter.by_ref().take(max_chars).collect();
    if iter.next().is_some() {
        format!("{truncated}{ellipsis}")
    } else {
        truncated
    }
}
