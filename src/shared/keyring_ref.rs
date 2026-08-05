//! Shared secret-reference constants.
//!
//! The `keyring://` prefix is the canonical way to reference a system-keyring
//! secret in go-on configuration. All secret-reference parsing must use this
//! constant instead of re-declaring the literal — see `agents::agent` and
//! `acp::helpers::planning::context`.

/// Prefix marking a value as a system-keyring secret reference.
///
/// Format: `keyring://<service>/<account>`.
pub const KEYRING_PREFIX: &str = "keyring://";
