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

/// Hard-budget variant: the input is trimmed and the `...` ellipsis replaces
/// the last 3 characters of the budget, so the result is **at most**
/// `max_chars` characters. This is the exact semantics of the former local
/// `truncate_chars` in `acp/impl/chat/knowledge.rs` (kept here so all
/// truncation lives in one module).
pub fn truncate_chars_hard_budget(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let trimmed = text.trim();
    let mut result = trimmed.chars().take(max_chars).collect::<String>();
    if trimmed.chars().count() > max_chars && max_chars > 1 {
        let keep = max_chars.saturating_sub(3);
        result = trimmed.chars().take(keep).collect::<String>();
        result.push_str("...");
    }
    result
}
