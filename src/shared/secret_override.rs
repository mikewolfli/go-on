//! Thread-safe secret override map
//!
//! Provides an in-memory key-value store for secrets as a replacement for
//! `std::env::set_var()`, which is documented as **undefined behavior** in
//! multi-threaded contexts.
//!
//! All access is guarded by a single `std::sync::Mutex`.  Lookups fall through
//! to `std::env::var()` when the key is not present in the override map, so
//! existing env-var-based code continues to work unchanged.
//!
//! # Usage
//!
//! ```text
//! use crate::shared::secret_override::{set_secret_override, get_secret};
//!
//! // Instead of:
//! //   std::env::set_var("GITHUB_TOKEN", "ghp_xxx");
//!
//! // Use:
//! set_secret_override("GITHUB_TOKEN", "ghp_xxx");
//!
//! // Read (fallback to env var):
//! let value = get_secret("GITHUB_TOKEN");
//! ```

use crate::lock_or_recover;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Result;

/// Validate the security of a secret string.
///
/// Single shared implementation — previously duplicated in
/// `agents/agent.rs` (inspect_secret_pool) and
/// `core/config/load/env_override.rs` (validate_secret_ref). The empty-value
/// error message key is caller-specific (`error.agent_empty_field` vs
/// `error.missing_field`), so it is passed in.
pub fn validate_secret_security(
    secret: &str,
    field_name: &str,
    empty_error_key: &str,
) -> Result<()> {
    if secret.trim().is_empty() {
        anyhow::bail!(
            "{}",
            crate::i18n::runtime::tf(empty_error_key, &[("field", field_name)])
        );
    }

    // Detect newline characters (possible multiline secret or injection attempt)
    if secret.contains('\n') || secret.contains('\r') {
        tracing::warn!(
            "{} contains newline characters, which may be a security issue",
            field_name
        );
    }

    // Check minimum secret length
    if secret.len() < 8 {
        tracing::warn!(
            "{} is very short ({} characters), which may be insecure",
            field_name,
            secret.len()
        );
    }

    // Detect common insecure patterns
    let insecure_patterns = [
        ("password", "contains the word 'password'"),
        ("123456", "contains simple numeric sequence"),
        ("admin", "contains the word 'admin'"),
        ("test", "contains the word 'test'"),
        ("secret", "contains the word 'secret'"),
    ];

    let secret_lower = secret.to_lowercase();
    for (pattern, description) in insecure_patterns {
        if secret_lower.contains(pattern) {
            tracing::warn!(
                "{} {} - consider using a stronger secret",
                field_name,
                description
            );
        }
    }

    Ok(())
}

/// In-memory secret override map.
///
/// Keys are environment variable names (e.g. `"GITHUB_TOKEN"`).
/// Values are the opaque secret strings.
static SECRET_OVERRIDE_MAP: std::sync::LazyLock<Mutex<HashMap<String, String>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Set a secret override that `get_secret` will return instead of reading
/// the real environment variable.
///
/// This is the thread-safe replacement for `std::env::set_var(key, value)`.
pub fn set_secret_override(key: &str, value: &str) {
    let mut map = lock_or_recover!(SECRET_OVERRIDE_MAP);
    map.insert(key.to_string(), value.to_string());
}

/// Resolve a secret value: returns the in-memory override if set, otherwise
/// falls back to `std::env::var(key)`.
pub fn get_secret(key: &str) -> Option<String> {
    let map = lock_or_recover!(SECRET_OVERRIDE_MAP);
    if let Some(value) = map.get(key) {
        return Some(value.clone());
    }
    std::env::var(key).ok()
}

// ── Keyring cache ──────────────────────────────────────────────────

/// Cached keyring entry with a TTL.
struct CachedEntry {
    value: String,
    fetched_at: Instant,
}

/// Thread-safe cache for keyring lookups.
///
/// Keyring operations (`keyring::Entry::get_password()`) are blocking I/O
/// that must not be called from async contexts without `spawn_blocking`.
/// This cache stores recently-fetched values so that hot-path lookups
/// (e.g. every chat request) avoid touching the system keyring.
static KEYRING_CACHE: std::sync::LazyLock<Mutex<HashMap<(String, String), CachedEntry>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Default TTL for cached keyring entries (30 seconds).
const KEYRING_CACHE_TTL: Duration = Duration::from_secs(30);

/// Fetch a secret from the system keyring with caching.
///
/// Returns `None` if the entry does not exist or cannot be read.
pub fn get_keyring_cached(service: &str, account: &str) -> Option<String> {
    let key = (service.to_string(), account.to_string());
    let now = Instant::now();

    // Check cache first. The guard is scoped to this block so it is dropped
    // before the keyring lookup below — `std::sync::Mutex` is not reentrant
    // and re-locking it on the miss path deadlocks the calling thread forever.
    {
        let cache = lock_or_recover!(KEYRING_CACHE);
        if let Some(entry) = cache.get(&key) {
            if now.duration_since(entry.fetched_at) < KEYRING_CACHE_TTL {
                return Some(entry.value.clone());
            }
        }
    }

    // Cache miss or expired — perform real keyring lookup.
    let value = keyring::Entry::new(service, account)
        .ok()
        .and_then(|e| e.get_password().ok())
        .filter(|v| !v.trim().is_empty());

    // Update cache (best-effort).
    if let Some(ref v) = value {
        let mut cache = lock_or_recover!(KEYRING_CACHE);
        cache.insert(
            key,
            CachedEntry {
                value: v.clone(),
                fetched_at: now,
            },
        );
    }

    value
}

/// Fetch a secret from the system keyring from an async context.
///
/// The keyring backend (e.g. secret-service over D-Bus) performs blocking
/// pipe/socket I/O. Calling it directly on a tokio worker starves the runtime
/// and, worse, prevents clean process shutdown (the runtime drop waits for the
/// blocked worker forever). Use this wrapper instead of the sync function
/// whenever the caller is async.
pub async fn get_keyring_cached_async(service: &str, account: &str) -> Option<String> {
    let service = service.to_string();
    let account = account.to_string();
    tokio::task::spawn_blocking(move || get_keyring_cached(&service, &account))
        .await
        .unwrap_or(None)
}

/// Store a secret in the system keyring from an async context.
///
/// The keyring backend performs blocking pipe/socket I/O on write as well as
/// read (see `get_keyring_cached_async`); calling `set_password` directly on a
/// tokio worker starves the runtime. Use this wrapper from async callers.
/// Returns the underlying `keyring::Error` on failure so callers can surface
/// the exact reason (credential store unavailable, locked, etc.).
pub async fn set_keyring_async(
    service: &str,
    account: &str,
    password: &str,
) -> Result<(), keyring::Error> {
    let service = service.to_string();
    let account = account.to_string();
    let password = password.to_string();
    tokio::task::spawn_blocking(move || {
        keyring::Entry::new(&service, &account)?.set_password(&password)
    })
    .await
    .unwrap_or_else(|join_err| {
        tracing::warn!("set_keyring_async: spawn_blocking join failed: {join_err}");
        Err(keyring::Error::NoEntry)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_and_get() {
        set_secret_override("GO_ON_TEST_OVERRIDE", "test-value-123");
        assert_eq!(
            get_secret("GO_ON_TEST_OVERRIDE"),
            Some("test-value-123".to_string())
        );
    }
}
