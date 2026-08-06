pub mod alert_severity;
pub mod db_pool;
pub mod execution_recorder;
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
