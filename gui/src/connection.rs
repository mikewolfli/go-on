use crate::backend::BackendClient;
use crate::backend::HealthStatus;
use crate::backend::ProviderStatus;
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::Instant;

/// Backend lifecycle status communicated from the polling thread to the UI.
pub(crate) enum BackendUpdate {
    Health(HealthStatus),
    Providers(Vec<ProviderStatus>),
    RefreshDone,
}

/// Manages connection to the go-on backend: HTTP client, health polling,
/// provider status, backend child process lifecycle, and reconnection logic.
pub struct ConnectionManager {
    /// HTTP/RPC client for backend communication
    pub backend: BackendClient,
    /// Channel for receiving async backend updates (health, providers)
    pub backend_updates: mpsc::Receiver<BackendUpdate>,
    /// Channel for sending async backend updates
    pub backend_tx: mpsc::SyncSender<BackendUpdate>,
    /// Whether a backend refresh is pending
    pub pending_refresh: bool,
    /// Last time backend data was refreshed
    pub last_refresh: Instant,
    /// Managed backend child process
    pub backend_child: Option<std::process::Child>,
    /// True when GUI reuses an already-running backend listener instead of spawning child.
    pub backend_reused_external: bool,
    /// Hash of backend URL to detect changes for cache invalidation
    pub last_backend_url_hash: u64,
    /// Original backend URL to detect changes for showing restart button
    pub backend_url_original: String,
    /// Staging buffer for backend health updates; committed in batches to reduce UI jitter.
    pub staged_health: Option<HealthStatus>,
    /// Staging buffer for provider updates; committed in batches to reduce UI jitter.
    pub staged_providers: Option<Vec<ProviderStatus>>,
    /// Marks the end of a refresh cycle so staged values can be committed atomically.
    pub staged_refresh_done: bool,
    /// Last time staged backend data was committed into visible UI state.
    pub last_backend_ui_commit: Instant,
    /// Consecutive backend disconnect samples; used to debounce transient failures.
    pub health_disconnect_streak: u8,
    /// Consecutive backend health poll failures for progressive backoff
    pub consecutive_poll_failures: u8,
}

impl ConnectionManager {
    pub fn new(
        backend: BackendClient,
        backend_child: Option<std::process::Child>,
        backend_reused_external: bool,
        backend_url: String,
    ) -> Self {
        let (backend_tx, backend_updates) = mpsc::sync_channel(128);

        let initial_url_hash = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            backend_url.hash(&mut hasher);
            hasher.finish()
        };

        Self {
            backend,
            backend_updates,
            backend_tx,
            pending_refresh: false,
            last_refresh: Instant::now(),
            backend_child,
            backend_reused_external,
            last_backend_url_hash: initial_url_hash,
            backend_url_original: backend_url,
            staged_health: None,
            staged_providers: None,
            staged_refresh_done: false,
            last_backend_ui_commit: Instant::now(),
            health_disconnect_streak: 0,
            consecutive_poll_failures: 0,
        }
    }
}

/// Build an HTTP client for Copilot OAuth device-code flow.
/// Tries env-var proxies first, then common local proxy ports,
/// then direct, and finally a dangerous fallback with invalid certs.
pub(crate) fn build_copilot_http_client() -> reqwest::Client {
    // Strategy 1: user-configured env var proxy (HTTPS_PROXY, HTTP_PROXY, ALL_PROXY)
    // Check this FIRST so users can explicitly route copilot auth through a proxy.
    let env_vars = [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ];
    for var in &env_vars {
        if let Ok(url) = std::env::var(var) {
            let url = url.trim().to_string();
            if url.is_empty() {
                continue;
            }
            for make_proxy in [
                reqwest::Proxy::all,
                reqwest::Proxy::https,
                reqwest::Proxy::http,
            ] {
                if let Ok(proxy) = make_proxy(&url) {
                    if let Ok(client) = reqwest::Client::builder().proxy(proxy).build() {
                        eprintln!("INFO: copilot auth using proxy from {}: {}", var, url);
                        return client;
                    }
                }
            }
        }
    }

    // Strategy 2: common local proxy ports (same list as backend's build_github_client)
    // Try HTTP probes first, then SOCKS5 probes for the same ports.
    let http_proxies: [&str; 6] = [
        "http://127.0.0.1:15732",
        "http://127.0.0.1:7890",
        "http://127.0.0.1:10809",
        "http://127.0.0.1:10808",
        "http://127.0.0.1:1080",
        "http://127.0.0.1:33210",
    ];
    for url in http_proxies {
        for make_proxy in [
            reqwest::Proxy::all,
            reqwest::Proxy::https,
            reqwest::Proxy::http,
        ] {
            if let Ok(proxy) = make_proxy(url) {
                if let Ok(client) = reqwest::Client::builder().proxy(proxy).build() {
                    eprintln!("INFO: copilot auth using proxy {}", url);
                    return client;
                }
            }
        }
    }

    // SOCKS5 probes for common proxy ports
    let socks_proxies: [&str; 2] = ["socks5://127.0.0.1:7890", "socks5://127.0.0.1:10809"];
    for url in socks_proxies {
        if let Ok(proxy) = reqwest::Proxy::all(url) {
            if let Ok(client) = reqwest::Client::builder().proxy(proxy).build() {
                eprintln!("INFO: copilot auth using proxy {}", url);
                return client;
            }
        }
    }

    // Strategy 3: direct connection (no proxy) — fallback for users without a proxy
    if let Ok(client) = reqwest::Client::builder().no_proxy().build() {
        eprintln!("INFO: copilot auth using direct connection (no proxy)");
        return client;
    }

    // Strategy 4: no proxy + accept invalid certs (for broken corporate cert stores)
    if let Ok(client) = reqwest::Client::builder()
        .no_proxy()
        .danger_accept_invalid_certs(true)
        .build()
    {
        eprintln!(
            "WARNING: copilot auth falling back to dangerous SSL (no certificate verification)"
        );
        return client;
    }

    // Final fallback: default system proxy detection
    eprintln!("INFO: copilot auth using default system proxy detection");
    reqwest::Client::new()
}

/// Lazily-initialised singleton Copilot HTTP client.
pub(crate) static COPILOT_HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
