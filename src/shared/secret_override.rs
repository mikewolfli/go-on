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
//! ```ignore
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
    if let Ok(mut map) = SECRET_OVERRIDE_MAP.lock() {
        map.insert(key.to_string(), value.to_string());
    }
}

/// Remove a secret override (restore env-var fallback).
#[allow(dead_code)] // Public API — reserved for credential rotation flows
pub fn remove_secret_override(key: &str) {
    if let Ok(mut map) = SECRET_OVERRIDE_MAP.lock() {
        map.remove(key);
    }
}

/// Resolve a secret value: returns the in-memory override if set, otherwise
/// falls back to `std::env::var(key)`.
pub fn get_secret(key: &str) -> Option<String> {
    if let Ok(map) = SECRET_OVERRIDE_MAP.lock() {
        if let Some(value) = map.get(key) {
            return Some(value.clone());
        }
    }
    std::env::var(key).ok()
}

/// Returns `true` if the given key has an in-memory override set.
#[allow(dead_code)] // Public API — reserved for diagnostic use
pub fn has_override(key: &str) -> bool {
    if let Ok(map) = SECRET_OVERRIDE_MAP.lock() {
        map.contains_key(key)
    } else {
        false
    }
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
    if let Ok(cache) = KEYRING_CACHE.lock() {
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
        if let Ok(mut cache) = KEYRING_CACHE.lock() {
            cache.insert(
                key,
                CachedEntry {
                    value: v.clone(),
                    fetched_at: now,
                },
            );
        }
    }

    value
}

/// Invalidate a cached keyring entry.
#[allow(dead_code)] // Public API — reserved for credential update flows
pub fn invalidate_keyring_cache(service: &str, account: &str) {
    let key = (service.to_string(), account.to_string());
    if let Ok(mut cache) = KEYRING_CACHE.lock() {
        cache.remove(&key);
    }
}

/// Invalidate all cached keyring entries.
#[allow(dead_code)] // Public API — reserved for credential reset flows
pub fn clear_keyring_cache() {
    if let Ok(mut cache) = KEYRING_CACHE.lock() {
        cache.clear();
    }
}

/// Returns the number of overrides currently stored.
#[allow(dead_code)] // Public API — reserved for diagnostic use
pub fn override_count() -> usize {
    if let Ok(map) = SECRET_OVERRIDE_MAP.lock() {
        map.len()
    } else {
        0
    }
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
        remove_secret_override("GO_ON_TEST_OVERRIDE");
        assert_eq!(get_secret("GO_ON_TEST_OVERRIDE"), None);
    }

    #[test]
    fn test_get_falls_back_to_env() {
        // This key is NOT overridden, so get_secret should return None
        // (since it's not set in the real env either).
        assert_eq!(get_secret("GO_ON_TEST_NONEXISTENT_KEY_XYZ"), None);
    }

    #[test]
    fn test_remove_nonexistent_key_is_safe() {
        // Should not panic.
        remove_secret_override("GO_ON_TEST_NONEXISTENT_KEY_XYZ");
    }

    #[test]
    fn test_overrides_are_independent() {
        set_secret_override("KEY_A", "value-a");
        set_secret_override("KEY_B", "value-b");
        assert_eq!(get_secret("KEY_A"), Some("value-a".to_string()));
        assert_eq!(get_secret("KEY_B"), Some("value-b".to_string()));
        remove_secret_override("KEY_A");
        assert_eq!(get_secret("KEY_A"), None);
        assert_eq!(get_secret("KEY_B"), Some("value-b".to_string()));
        remove_secret_override("KEY_B");
    }

    #[test]
    fn test_has_override() {
        assert!(!has_override("GO_ON_TEST_OVERRIDE_CHECK"));
        set_secret_override("GO_ON_TEST_OVERRIDE_CHECK", "x");
        assert!(has_override("GO_ON_TEST_OVERRIDE_CHECK"));
        remove_secret_override("GO_ON_TEST_OVERRIDE_CHECK");
        assert!(!has_override("GO_ON_TEST_OVERRIDE_CHECK"));
    }

    #[test]
    fn test_override_count() {
        // Use unique keys to avoid interference from parallel tests.
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let k1 = format!("GO_ON_TEST_COUNT_A_{}", n);
        let k2 = format!("GO_ON_TEST_COUNT_B_{}", n);

        set_secret_override(&k1, "a");
        set_secret_override(&k2, "b");
        assert!(has_override(&k1));
        assert!(has_override(&k2));
        remove_secret_override(&k1);
        remove_secret_override(&k2);
        // Clean up: ensure our keys are gone
        assert!(!has_override(&k1));
        assert!(!has_override(&k2));
    }
}
