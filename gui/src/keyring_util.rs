/// Utility for storing and retrieving API keys via the system keyring.
///
/// The keyring entries use the format `go-on/{provider}_api_key`.
/// Uses the platform-native keyring:
///   - Linux: libsecret (Secret Service)
///   - macOS: Keychain (uses login keychain, will prompt for access on first use)
///   - Windows: Credential Manager
///
/// The GUI also stores API keys in config.providers (metadata only — actual key
/// is NOT in config file). On startup the GUI injects env vars into the backend.
use anyhow::Result;

/// Store an API key in the system keyring.
pub fn store_api_key(provider: &str, api_key: &str) -> Result<()> {
    let account = format!("{}_api_key", provider);
    let entry = keyring::Entry::new("go-on", &account)
        .map_err(|e| anyhow::anyhow!("failed to create keyring entry: {}", e))?;
    entry
        .set_password(api_key)
        .map_err(|e| anyhow::anyhow!("failed to save API key to system keyring: {}", e))?;
    Ok(())
}

/// Retrieve an API key from the system keyring.
pub fn get_api_key(provider: &str) -> Option<String> {
    let account = format!("{}_api_key", provider);
    let entry = match keyring::Entry::new("go-on", &account) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("warning: cannot open keyring entry ({}): {}", account, e);
            return None;
        }
    };
    match entry.get_password() {
        Ok(key) => Some(key),
        Err(e) => {
            eprintln!("warning: cannot read keyring entry ({}): {}", account, e);
            None
        }
    }
}

/// Check whether a key exists in the system keyring for the given provider.
pub fn has_api_key(provider: &str) -> bool {
    get_api_key(provider).is_some()
}
