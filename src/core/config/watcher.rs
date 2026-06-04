//! Config file hot-reload using the `notify` crate.

use std::path::Path;
use std::sync::Arc;
use tokio::sync::watch;
use notify::{RecursiveMode, Watcher, EventKind};

/// Starts watching the config file at `path` for changes.
/// Returns a receiver that gets the latest Arc<AppConfig> on every reload.
pub fn start_config_watcher(
    path: &Path,
    initial: Arc<crate::config::AppConfig>,
) -> (watch::Receiver<Arc<crate::config::AppConfig>>, impl Watcher) {
    let (tx, rx) = watch::channel(initial);

    let config_path = path.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event {
            if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                if let Ok(config) = crate::config::AppConfig::load(&config_path) {
                    tracing::info!("config hot-reloaded from {}", config_path.display());
                    let _ = tx.send(Arc::new(config));
                }
            }
        }
    }).expect("config watcher should initialize");

    watcher.watch(path, RecursiveMode::NonRecursive)
        .expect("config watcher should start watching");

    (rx, watcher)
}
