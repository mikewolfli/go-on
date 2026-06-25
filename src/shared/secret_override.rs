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

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

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
    match SECRET_OVERRIDE_MAP.lock() {
        Ok(mut map) => {
            map.insert(key.to_string(), value.to_string());
        }
        Err(poisoned) => {
            tracing::warn!("SECRET_OVERRIDE_MAP mutex poisoned in set_secret_override");
            let mut map = poisoned.into_inner();
            map.insert(key.to_string(), value.to_string());
        }
    }
}

/// Resolve a secret value: returns the in-memory override if set, otherwise
/// falls back to `std::env::var(key)`.
pub fn get_secret(key: &str) -> Option<String> {
    let map = SECRET_OVERRIDE_MAP.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("lock poisoned, recovering");
        poisoned.into_inner()
    });
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

    // Check cache first.
    let cache = KEYRING_CACHE.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("lock poisoned, recovering");
        poisoned.into_inner()
    });
    if let Some(entry) = cache.get(&key) {
        if now.duration_since(entry.fetched_at) < KEYRING_CACHE_TTL {
            return Some(entry.value.clone());
        }
    }

    // Cache miss or expired — perform real keyring lookup.
    let value = keyring::Entry::new(service, account)
        .ok()
        .and_then(|e| e.get_password().ok())
        .filter(|v| !v.trim().is_empty());

    // Update cache (best-effort).
    if let Some(ref v) = value {
        let mut cache = KEYRING_CACHE.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
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
