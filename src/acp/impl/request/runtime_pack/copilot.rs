use super::super::*;
use crate::i18n::runtime::{t, tf};
use crate::shared::secret_override::{get_secret, set_secret_override};

type CopilotModelsCacheEntry = Option<(u64, Vec<String>)>;
type CopilotModelsCache = std::sync::Mutex<CopilotModelsCacheEntry>;
static COPILOT_MODELS_CACHE: std::sync::OnceLock<CopilotModelsCache> = std::sync::OnceLock::new();

const COPILOT_TOKEN_URL: &str = "https://api.github.com/copilot_internal/v2/token";
const COPILOT_MODELS_URL: &str = "https://api.githubcopilot.com/models";
const COPILOT_MODELS_CACHE_TTL_SECS: u64 = 300;

/// Try to build a [`reqwest::Client`] with proxy autodetection.
///
/// Checks `HTTPS_PROXY` / `https_proxy` / `ALL_PROXY` / `all_proxy` environment
/// variables first.  If none are set, probes a list of well-known local proxy ports.
/// Falls back to a plain (direct) client if nothing works.
fn build_github_client() -> reqwest::Client {
    // 1. Check explicitly-configured env vars
    let proxy_env = std::env::var("HTTPS_PROXY")
        .or_else(|_| std::env::var("https_proxy"))
        .or_else(|_| std::env::var("ALL_PROXY"))
        .or_else(|_| std::env::var("all_proxy"));

    if let Ok(proxy_url) = proxy_env {
        if !proxy_url.is_empty() {
            if let Ok(proxy) = reqwest::Proxy::https(&proxy_url) {
                tracing::debug!("Using HTTPS_PROXY proxy: {proxy_url}");
                if let Ok(client) = reqwest::Client::builder().proxy(proxy).build() {
                    return client;
                }
            }
            // If the user set a proxy but it failed to parse, fall through to probing
            tracing::warn!("Failed to build proxy from env var {proxy_url}, trying auto-detect");
        }
    }

    // 2. Common local proxy ports (same list as gui/src/main.rs auto_detect_proxy)
    let common_proxies: &[&str] = &[
        "http://127.0.0.1:15732",
        "http://127.0.0.1:7890",
        "socks5://127.0.0.1:7890",
        "http://127.0.0.1:10809",
        "http://127.0.0.1:10808",
        "http://127.0.0.1:1080",
        "http://127.0.0.1:33210",
    ];

    for proxy_url in common_proxies {
        // Try a quick TCP connect first to see if anything is listening
        let addr = proxy_url
            .trim_start_matches("http://")
            .trim_start_matches("https://")
            .trim_start_matches("socks5://")
            .trim_start_matches("socks4://");
        if let Some(port_str) = addr.split(':').nth(1) {
            if let Ok(port) = port_str.parse::<u16>() {
                let socket_addr = match format!("127.0.0.1:{port}").parse() {
                    Ok(addr) => addr,
                    Err(_) => continue,
                };
                if std::net::TcpStream::connect_timeout(
                    &socket_addr,
                    std::time::Duration::from_millis(100),
                )
                .is_err()
                {
                    continue;
                }
                // Port open – try to build a reqwest client with this proxy
                let proxy_url_str = *proxy_url;
                let proxy_result = if proxy_url_str.starts_with("socks5://") {
                    reqwest::Proxy::all(proxy_url_str)
                } else {
                    reqwest::Proxy::https(proxy_url_str)
                };

                match proxy_result {
                    Ok(proxy) => match reqwest::Client::builder().proxy(proxy).build() {
                        Ok(client) => {
                            tracing::debug!("Using auto-detected proxy: {proxy_url}");
                            return client;
                        }
                        Err(e) => {
                            tracing::debug!(
                                "Proxy {proxy_url} port open but client build failed: {e}"
                            );
                        }
                    },
                    Err(e) => {
                        tracing::debug!("Proxy {proxy_url} port open but proxy parse failed: {e}");
                    }
                }
            }
        }
    }

    // 3. Fallback: plain direct client
    tracing::debug!("No proxy detected, using direct connection");
    reqwest::Client::new()
}

fn copilot_models_cache() -> &'static CopilotModelsCache {
    COPILOT_MODELS_CACHE.get_or_init(|| std::sync::Mutex::new(None))
}

fn read_copilot_models_cache() -> Option<Vec<String>> {
    let guard = copilot_models_cache().lock().ok()?;
    let (fetched_at, models) = guard.as_ref()?.clone();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if now.saturating_sub(fetched_at) <= COPILOT_MODELS_CACHE_TTL_SECS {
        Some(models)
    } else {
        None
    }
}

fn read_stale_copilot_models_cache() -> Option<Vec<String>> {
    let guard = copilot_models_cache().lock().ok()?;
    guard.as_ref().map(|(_, models)| models.clone())
}

fn store_copilot_models_cache(models: Vec<String>) {
    if let Ok(mut guard) = copilot_models_cache().lock() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        *guard = Some((now, models));
    }
}

fn resolve_copilot_github_token() -> Option<String> {
    for env_name in ["GITHUB_COPILOT_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(value) = std::env::var(env_name) {
            if !value.trim().is_empty() {
                return Some(value);
            }
        }
    }

    for account in ["github_copilot_token", "copilot_api_key"] {
        if let Some(value) = crate::shared::secret_override::get_keyring_cached("go-on", account) {
            return Some(value);
        }
    }

    None
}

pub(super) async fn resolve_copilot_models_dynamic() -> Vec<String> {
    if let Some(models) = read_copilot_models_cache() {
        return models;
    }

    let fallback = crate::agents::copilot::COPILOT_FALLBACK_MODEL_PRIORITY
        .iter()
        .map(|model| (*model).to_string())
        .collect::<Vec<_>>();

    let Some(github_token) = resolve_copilot_github_token() else {
        return read_stale_copilot_models_cache().unwrap_or(fallback);
    };

    let client = build_github_client();
    let token_resp = match client
        .get(COPILOT_TOKEN_URL)
        .header("Authorization", format!("token {}", github_token))
        .header("Accept", "application/json")
        .header("User-Agent", "go-on/1.0")
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(_) => return read_stale_copilot_models_cache().unwrap_or(fallback),
    };

    if !token_resp.status().is_success() {
        return read_stale_copilot_models_cache().unwrap_or(fallback);
    }

    let token_body: Value = match token_resp.json().await {
        Ok(body) => body,
        Err(_) => return read_stale_copilot_models_cache().unwrap_or(fallback),
    };

    let Some(copilot_token) = token_body.get("token").and_then(Value::as_str) else {
        return read_stale_copilot_models_cache().unwrap_or(fallback);
    };

    let models_resp = match client
        .get(COPILOT_MODELS_URL)
        .header("Authorization", format!("Bearer {}", copilot_token))
        .header("Accept", "application/json")
        .header("User-Agent", "go-on/1.0")
        .header("Editor-Version", "vscode/1.90.0")
        .header("Editor-Plugin-Version", "copilot-chat/0.17.0")
        .header("Copilot-Integration-Id", "copilot-chat")
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(_) => return read_stale_copilot_models_cache().unwrap_or(fallback),
    };

    if !models_resp.status().is_success() {
        return read_stale_copilot_models_cache().unwrap_or(fallback);
    }

    let payload: Value = match models_resp.json().await {
        Ok(body) => body,
        Err(_) => return read_stale_copilot_models_cache().unwrap_or(fallback),
    };

    let ranked = crate::agents::copilot::CopilotAgent::extract_ranked_model_ids(&payload);
    if ranked.is_empty() {
        return read_stale_copilot_models_cache().unwrap_or(fallback);
    }

    store_copilot_models_cache(ranked.clone());
    ranked
}

/// Configure a keychain item's ACL so ANY process (not just the creator)
/// can read the password without triggering the macOS permission dialog.
/// This is essential for the backend (a headless child process) to access
/// API keys stored in the login keychain.
///
/// Matches by service name (`-d "go-on"`) because the `keyring` crate stores
/// the service as "go-on" but does NOT set a custom keychain "description" field.
/// Using `-D` (description) would therefore be a silent no-op.
#[cfg(target_os = "macos")]
fn ensure_keyring_item_accessible(_account: &str) {
    use std::process::Command;
    let _ = Command::new("security")
        .args([
            "set-key-partition-list",
            "-S",
            "apple:default,apple:toolbar,apple:unknown,apple:keychain:basic",
            "-k",
            "",
            "-d",
            "go-on",
            "login.keychain",
        ])
        .output();
}

#[cfg(not(target_os = "macos"))]
fn ensure_keyring_item_accessible(_account: &str) {}

/// Handle provider configuration request from GUI or other clients.
/// Stores the provider config to system keyring.
pub(super) async fn handle_provider_configure(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let api_key = params.get("api_key").and_then(Value::as_str).unwrap_or("");
    let model = params
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("auto");
    let secret_key = params
        .get("secret_key")
        .and_then(Value::as_str)
        .unwrap_or("");

    info!(
        "{}",
        tf(
            "Provider configured: name={}, model={}, has_secret_key={}",
            &[
                ("name", name),
                ("model", model),
                (
                    "has_secret_key",
                    if secret_key.is_empty() { "no" } else { "yes" }
                )
            ]
        )
    );

    // ── Persist API key to system keyring ──────────────────────────
    if !api_key.is_empty() {
        let account = format!("{}_api_key", name);
        match keyring::Entry::new("go-on", &account) {
            Ok(entry) => {
                if let Err(e) = entry.set_password(api_key) {
                    tracing::warn!("failed to save API key for '{}' to keyring: {}", name, e);
                } else {
                    ensure_keyring_item_accessible(&account);
                }
            }
            Err(e) => tracing::warn!("failed to open keyring entry for '{}': {}", name, e),
        }

        // ── Copilot needs additional secret overrides + keyring entries ──
        if name == "copilot" {
            // Set secret overrides that CopilotAgent reads (thread-safe alternative
            // to std::env::set_var, which is UB in multi-threaded programs).
            set_secret_override("GITHUB_TOKEN", api_key);
            set_secret_override("GITHUB_COPILOT_TOKEN", api_key);
            tracing::info!(
                "Set GITHUB_TOKEN and GITHUB_COPILOT_TOKEN secret overrides for copilot"
            );
            // The built-in provider spec uses api_key_env="GITHUB_COPILOT_TOKEN",
            // which setup.rs maps to keyring://go-on/github_copilot_token.
            // Without this entry, CopilotAgent fails with "keyring lookup failed".
            match keyring::Entry::new("go-on", "github_copilot_token") {
                Ok(entry) => {
                    if let Err(e) = entry.set_password(api_key) {
                        tracing::warn!(
                                "failed to save Copilot token to keyring account github_copilot_token: {}",
                                e
                            );
                    } else {
                        ensure_keyring_item_accessible("github_copilot_token");
                    }
                }
                Err(e) => {
                    tracing::warn!("failed to open keyring entry 'github_copilot_token': {}", e)
                }
            }
        }
    }

    // ── Persist secret_key to system keyring (wenxin dual-auth) ────
    if !secret_key.is_empty() {
        let account = format!("{}_secret_key", name);
        match keyring::Entry::new("go-on", &account) {
            Ok(entry) => {
                if let Err(e) = entry.set_password(secret_key) {
                    tracing::warn!("failed to save secret key for '{}' to keyring: {}", name, e);
                } else {
                    ensure_keyring_item_accessible(&account);
                }
            }
            Err(e) => tracing::warn!("failed to open keyring entry for '{}': {}", name, e),
        }
    }

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "provider": name,
            "model": model,
        }),
    )
    .await
}

/// Handle GitHub Copilot OAuth Device Code flow initiation.
/// Returns a `device_code`, `user_code`, and `verification_uri` (like GitHub's API).
/// The caller (GUI) should display the URI + user_code and then poll
/// `provider.copilot_device_code_poll` with the returned `device_code`.
pub(super) async fn handle_copilot_device_code_request(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    info!("GitHub Copilot Device Code flow requested");

    let client_id = params
        .get("client_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("01ab8ac9400c4e429b23");
    let device_code_url = "https://github.com/login/device/code";
    let scope = params
        .get("scope")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("read:user");

    // Build reqwest client with proxy support
    let client = build_github_client();

    let device_params = [("client_id", client_id), ("scope", scope)];

    match client
        .post(device_code_url)
        .header("Accept", "application/json")
        .form(&device_params)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                let err_msg = format!("GitHub device code request failed ({status}): {body}");
                tracing::error!("{}", err_msg);
                return send_error(server, request_id, -32000, err_msg, None).await;
            }
            match resp.json::<Value>().await {
                Ok(body) => {
                    let device_code = body["device_code"].as_str().unwrap_or("").to_string();
                    let user_code = body["user_code"].as_str().unwrap_or("").to_string();
                    let verification_uri = body["verification_uri"]
                        .as_str()
                        .unwrap_or("https://github.com/login/device")
                        .to_string();
                    let interval = body["interval"].as_u64().unwrap_or(5);

                    info!(
                        "Copilot Device Code issued: user_code={}, uri={}",
                        user_code, verification_uri
                    );

                    send_result(
                        server,
                        request_id,
                        json!({
                            "ok": true,
                            "device_code": device_code,
                            "user_code": user_code,
                            "verification_uri": verification_uri,
                            "interval": interval,
                        }),
                    )
                    .await
                }
                Err(e) => {
                    let err_msg = format!("Failed to parse GitHub device code response: {}", e);
                    tracing::error!("{}", err_msg);
                    send_error(server, request_id, -32000, err_msg, None).await
                }
            }
        }
        Err(e) => {
            let err_msg = format!("Failed to connect to GitHub device code endpoint: {}", e);
            tracing::error!("{}", err_msg);
            send_error(server, request_id, -32000, err_msg, None).await
        }
    }
}

/// Poll GitHub for the access token after device code authorization.
/// The GUI should call this repeatedly (every `interval` seconds) until
/// either a token is returned or the device_code expires.
pub(super) async fn handle_copilot_device_code_poll(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let device_code = params
        .get("device_code")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    if device_code.is_empty() {
        return send_error(
            server,
            request_id,
            -32602,
            "Missing 'device_code' parameter".to_string(),
            None,
        )
        .await;
    }

    info!(
        "Copilot Device Code poll: device_code={}",
        &device_code[..8.min(device_code.len())]
    );

    let client_id = params
        .get("client_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("01ab8ac9400c4e429b23");
    let token_url = "https://github.com/login/oauth/access_token";

    let client = build_github_client();

    let poll_params = [
        ("client_id", client_id),
        ("device_code", &device_code),
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
    ];

    match client
        .post(token_url)
        .header("Accept", "application/json")
        .form(&poll_params)
        .send()
        .await
    {
        Ok(resp) => {
            match resp.json::<Value>().await {
                Ok(body) => {
                    // Check for error responses
                    if let Some(error) = body.get("error").and_then(Value::as_str) {
                        match error {
                            "authorization_pending" => {
                                // User hasn't authorized yet — keep polling
                                return send_result(
                                    server,
                                    request_id,
                                    json!({
                                        "ok": true,
                                        "status": "pending",
                                        "error": error,
                                    }),
                                )
                                .await;
                            }
                            "slow_down" => {
                                // Poll too fast — slow down
                                return send_result(
                                    server,
                                    request_id,
                                    json!({
                                        "ok": true,
                                        "status": "slow_down",
                                        "error": error,
                                    }),
                                )
                                .await;
                            }
                            "expired_token" => {
                                // Device code expired
                                return send_result(
                                    server,
                                    request_id,
                                    json!({
                                        "ok": true,
                                        "status": "expired",
                                        "error": error,
                                    }),
                                )
                                .await;
                            }
                            "access_denied" => {
                                return send_result(
                                    server,
                                    request_id,
                                    json!({
                                        "ok": true,
                                        "status": "denied",
                                        "error": error,
                                    }),
                                )
                                .await;
                            }
                            _ => {
                                return send_result(
                                    server,
                                    request_id,
                                    json!({
                                        "ok": true,
                                        "status": "error",
                                        "error": error,
                                    }),
                                )
                                .await;
                            }
                        }
                    }

                    // Success! We got an access_token
                    let access_token = body
                        .get("access_token")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let token_type = body
                        .get("token_type")
                        .and_then(Value::as_str)
                        .unwrap_or("bearer");
                    let scope = body.get("scope").and_then(Value::as_str).unwrap_or("");

                    info!(
                        "Copilot Device Code flow completed — access_token obtained ({} chars)",
                        access_token.len()
                    );

                    // Set both secret overrides so CopilotAgent works regardless
                    // of configured token_env (thread-safe alternative to
                    // std::env::set_var, which is UB in multi-threaded programs).
                    set_secret_override("GITHUB_TOKEN", access_token);
                    set_secret_override("GITHUB_COPILOT_TOKEN", access_token);

                    // Persist both Copilot keyring aliases for backward/forward compatibility.
                    if !access_token.is_empty() {
                        match keyring::Entry::new("go-on", "copilot_api_key") {
                            Ok(entry) => {
                                if let Err(e) = entry.set_password(access_token) {
                                    tracing::warn!(
                                        "failed to save Copilot token to keyring account copilot_api_key: {}",
                                        e
                                    );
                                } else {
                                    ensure_keyring_item_accessible("copilot_api_key");
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "failed to open keyring for Copilot account copilot_api_key: {}",
                                    e
                                );
                            }
                        }

                        match keyring::Entry::new("go-on", "github_copilot_token") {
                            Ok(entry) => {
                                if let Err(e) = entry.set_password(access_token) {
                                    tracing::warn!(
                                        "failed to save Copilot token to keyring account github_copilot_token: {}",
                                        e
                                    );
                                } else {
                                    ensure_keyring_item_accessible("github_copilot_token");
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "failed to open keyring for Copilot account github_copilot_token: {}",
                                    e
                                );
                            }
                        }
                    }

                    send_result(
                        server,
                        request_id,
                        json!({
                            "ok": true,
                            "status": "authorized",
                            "access_token": access_token,
                            "token_type": token_type,
                            "scope": scope,
                        }),
                    )
                    .await
                }
                Err(e) => {
                    let err_msg = format!("Failed to parse GitHub token response: {}", e);
                    tracing::error!("{}", err_msg);
                    send_error(server, request_id, -32000, err_msg, None).await
                }
            }
        }
        Err(e) => {
            let err_msg = format!("Failed to connect to GitHub token endpoint: {}", e);
            tracing::error!("{}", err_msg);
            send_error(server, request_id, -32000, err_msg, None).await
        }
    }
}
