/// Utility for storing and retrieving API keys via the system keyring.
///
/// The keyring entries use the format `go-on/{provider}_api_key` to match
/// the `keyring://go-on/{provider}_api_key` scheme used by the backend config.
///
/// Works across Linux (Secret Service / libsecret), macOS (Keychain), and Windows (Credential Manager).
use anyhow::Result;

/// Store an API key in the system keyring.
///
/// # Arguments
/// * `provider` - The provider name (e.g. "deepseek", "openai").
/// * `api_key` - The API key to store.
pub fn store_api_key(provider: &str, api_key: &str) -> Result<()> {
    let account = format!("{}_api_key", provider);
    let entry = keyring::Entry::new("go-on", &account)
        .map_err(|e| anyhow::anyhow!("无法创建 keyring 条目: {}", e))?;
    entry
        .set_password(api_key)
        .map_err(|e| anyhow::anyhow!("无法保存 API key 到系统 keyring: {}", e))?;
    Ok(())
}

/// Retrieve an API key from the system keyring.
///
/// # Arguments
/// * `provider` - The provider name (e.g. "deepseek", "openai").
#[allow(dead_code)]
pub fn get_api_key(provider: &str) -> Option<String> {
    let account = format!("{}_api_key", provider);
    let entry = match keyring::Entry::new("go-on", &account) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("警告: 无法打开 keyring 条目 ({}): {}", account, e);
            return None;
        }
    };
    match entry.get_password() {
        Ok(key) => Some(key),
        Err(e) => {
            eprintln!("警告: 无法读取 keyring 条目 ({}): {}", account, e);
            None
        }
    }
}

/// Delete an API key from the system keyring.
///
/// # Arguments
/// * `provider` - The provider name (e.g. "deepseek", "openai").
#[allow(dead_code)]
pub fn delete_api_key(provider: &str) -> Result<()> {
    let account = format!("{}_api_key", provider);
    let entry = keyring::Entry::new("go-on", &account)
        .map_err(|e| anyhow::anyhow!("无法创建 keyring 条目: {}", e))?;
    entry
        .delete_credential()
        .map_err(|e| anyhow::anyhow!("无法从 keyring 删除 API key: {}", e))?;
    Ok(())
}
