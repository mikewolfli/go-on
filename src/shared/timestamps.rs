//! Shared timestamp utilities.
//!
//! Extracted from `acp::prelude` to break the circular dependency:
//!   acp → observability → intelligence → acp
//!
//! Both `intelligence` and `acp` (and any other module) should use
//! these helpers instead of re-implementing or cross-importing.

use std::time::{SystemTime, UNIX_EPOCH};

/// Get current timestamp in seconds since Unix epoch.
pub fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Get current timestamp in milliseconds since Unix epoch.
pub fn now_ts_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Get current timestamp in milliseconds since Unix epoch as `u64`.
///
/// Single source for modules that need an unsigned millis timestamp
/// (previously each module re-declared its own `now_millis()` /
/// `current_timestamp_ms()` alias).
pub fn now_ts_ms_u64() -> u64 {
    now_ts_ms() as u64
}
