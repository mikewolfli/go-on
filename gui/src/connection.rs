use crate::backend::BackendClient;
use crate::backend::HealthStatus;
use crate::backend::ProviderStatus;
use std::sync::mpsc;
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
