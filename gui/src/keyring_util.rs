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
/// Rationale:
///   - .env files are world-readable on Unix (0755 parent dir)
///   - Process environment is visible via /proc/PID/environ to any user
///   - Keyring provides OS-level encryption at rest (Keychain, libsecret,
///     Credential Manager)
///
/// # macOS keychain prompt elimination
///
/// macOS keychain access prompts are **per-item**: accessing 36 provider entries
/// (each with api_key + secret_key) would trigger **72 individual dialogs**.
/// To avoid this, `KEYRING_CACHE` stores all read results in memory so each
/// keychain item is accessed exactly **once** per process lifetime.
use anyhow::Result;
use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex};

/// In-memory cache of keychain read results.
/// Key: `"go-on/{account}"`, Value: `Some(key)` if found, `None` if not in keychain.
///
/// ## Why this cache exists (DO NOT REMOVE)
///
/// macOS keychain access prompts are **per-item per-process**. The project has
/// 36+ canonical provider names. Without this cache, every call to
/// `get_api_key()` or `get_secret_key()` for each provider triggers a separate
/// macOS keychain dialog — that's **72+ popups** asking the user to allow
/// keychain access. With the cache, each keychain item is accessed exactly
/// **once** per process lifetime.
///
/// ## Usage rules
///
/// 1. **ALWAYS** call `get_api_key()` / `get_secret_key()` — never call
///    `platform::get_api_key()` directly, which bypasses the cache.
/// 2. After `store_*` or `delete_*`, `invalidate_cache()` is called
///    automatically so the next read fetches fresh data.
/// 3. If a new public read function is added, it MUST go through this cache.
static KEYRING_CACHE: LazyLock<Mutex<BTreeMap<String, Option<String>>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

/// Clear the in-memory keyring cache. Call this after storing/deleting a key
/// so subsequent reads reflect the new state.
///
/// Currently reserved for future/emergency use — `invalidate_cache()` handles
/// per-provider invalidation automatically during normal store/delete operations.
#[expect(
    dead_code,
    reason = "exposed as public API for future/emergency use; normal store/delete uses invalidate_cache()"
)]
pub fn clear_keyring_cache() {
    if let Ok(mut cache) = KEYRING_CACHE.lock() {
        cache.clear();
    }
}

// ╔══════════════════════════════════════════════════════════════════════════╗
// ║  macOS: keyring crate + security CLI for ACL                             ║
// ║                                                                          ║
// ║  IMPORTANT: DO NOT REMOVE this platform block!                            ║
// ║                                                                          ║
// ║  The keyring crate stores/reads passwords via the macOS Keychain.        ║
// ║  HOWEVER, by default macOS restricts keychain item access to the         ║
// ║  creating process only. When our background backend process tries to     ║
// ║  read the keychain item, macOS silently denies access because no         ║
// ║  visible dialog can be shown to the user for permission approval.        ║
// ║                                                                          ║
// ║  Without `ensure_item_accessible()`, the result is:                      ║
// ║    - Keyring write succeeds                                              ║
// ║    - Keyring read from the same binary also fails (!)                     ║
// ║    - The backend shows "deepseek: not ready"                            ║
// ║    - User sees "API key missing" even though it was just stored          ║
// ║                                                                          ║
// ║  The fix: after writing via `keyring::Entry::set_password()`, run:       ║
// ║    security set-key-partition-list -S apple-tool:,apple: -k ""          ║
// ║      -D "go-on ({account})" login.keychain                              ║
// ║  This adds the standard system partition groups to the keychain item's   ║
// ║  ACL, allowing any process in those groups (including our headless       ║
// ║  backend) to read the password without prompting.                        ║
// ║                                                                          ║
// ║  OPTIMIZATION: accounts are batched and flushed once via                     ║
// ║  `flush_pending_acl_updates()` instead of running `security` once         ║
// ║  per key. This avoids N keychain password dialogs (one per provider).    ║
// ╚══════════════════════════════════════════════════════════════════════════╝
#[cfg(target_os = "macos")]
mod platform {
    use anyhow::Result;
    use std::process::Command;
    use std::sync::Mutex;

    /// Thread-safe queue of accounts whose keychain ACL needs updating.
    /// Batched and flushed by `flush_pending_acl_updates()` to avoid N keychain
    /// password dialogs (one per `security` invocation).
    static PENDING_ACL_ACCOUNTS: std::sync::LazyLock<Mutex<Vec<String>>> =
        std::sync::LazyLock::new(|| Mutex::new(Vec::new()));

    /// Queue an account for ACL update. The actual `security` command is deferred
    /// until `flush_pending_acl_updates()` is called.
    pub(crate) fn defer_ensure_item_accessible(account: &str) {
        if let Ok(mut pending) = PENDING_ACL_ACCOUNTS.lock() {
            if !pending.contains(&account.to_string()) {
                pending.push(account.to_string());
            }
        }
    }

    /// Flush all queued ACL updates in a single `security` invocation.
    /// This runs one command with all accounts as separate `-a` flags,
    /// prompting the user for their keychain password only ONCE.
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

        // Build a single `security set-key-partition-list` command with
        // multiple `-a` flags, one per account. This modifies ACL on all
        // queued keychain items in one invocation, requiring only ONE
        // keychain password prompt.
        let mut args: Vec<String> = vec![
            "set-key-partition-list".into(),
            "-S".into(),
            "apple:default,apple:toolbar,apple:unknown,apple:keychain:basic".into(),
            "-k".into(),
            String::new(), // empty keychain password (triggers dialog once)
            "-d".into(),
            "go-on".into(),
        ];
        for account in &accounts {
            args.push("-a".into());
            args.push(account.clone());
        }
        args.push("login.keychain".into());

        let _ = Command::new("security").args(&args).output();
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

/// Invalidate cached entries for a provider (both api_key and secret_key).
///
/// Called automatically by `store_*` and `delete_*` functions. If you add a
/// new function that modifies keychain state, call this to keep the cache
/// consistent.
///
/// ## Why not just clear the entire cache?
///
/// 1. Targeted invalidation preserves all OTHER providers' cached values,
///    avoiding redundant keychain prompts for them on the next read.
/// 2. Clearing the whole cache would re-prompt for every provider on the
///    next iteration — reverting the 72-dialog problem.
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
/// per process lifetime, avoiding 36+ redundant keychain dialogs.
pub fn get_api_key(provider: &str) -> Option<String> {
    let cache_key = format!("go-on/{}_api_key", provider);

    // Check in-memory cache first (avoids redundant keychain prompts)
    if let Ok(cache) = KEYRING_CACHE.lock() {
        if let Some(cached) = cache.get(&cache_key) {
            return cached.clone();
        }
    }

    // Cache miss — query the system keyring
    let result = platform::get_api_key(provider);

    // Cache the result (both Some and None) to avoid redundant prompts
    if let Ok(mut cache) = KEYRING_CACHE.lock() {
        cache.insert(cache_key, result.clone());
    }

    result
}

/// Retrieve a provider secret key from the system keyring with in-memory caching.
/// Same cache strategy as `get_api_key`.
pub fn get_secret_key(provider: &str) -> Option<String> {
    let cache_key = format!("go-on/{}_secret_key", provider);

    // Check in-memory cache first
    if let Ok(cache) = KEYRING_CACHE.lock() {
        if let Some(cached) = cache.get(&cache_key) {
            return cached.clone();
        }
    }

    // Cache miss — query the system keyring
    let result = platform::get_secret_key(provider);

    // Cache the result
    if let Ok(mut cache) = KEYRING_CACHE.lock() {
        cache.insert(cache_key, result.clone());
    }

    result
}

/// Check whether a key exists in the system keyring for the given provider.
/// Uses cached `get_api_key` internally, so no redundant keychain access.
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
/// F-GAP-48: Wired — called from providers/mod.rs when removing dual-auth providers.
pub fn delete_secret_key(provider: &str) -> Result<()> {
    platform::delete_secret_key(provider)?;
    invalidate_cache(provider);
    Ok(())
}

/// Flush all pending macOS Keychain ACL updates in a single batch.
///
/// On macOS, each `security set-key-partition-list` invocation can trigger a
/// keychain password dialog. Batching all pending accounts into one command
/// ensures the user only gets prompted once.
///
/// Safe to call on non-macOS platforms (no-op).
pub fn flush_pending_acl_updates() {
    #[cfg(target_os = "macos")]
    platform::flush_pending_acl_updates();
}

/// Delete the github_copilot_token alias from the system keyring.
/// Used when removing a Copilot provider to clean up the alternative keyring entry
/// that was created alongside copilot_api_key for backward compatibility.
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
    // First try: keyring lookup
    if let Some(key) = get_api_key(provider) {
        return Some(key);
    }
    // Fallback: use config-provided key from the settings
    config_key
        .filter(|k| !k.is_empty() && *k != REDACTED_API_KEY)
        .map(|k| k.to_string())
}
