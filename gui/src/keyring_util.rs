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
    pub(crate) fn ensure_item_accessible(_account: &str) {
        // `security set-key-partition-list` modifies the ACL partition list
        // of a keychain item identified by its service name (-d "go-on").
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

/// Store an API key in the system keyring.
pub fn store_api_key(provider: &str, api_key: &str) -> Result<()> {
    platform::store_api_key(provider, api_key)
}

/// Store a provider secret key in the system keyring.
pub fn store_secret_key(provider: &str, secret_key: &str) -> Result<()> {
    platform::store_secret_key(provider, secret_key)
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

/// Delete an API key from the system keyring (silent if missing).
pub fn delete_api_key(provider: &str) -> Result<()> {
    platform::delete_api_key(provider)
}

/// Delete a provider secret key from the system keyring (silent if missing).
/// F-GAP-48: Reserved for future secret key management feature
/// DEPRECATED: Unused. Secret key deletion is handled via platform::delete_secret_key
/// internally when providers are removed through the GUI. Retained for reference;
/// remove in a future cleanup round.
#[allow(dead_code)]
pub fn delete_secret_key(provider: &str) -> Result<()> {
    platform::delete_secret_key(provider)
}

/// Delete the github_copilot_token alias from the system keyring (silent if missing).
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
/// This is a separate alias (alongside copilot_api_key) that the backend reads.
/// On macOS, also configures ACL so the backend process can access it.
pub fn store_copilot_token(token: &str) -> Result<()> {
    let account = "github_copilot_token";
    let entry = keyring::Entry::new("go-on", account)?;
    entry.set_password(token)?;
    #[cfg(target_os = "macos")]
    platform::ensure_item_accessible(account);
    Ok(())
}

/// Retrieve an API key exclusively from the system keyring.
/// Does NOT fall back to config-provided keys to prevent secrets
/// from being stored in plaintext config files.
/// Returns `None` if the key is not found in the keyring.
pub fn get_api_key_with_fallback(provider: &str, _config_key: Option<&str>) -> Option<String> {
    get_api_key(provider)
}
