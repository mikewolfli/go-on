// ╔══════════════════════════════════════════════════════════════════════════╗
// ║                                                                              ║
// ║  WARNING: Per-provider-per-key entries are BY DESIGN.                        ║
// ║                                                                              ║
// ║  Each provider's api_key is stored as a separate macOS Keychain entry         ║
// ║  (e.g. "go-on/openai_api_key", "go-on/deepseek_api_key"). This allows        ║
// ║  independent CRUD per provider and is simpler and more reliable than          ║
// ║  a single-JSON-blob approach.                                                 ║
// ║                                                                              ║
// ║  The in-memory KEYRING_CACHE ensures each keychain item is read at most       ║
// ║  ONCE per process lifetime, so after the initial N prompts, subsequent        ║
// ║  reads are instant.                                                           ║
// ║                                                                              ║
// ║  macOS keychain ACL is fixed after each write so the headless backend         ║
// ║  process can read keys without prompting. The ACL updates are batched         ║
// ║  via flush_pending_acl_updates() to trigger only ONE password dialog.         ║
// ║                                                                              ║
// ║  DO NOT consolidate into a single entry — that was tried and reverted.        ║
// ║  Per-provider entries are more reliable and maintainable.                     ║
// ║                                                                              ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

/// The placeholder used to redact API keys in config display (e.g. "********").
/// Shared across config loading, editor redaction, and UI preview logic.
pub const REDACTED_API_KEY: &str = "********";

/// Utility for storing and retrieving API keys via the system keyring ONLY.
///
/// Keyring entries use the format `go-on/{provider}_api_key`.
///
/// Uses the `keyring` crate on all platforms:
///   - **macOS**: Keychain (via `apple-native` feature)
///   - **Linux**: libsecret (Secret Service)
///   - **Windows**: Credential Manager
///
/// # SECURITY POLICY (DO NOT VIOLATE)
///
/// ALL API keys and tokens MUST be stored and retrieved exclusively via the
/// system keyring. NO .env files, NO environment variable fallbacks, NO
/// plaintext storage of any kind. This applies to ALL providers and the
/// Copilot token.
///
/// # macOS keychain prompt minimization
///
/// macOS keychain access prompts are **per-item per-process**: accessing 36
/// provider entries (each with api_key + secret_key) would trigger 72 individual
/// dialogs. To minimize this, `KEYRING_CACHE` stores all read results in memory
/// so each keychain item is accessed exactly **once** per process lifetime.
use anyhow::Result;
use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex};

/// In-memory cache of keychain read results.
/// Key: `"go-on/{account}"`, Value: `Some(key)` if found, `None` if not in keychain.
///
/// ## Why this cache exists (DO NOT REMOVE)
///
/// macOS keychain access prompts are **per-item per-process**. Without this cache,
/// every call to `get_api_key()` or `get_secret_key()` for each provider triggers
/// a separate macOS keychain dialog. With the cache, each keychain item is
/// accessed exactly **once** per process lifetime.
static KEYRING_CACHE: LazyLock<Mutex<BTreeMap<String, Option<String>>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

// ╔══════════════════════════════════════════════════════════════════════════╗
// ║  macOS: keyring crate + security CLI for ACL                             ║
// ║                                                                          ║
// ║  The keyring crate stores/reads passwords via the macOS Keychain.        ║
// ║  By default macOS restricts keychain item access to the creating         ║
// ║  process only. When our background backend process tries to read         ║
// ║  the keychain item, macOS silently denies access because no visible      ║
// ║  dialog can be shown to the user for permission approval.                ║
// ║                                                                          ║
// ║  The fix: after writing via keyring::Entry::set_password(), run:         ║
// ║    security set-key-partition-list ...                                    ║
// ║  This adds the standard system partition groups to the keychain item's   ║
// ║  ACL, allowing any process (including our headless backend) to read      ║
// ║  the password without prompting.                                          ║
// ║                                                                          ║
// ║  OPTIMIZATION: accounts are batched and flushed once via                  ║
// ║  flush_pending_acl_updates() instead of running security once per key.   ║
// ╚══════════════════════════════════════════════════════════════════════════╝
#[cfg(target_os = "macos")]
mod platform {
    use anyhow::Result;
    use std::process::Command;
    use std::sync::Mutex;

    static PENDING_ACL_ACCOUNTS: std::sync::LazyLock<Mutex<Vec<String>>> =
        std::sync::LazyLock::new(|| Mutex::new(Vec::new()));

    pub(crate) fn defer_ensure_item_accessible(account: &str) {
        if let Ok(mut pending) = PENDING_ACL_ACCOUNTS.lock() {
            if !pending.contains(&account.to_string()) {
                pending.push(account.to_string());
            }
        }
    }

    /// Flush all queued ACL updates in a single batch using `security add-generic-password`.
    /// Uses `-U` (update) with `-A` (allow all apps) to ensure headless backend
    /// processes can read the entries without prompting.
    /// Prompts the user for their keychain password only ONCE.
    pub(crate) fn flush_pending_acl_updates() {
        let accounts = {
            let mut pending = match PENDING_ACL_ACCOUNTS.lock() {
                Ok(g) => g,
                Err(poisoned) => {
                    eprintln!("[keyring_util] PENDING_ACL_ACCOUNTS lock poisoned, recovering");
                    poisoned.into_inner()
                }
            };
            std::mem::take(&mut *pending)
        };
        if accounts.is_empty() {
            return;
        }

        // First pass: read all passwords (sequential — avoid multiple keychain dialogs)
        let mut entries: Vec<(String, String)> = Vec::with_capacity(accounts.len());
        for account in &accounts {
            let entry = match keyring::Entry::new("go-on", account) {
                Ok(e) => e,
                Err(_) => continue,
            };
            if let Ok(password) = entry.get_password() {
                entries.push((account.clone(), password));
            }
        }

        // Second pass: run all `security add-generic-password -A` commands in parallel.
        // These are independent I/O operations, so parallel execution reduces total delay.
        // Uses std::thread::scope for safe shared borrowing of entries.
        let handles: Vec<_> = entries
            .iter()
            .map(|(account, password)| {
                let account = account.clone();
                let password = password.clone();
                std::thread::spawn(move || {
                    let _ = Command::new("security")
                        .args([
                            "add-generic-password",
                            "-U",
                            "-s",
                            "go-on",
                            "-a",
                            &account,
                            "-w",
                            &password,
                            "-A",
                            "login.keychain",
                        ])
                        .output();
                })
            })
            .collect();

        // Wait for all security commands to complete
        for h in handles {
            let _ = h.join();
        }
    }

    pub fn store_api_key(provider: &str, api_key: &str) -> Result<()> {
        let account = format!("{}_api_key", provider);
        let entry = keyring::Entry::new("go-on", &account)?;
        entry.set_password(api_key)?;
        defer_ensure_item_accessible(&account);
        Ok(())
    }

    pub fn store_secret_key(provider: &str, secret_key: &str) -> Result<()> {
        let account = format!("{}_secret_key", provider);
        let entry = keyring::Entry::new("go-on", &account)?;
        entry.set_password(secret_key)?;
        defer_ensure_item_accessible(&account);
        Ok(())
    }

    pub fn get_api_key(provider: &str) -> Option<String> {
        let account = format!("{}_api_key", provider);
        let entry = keyring::Entry::new("go-on", &account).ok()?;
        entry.get_password().ok()
    }

    pub fn get_secret_key(provider: &str) -> Option<String> {
        let account = format!("{}_secret_key", provider);
        let entry = keyring::Entry::new("go-on", &account).ok()?;
        entry.get_password().ok()
    }

    pub fn delete_api_key(provider: &str) -> Result<()> {
        let account = format!("{}_api_key", provider);
        let entry = keyring::Entry::new("go-on", &account)?;
        entry.delete_credential()?;
        Ok(())
    }

    pub fn delete_secret_key(provider: &str) -> Result<()> {
        let account = format!("{}_secret_key", provider);
        let entry = keyring::Entry::new("go-on", &account)?;
        entry.delete_credential()?;
        Ok(())
    }
}

// ── Linux / Windows: use the keyring crate only ───────────────────────────
#[cfg(not(target_os = "macos"))]
mod platform {
    use anyhow::Result;

    pub fn store_api_key(provider: &str, api_key: &str) -> Result<()> {
        let account = format!("{}_api_key", provider);
        let entry = keyring::Entry::new("go-on", &account)
            .map_err(|e| anyhow::anyhow!("failed to create keyring entry: {}", e))?;
        entry
            .set_password(api_key)
            .map_err(|e| anyhow::anyhow!("failed to save API key to system keyring: {}", e))?;
        Ok(())
    }

    pub fn store_secret_key(provider: &str, secret_key: &str) -> Result<()> {
        let account = format!("{}_secret_key", provider);
        let entry = keyring::Entry::new("go-on", &account)
            .map_err(|e| anyhow::anyhow!("failed to create keyring entry: {}", e))?;
        entry
            .set_password(secret_key)
            .map_err(|e| anyhow::anyhow!("failed to save secret key to system keyring: {}", e))?;
        Ok(())
    }

    pub fn get_api_key(provider: &str) -> Option<String> {
        let account = format!("{}_api_key", provider);
        let entry = keyring::Entry::new("go-on", &account).ok()?;
        entry.get_password().ok()
    }

    pub fn get_secret_key(provider: &str) -> Option<String> {
        let account = format!("{}_secret_key", provider);
        let entry = keyring::Entry::new("go-on", &account).ok()?;
        entry.get_password().ok()
    }

    pub fn delete_api_key(provider: &str) -> Result<()> {
        let account = format!("{}_api_key", provider);
        let entry = keyring::Entry::new("go-on", &account)?;
        entry.delete_credential()?;
        Ok(())
    }

    pub fn delete_secret_key(provider: &str) -> Result<()> {
        let account = format!("{}_secret_key", provider);
        let entry = keyring::Entry::new("go-on", &account)?;
        entry.delete_credential()?;
        Ok(())
    }
}

// ── Public API — delegates to the active platform module ──────────────────

fn invalidate_cache(provider: &str) {
    if let Ok(mut cache) = KEYRING_CACHE.lock() {
        let api_key = format!("go-on/{}_api_key", provider);
        let secret_key = format!("go-on/{}_secret_key", provider);
        cache.remove(&api_key);
        cache.remove(&secret_key);
    }
}

/// Store an API key in the system keyring.
pub fn store_api_key(provider: &str, api_key: &str) -> Result<()> {
    platform::store_api_key(provider, api_key)?;
    invalidate_cache(provider);
    Ok(())
}

/// Store a provider secret key in the system keyring.
pub fn store_secret_key(provider: &str, secret_key: &str) -> Result<()> {
    platform::store_secret_key(provider, secret_key)?;
    invalidate_cache(provider);
    Ok(())
}

/// Retrieve an API key from the system keyring with in-memory caching.
/// The cache ensures each macOS keychain item is accessed at most **once**
/// per process lifetime.
pub fn get_api_key(provider: &str) -> Option<String> {
    let cache_key = format!("go-on/{}_api_key", provider);

    if let Ok(cache) = KEYRING_CACHE.lock() {
        if let Some(cached) = cache.get(&cache_key) {
            return cached.clone();
        }
    }

    let result = platform::get_api_key(provider);

    if let Ok(mut cache) = KEYRING_CACHE.lock() {
        cache.insert(cache_key, result.clone());
    }

    result
}

/// Retrieve a provider secret key from the system keyring with in-memory caching.
pub fn get_secret_key(provider: &str) -> Option<String> {
    let cache_key = format!("go-on/{}_secret_key", provider);

    if let Ok(cache) = KEYRING_CACHE.lock() {
        if let Some(cached) = cache.get(&cache_key) {
            return cached.clone();
        }
    }

    let result = platform::get_secret_key(provider);

    if let Ok(mut cache) = KEYRING_CACHE.lock() {
        cache.insert(cache_key, result.clone());
    }

    result
}

/// Check whether a key exists in the system keyring for the given provider.
pub fn has_api_key(provider: &str) -> bool {
    get_api_key(provider).is_some()
}

/// Delete an API key from the system keyring (silent if missing).
pub fn delete_api_key(provider: &str) -> Result<()> {
    platform::delete_api_key(provider)?;
    invalidate_cache(provider);
    Ok(())
}

/// Delete a provider secret key from the system keyring (silent if missing).
pub fn delete_secret_key(provider: &str) -> Result<()> {
    platform::delete_secret_key(provider)?;
    invalidate_cache(provider);
    Ok(())
}

/// Flush all pending macOS Keychain ACL updates in a single batch.
/// On macOS, each `security set-key-partition-list` invocation can trigger a
/// keychain password dialog. Batching all pending accounts into one command
/// ensures the user only gets prompted once.
/// Safe to call on non-macOS platforms (no-op).
pub fn flush_pending_acl_updates() {
    #[cfg(target_os = "macos")]
    platform::flush_pending_acl_updates();
}

/// Delete the github_copilot_token alias from the system keyring.
pub fn delete_copilot_token() -> Result<()> {
    let account = "github_copilot_token";
    if let Ok(entry) = keyring::Entry::new("go-on", account) {
        let _ = entry.delete_credential();
    }
    Ok(())
}

/// Store the Copilot token to the github_copilot_token keyring entry.
pub fn store_copilot_token(token: &str) -> Result<()> {
    let account = "github_copilot_token";
    let entry = keyring::Entry::new("go-on", account)?;
    entry.set_password(token)?;
    #[cfg(target_os = "macos")]
    platform::defer_ensure_item_accessible(account);
    Ok(())
}

/// Retrieve an API key exclusively from the system keyring.
/// Does NOT fall back to config-provided keys to prevent secrets
/// from being stored in plaintext config files.
/// Returns `None` if the key is not found in the keyring.
pub fn get_api_key_with_fallback(provider: &str, config_key: Option<&str>) -> Option<String> {
    if let Some(key) = get_api_key(provider) {
        return Some(key);
    }
    config_key
        .filter(|k| !k.is_empty() && *k != REDACTED_API_KEY)
        .map(|k| k.to_string())
}
