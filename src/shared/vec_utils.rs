//! Shared vector utility functions.
//!
//! Eliminates byte-for-byte duplication of order-preserving dedup across
//! `governance/pua.rs` (strings + `AgentRole`) and `orchestration/task_router.rs`.

/// Remove duplicate items from a vector, preserving order (first occurrence
/// wins). Generic over the element type: `Vec<String>` (principles, safeguard
/// names) and `Vec<AgentRole>` (PUA mandatory roles) previously each had a
/// byte-identical copy.
pub fn dedupe<T: PartialEq>(values: &mut Vec<T>) {
    let mut deduped = Vec::new();
    for value in values.drain(..) {
        if !deduped.contains(&value) {
            deduped.push(value);
        }
    }
    *values = deduped;
}
