/// Utility for storing and retrieving API keys via the system keyring.
///
/// Keyring entries use the format `go-on/{provider}_api_key`.
///
/// ## Platform backends
///   - **Linux**: `keyring` crate → libsecret (Secret Service)
///   - **Windows**: `keyring` crate → Credential Manager
///   - **macOS**: `security-framework` crate directly → Keychain (with ACL set to
///     system default groups so subsequent reads do NOT pop a password prompt).
///
/// The GUI also keeps `api_key` in `config.providers` as a fallback so that if the
/// system keyring is unavailable (e.g. headless, macOS prompt blocked) the key can
/// still be injected into the backend process environment at startup.
use anyhow::Result;

// ── macOS: use security-framework directly for ACL control ────────────────
#[cfg(target_os = "macos")]
mod platform {
    use anyhow::Result;
    use security_framework::os::macos::keychain::{SecKeychain, SecPreferencesDomain};

    const SERVICE: &str = "go-on";

    pub fn store_api_key(provider: &str, api_key: &str) -> Result<()> {
        let account = format!("{}_api_key", provider);
        let keychain = SecKeychain::default_for_domain(SecPreferencesDomain::User)
            .map_err(|e| anyhow::anyhow!("failed to open login keychain: {}", e))?;

        // If item already exists, update its password.
        if let Ok((_pass, item)) = keychain.find_generic_password(SERVICE, &account) {
            item.set_password(api_key.as_bytes())
                .map_err(|e| anyhow::anyhow!("failed to set password: {}", e))?;
            // Ensure access control is permissive (no pop-up on next read).
            set_item_accessible(&item)?;
            return Ok(());
        }

        // Create new item.
        keychain
            .set_generic_password(SERVICE, &account, api_key.as_bytes())
            .map_err(|e| anyhow::anyhow!("failed to create keychain entry: {}", e))?;

        // Immediately set access control so future reads don't prompt.
        if let Ok((_, item)) = keychain.find_generic_password(SERVICE, &account) {
            let _ = set_item_accessible(&item);
        }
        Ok(())
    }

    fn set_item_accessible(item: &security_framework::os::macos::keychain::SecKeychainItem) -> Result<()> {
        // Create an access object with no specific trusted apps, but with the
        // standard system groups ("apple-tool:", "apple:") that macOS UI tools
        // and the current process are automatically members of.
        //
        // This prevents the "X wants to use your confidential information" dialog.
        let access = security_framework::os::macos::access::SecAccess::create_with_label(
            "go-on",
            &[],                       // no per-app restriction
            &["apple-tool:", "apple:"], // system partition groups
        )
        .map_err(|e| anyhow::anyhow!("failed to create SecAccess: {}", e))?;

        item.set_access(&access)
            .map_err(|e| anyhow::anyhow!("failed to set keychain access: {}", e))?;
        Ok(())
    }

    pub fn get_api_key(provider: &str) -> Option<String> {
        let account = format!("{}_api_key", provider);
        let keychain = SecKeychain::default_for_domain(SecPreferencesDomain::User).ok()?;
        let (password, _item) = keychain.find_generic_password(SERVICE, &account).ok()?;
        String::from_utf8(password.to_vec()).ok()
    }

    pub fn has_api_key(provider: &str) -> bool {
        get_api_key(provider).is_some()
    }

    pub fn delete_api_key(provider: &str) -> Result<()> {
        let account = format!("{}_api_key", provider);
        let keychain = SecKeychain::default_for_domain(SecPreferencesDomain::User)?;
        if let Ok((_, item)) = keychain.find_generic_password(SERVICE, &account) {
            item.delete()?;
        }
        Ok(())
    }
}

// ── Linux / Windows: use the keyring crate ───────────────────────────────
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

    pub fn get_api_key(provider: &str) -> Option<String> {
        let account = format!("{}_api_key", provider);
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
}

// ── Public API — delegates to the active platform module ──────────────────

/// Store an API key in the system keyring.
pub fn store_api_key(provider: &str, api_key: &str) -> Result<()> {
    platform::store_api_key(provider, api_key)
}

/// Retrieve an API key from the system keyring.
pub fn get_api_key(provider: &str) -> Option<String> {
    platform::get_api_key(provider)
}

/// Check whether a key exists in the system keyring for the given provider.
pub fn has_api_key(provider: &str) -> bool {
    platform::has_api_key(provider)
}

/// Delete an API key from the system keyring (silent if missing).
pub fn delete_api_key(provider: &str) -> Result<()> {
    platform::delete_api_key(provider)
}

/// Retrieve an API key, falling back to a config-provided key if the system
/// keyring returns nothing and the config key is non-empty and not a placeholder.
pub fn get_api_key_with_fallback(provider: &str, config_key: Option<&str>) -> Option<String> {
    let k = get_api_key(provider);
    if k.is_some() {
        return k;
    }
    if let Some(ck) = config_key {
        if !ck.is_empty() && ck != "********" {
            return Some(ck.to_owned());
        }
    }
    None
}
