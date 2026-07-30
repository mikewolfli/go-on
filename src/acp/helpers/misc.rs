//! Shared misc helpers for ACP — extracted from duplicated inline functions.
//!
//! Centralizes `extra_u64`, `extra_string`, and `extra_string_list` to
//! eliminate the byte-for-byte duplication between `acp/helpers/governance/policy.rs`
//! and `acp/impl/agent.rs`.

use crate::config::PhaseOptions;

/// Extract an optional `u64` value from the `extra` map of phase options.
pub fn extra_u64(options: Option<&PhaseOptions>, key: &str) -> Option<u64> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|v| v.as_u64())
}

/// Extract an optional `String` value from the `extra` map of phase options.
pub fn extra_string(options: Option<&PhaseOptions>, key: &str) -> Option<String> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
}

/// Extract an optional list of strings from the `extra` map of phase options.
pub fn extra_string_list(options: Option<&PhaseOptions>, key: &str) -> Option<Vec<String>> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
}
