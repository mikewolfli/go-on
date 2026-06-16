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
use anyhow::Result;
use std::collections::BTreeMap;

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
// ╚══════════════════════════════════════════════════════════════════════════╝
#[cfg(target_os = "macos")]
mod platform {
    use anyhow::Result;
    use std::process::Command;

    /// Configure the keychain item's ACL so ANY process (not just the creator)
    /// can read the password without triggering the macOS permission dialog.
    /// This is essential for the backend (a headless child process) to access
    /// API keys stored in the login keychain.
    ///
    /// Matches by service name (`-d "go-on"`) because the `keyring` crate stores
    /// the service as "go-on" but does NOT set a custom keychain "description" field.
    /// Using `-D` (description) would therefore be a silent no-op.
    pub(crate) fn ensure_item_accessible(account: &str) {
        // `security set-key-partition-list` modifies the ACL partition list
        // of a keychain item identified by service name (-d "go-on") AND
        // account name (-a "{account}").  Using -a limits the operation to
        // JUST the specific item, so each call only touches one keychain entry
        // instead of ALL items with service="go-on".
        // -S "apple:default,apple:toolbar,apple:unknown,apple:keychain:basic"
        //    adds the standard system partition groups that all macOS processes
        //    (GUI and CLI) are automatically members of.
        // Without this step, macOS Keychain Services will reject reads from
        // the backend because it's not the process that originally created the item.
        let _ = Command::new("security")
            .args([
                "set-key-partition-list",
                "-S",
                "apple:default,apple:toolbar,apple:unknown,apple:keychain:basic",
                "-k",
                "", // empty keychain password (uses login keychain)
                "-d",
                "go-on",
                "-a",
                account,
                "login.keychain",
            ])
            .output();
    }

    pub fn store_api_key(provider: &str, api_key: &str) -> Result<()> {
        let account = format!("{}_api_key", provider);
        let entry = keyring::Entry::new("go-on", &account)?;
        entry.set_password(api_key)?;
        ensure_item_accessible(&account);
        Ok(())
    }

    pub fn store_secret_key(provider: &str, secret_key: &str) -> Result<()> {
        let account = format!("{}_secret_key", provider);
        let entry = keyring::Entry::new("go-on", &account)?;
        entry.set_password(secret_key)?;
        ensure_item_accessible(&account);
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

    pub fn has_api_key(provider: &str) -> bool {
        get_api_key(provider).is_some()
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

    pub fn has_api_key(provider: &str) -> bool {
        get_api_key(provider).is_some()
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

/// Store an API key in the system keyring AND sync to `.env` file.
/// Hybrid storage ensures the backend can read keys even in headless mode
/// (macOS Security.framework blocks background keychain access).
pub fn store_api_key(provider: &str, api_key: &str) -> Result<()> {
    platform::store_api_key(provider, api_key)?;
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
    let env_name = provider_to_env_name(provider, "secret_key");
    if let Err(e) = sync_dotenv_key(&env_name, secret_key) {
        eprintln!(
            "Warning: failed to sync secret key to .env ({}): {}",
            env_name, e
        );
    }
    Ok(())
}

/// Retrieve an API key from the system keyring.
pub fn get_api_key(provider: &str) -> Option<String> {
    platform::get_api_key(provider)
}

/// Retrieve a provider secret key from the system keyring.
pub fn get_secret_key(provider: &str) -> Option<String> {
    platform::get_secret_key(provider)
}

/// Check whether a key exists in the system keyring for the given provider.
pub fn has_api_key(provider: &str) -> bool {
    platform::has_api_key(provider)
}

/// Delete an API key from the system keyring AND `.env` file (silent if missing).
pub fn delete_api_key(provider: &str) -> Result<()> {
    platform::delete_api_key(provider)?;
    let env_name = provider_to_env_name(provider, "api_key");
    let _ = remove_dotenv_key(&env_name);
    Ok(())
}

/// Delete a provider secret key from the system keyring AND `.env` file (silent if missing).
/// F-GAP-48: Wired — called from providers/mod.rs when removing dual-auth providers.
pub fn delete_secret_key(provider: &str) -> Result<()> {
    platform::delete_secret_key(provider)?;
    let env_name = provider_to_env_name(provider, "secret_key");
    let _ = remove_dotenv_key(&env_name);
    Ok(())
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
    platform::ensure_item_accessible(account);
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
pub fn get_api_key_with_fallback(provider: &str, _config_key: Option<&str>) -> Option<String> {
    get_api_key(provider)
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
