pub mod alert_severity;
pub mod db_pool;
pub mod execution_recorder;
pub mod goon_paths;
pub mod http_client;
pub mod keyring_ref;
pub mod lock_utils;
pub mod math;
pub mod protocol_mode;
pub mod provenance_helpers;
pub mod role_types;
pub mod secret_override;
pub mod stdio;
pub mod tcp_accept_loop;
pub mod timestamps;
pub mod token_bucket;
pub mod token_estimator;
pub mod tool_descriptors;
pub mod vec_utils;

use std::collections::HashMap;

use serde_json::Value;

/// Read a boolean option from a serde JSON options map with a default.
pub fn option_bool(options: &HashMap<String, Value>, key: &str, default: bool) -> bool {
    options.get(key).and_then(Value::as_bool).unwrap_or(default)
}

/// Read a usize option from a serde JSON options map with a default.
pub fn option_usize(options: &HashMap<String, Value>, key: &str, default: usize) -> usize {
    options
        .get(key)
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(default)
}

/// Raw SHA-256 digest of `data` (single source; per-module copies removed).
pub fn sha256_bytes(data: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().to_vec()
}

/// Raw HMAC-SHA256 digest (RFC 2104) of `data` with the given `key`.
///
/// Single shared primitive for every compute-side HMAC use (token signing,
/// request signatures, etc.). Verification callers must keep their own
/// constant-time comparison (`verify_slice`) on top of this.
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    use hmac::{digest::KeyInit, Mac};
    use sha2::Sha256;
    let mut mac = hmac::Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Hex-encoded SHA-256 digest of `data`.
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(data);
    hash.iter()
        .map(|b| format!("{:02x}", b))
        .collect::<Vec<_>>()
        .concat()
}
