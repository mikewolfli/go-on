/// The placeholder used to redact API keys in config display (e.g. "********").
/// Shared across config loading, editor redaction, and UI preview logic.
pub const REDACTED_API_KEY: &str = "********";

/// Utility for storing and retrieving API keys via the system keyring.
///
/// Keyring entries use the format `go-on/{provider}_api_key`.
///
/// Uses the `keyring` crate on all platforms:
///   - **macOS**: Keychain (via `apple-native` feature)
///   - **Linux**: libsecret (Secret Service)
///   - **Windows**: Credential Manager
///
/// The GUI also keeps `api_key` in `config.providers` as a fallback so that if the
/// system keyring is unavailable the key can still be injected into the backend
/// process environment at startup.
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

/// Store an API key in the system keyring AND sync to `.env` file.
/// Hybrid storage ensures the backend can read keys even in headless mode
/// (macOS Security.framework blocks background keychain access).
pub fn store_api_key(provider: &str, api_key: &str) -> Result<()> {
    platform::store_api_key(provider, api_key)?;
    invalidate_cache(provider);
    // Also sync to .env file for headless backend / Zed standalone
    let env_name = provider_to_env_name(provider, "api_key");
    if let Err(e) = sync_dotenv_key(&env_name, api_key) {
        eprintln!(
            "Warning: failed to sync API key to .env ({}): {}",
            env_name, e
        );
    }
    Ok(())
}

/// Store a provider secret key in the system keyring AND sync to `.env` file.
pub fn store_secret_key(provider: &str, secret_key: &str) -> Result<()> {
    platform::store_secret_key(provider, secret_key)?;
    invalidate_cache(provider);
    let env_name = provider_to_env_name(provider, "secret_key");
    if let Err(e) = sync_dotenv_key(&env_name, secret_key) {
        eprintln!(
            "Warning: failed to sync secret key to .env ({}): {}",
            env_name, e
        );
    }
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
    let result = platform::get_api_key(provider).or_else(|| {
        // Fallback: read from `.env` file
        let env_name = provider_to_env_name(provider, "api_key");
        let dotenv = read_dotenv();
        dotenv
            .get(&env_name)
            .map(|v| v.trim_matches('"').to_string())
    });

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
    let result = platform::get_secret_key(provider).or_else(|| {
        let env_name = provider_to_env_name(provider, "secret_key");
        let dotenv = read_dotenv();
        dotenv
            .get(&env_name)
            .map(|v| v.trim_matches('"').to_string())
    });

    // Cache the result
    if let Ok(mut cache) = KEYRING_CACHE.lock() {
        cache.insert(cache_key, result.clone());
    }

    result
}

/// Check whether a key exists in the system keyring or `.env` file for the given provider.
/// Uses cached `get_api_key` internally, so no redundant keychain access.
pub fn has_api_key(provider: &str) -> bool {
    get_api_key(provider).is_some()
}

/// Delete an API key from the system keyring AND `.env` file (silent if missing).
pub fn delete_api_key(provider: &str) -> Result<()> {
    platform::delete_api_key(provider)?;
    invalidate_cache(provider);
    let env_name = provider_to_env_name(provider, "api_key");
    let _ = remove_dotenv_key(&env_name);
    Ok(())
}

/// Delete a provider secret key from the system keyring AND `.env` file (silent if missing).
/// F-GAP-48: Wired — called from providers/mod.rs when removing dual-auth providers.
pub fn delete_secret_key(provider: &str) -> Result<()> {
    platform::delete_secret_key(provider)?;
    invalidate_cache(provider);
    let env_name = provider_to_env_name(provider, "secret_key");
    let _ = remove_dotenv_key(&env_name);
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

/// Delete the github_copilot_token alias from the system keyring AND `.env` file.
/// Used when removing a Copilot provider to clean up the alternative keyring entry
/// that was created alongside copilot_api_key for backward compatibility.
pub fn delete_copilot_token() -> Result<()> {
    let account = "github_copilot_token";
    if let Ok(entry) = keyring::Entry::new("go-on", account) {
        let _ = entry.delete_credential();
    }
    let _ = remove_dotenv_key("GITHUB_COPILOT_TOKEN");
    Ok(())
}

/// Store the Copilot token to the github_copilot_token keyring entry AND `.env`.
/// Hybrid storage ensures the backend / Zed can read it without keychain.
pub fn store_copilot_token(token: &str) -> Result<()> {
    let account = "github_copilot_token";
    let entry = keyring::Entry::new("go-on", account)?;
    entry.set_password(token)?;
    #[cfg(target_os = "macos")]
    platform::defer_ensure_item_accessible(account);
    // Also sync to .env
    if let Err(e) = sync_dotenv_key("GITHUB_COPILOT_TOKEN", token) {
        eprintln!("Warning: failed to sync Copilot token to .env: {}", e);
    }
    Ok(())
}

/// Retrieve an API key exclusively from the system keyring.
/// Does NOT fall back to config-provided keys to prevent secrets
/// from being stored in plaintext config files.
/// Returns `None` if the key is not found in the keyring.
pub fn get_api_key_with_fallback(provider: &str, config_key: Option<&str>) -> Option<String> {
    // First try: keyring lookup (with .env fallback already built into get_api_key)
    if let Some(key) = get_api_key(provider) {
        return Some(key);
    }
    // Fallback: use config-provided key from the settings
    config_key
        .filter(|k| !k.is_empty() && *k != REDACTED_API_KEY)
        .map(|k| k.to_string())
}

// ── .env file sync (Hybrid storage: keyring + dotenv) ────────────────────
//
// The backend (especially when run standalone by Zed) cannot reliably read
// the system keyring on macOS (blocking Security.framework dialog).
// As a fallback, we sync all API keys to a `.env` file next to the backend
// binary. The backend reads this file when keychain access times out.
//
// .env format:
//   DEEPSEEK_API_KEY=sk-xxx
//   OPENAI_API_KEY=sk-xxx
//   WENXIN_API_KEY=xxx
//   WENXIN_SECRET_KEY=xxx
//   GITHUB_COPILOT_TOKEN=ghu_xxx

/// Map a provider name and key type to the corresponding environment variable name.
fn provider_to_env_name(provider: &str, key_type: &str) -> String {
    let prefix = match provider {
        "copilot" => "GITHUB_COPILOT".to_string(),
        "github" => "GITHUB_COPILOT".to_string(),
        p => p.to_uppercase(),
    };
    match key_type {
        "api_key" | "token" => {
            if provider == "copilot" || provider == "github" {
                "GITHUB_COPILOT_TOKEN".to_string()
            } else {
                format!("{}_API_KEY", prefix)
            }
        }
        "secret_key" => format!("{}_SECRET_KEY", prefix),
        other => format!("{}_{}", prefix, other.to_uppercase()),
    }
}

/// Find the `.env` file path relative to the backend binary.
/// Falls back to `./.env` in current directory.
fn dotenv_path() -> std::path::PathBuf {
    // Try next to backend binary first
    if let Some(bin_path) = crate::app::actions::find_backend_binary() {
        if let Some(parent) = bin_path.parent() {
            return parent.join(".env");
        }
    }
    // Fallback: current directory
    std::path::PathBuf::from(".env")
}

/// Read the current `.env` file into a map of key-value pairs.
fn read_dotenv() -> BTreeMap<String, String> {
    let path = dotenv_path();
    let mut map = BTreeMap::new();
    if let Ok(content) = std::fs::read_to_string(&path) {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                map.insert(key.trim().to_string(), value.trim().to_string());
            }
        }
    }
    map
}

/// Write a map of key-value pairs to the `.env` file.
fn write_dotenv(map: &BTreeMap<String, String>) -> Result<()> {
    let path = dotenv_path();
    let mut content = String::from("# Auto-generated by go-on-gui — do not edit manually.\n");
    content.push_str("# Add new keys via the GUI Providers page.\n");
    for (key, value) in map {
        content.push_str(&format!("{}=\"{}\"\n", key, value));
    }
    std::fs::write(&path, content.as_bytes())?;
    // Set 600 permission (user read/write only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Sync a single key-value pair to the `.env` file, preserving existing entries.
pub fn sync_dotenv_key(env_name: &str, value: &str) -> Result<()> {
    let mut map = read_dotenv();
    map.insert(env_name.to_string(), value.to_string());
    write_dotenv(&map)
}

/// Remove a key from the `.env` file, preserving other entries.
pub fn remove_dotenv_key(env_name: &str) -> Result<()> {
    let mut map = read_dotenv();
    map.remove(env_name);
    write_dotenv(&map)
}
