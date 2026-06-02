//! Secret management sub-module (GAP-B53-23).
//!
//! Re-exports secret management functions from the parent `setup::mod.rs`.
//! Actual implementations:
//!   - `run_secret_command`
//!   - `parse_secret_action`
//!   - `detect_available_providers`
//!   - `keyring_target_for_env`
//!
//! TODO: Extract these functions from `super::mod.rs` into this file
//!       to reduce the size of mod.rs.

use anyhow::Result;

/// Resolve a secret reference (env var or keyring URI).
pub fn resolve_secret(value: &str) -> Result<String> {
    if value.starts_with("keyring://") {
        let account = value.trim_start_matches("keyring://");
        let entry = keyring::Entry::new("go-on", account).map_err(|e| {
            anyhow::anyhow!("failed to open keyring entry '{}': {}", account, e)
        })?;
        let secret = entry.get_password().map_err(|e| {
            anyhow::anyhow!("failed to read secret from keyring: {}", e)
        })?;
        Ok(secret)
    } else {
        std::env::var(value).map_err(|_| {
            anyhow::anyhow!(
                "environment variable '{}' is not set or empty",
                value
            )
        })
    }
}

/// Check if any secrets are available for the given provider.
pub fn has_provider_secret(provider_name: &str) -> bool {
    let env_name = format!("{}_API_KEY", provider_name.to_uppercase());
    std::env::var(&env_name).is_ok() || std::env::var(format!("GO_ON_{}", env_name)).is_ok()
}
