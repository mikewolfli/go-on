//! Shared vector utility functions.
//!
//! Eliminates byte-for-byte duplication of `dedupe_strings` across
//! `governance/pua.rs` and `orchestration/task_router.rs`.

/// Remove duplicate strings from a vector, preserving order (first occurrence wins).
pub fn dedupe_strings(values: &mut Vec<String>) {
    let mut deduped = Vec::new();
    for value in values.drain(..) {
        if !deduped.contains(&value) {
            deduped.push(value);
        }
    }
    *values = deduped;
}
