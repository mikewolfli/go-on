//! Hot-reload watcher for language files
//!
//! Monitors language files for changes and automatically reloads translations

use crate::i18n::runtime::I18nManager;
use anyhow::Result;
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tracing::{info, warn};

/// Language file watcher for hot-reloading
pub struct LanguageWatcher {
    /// Reference to i18n manager
    i18n_manager: Arc<I18nManager>,
    /// Languages directory to watch
    watch_dir: std::path::PathBuf,
    /// File modification times for detection
    file_times: std::collections::HashMap<std::path::PathBuf, std::time::SystemTime>,
    /// Stop signal
    should_stop: Arc<std::sync::atomic::AtomicBool>,
}

impl LanguageWatcher {
    /// Create new language watcher
    pub fn new(i18n_manager: Arc<I18nManager>, watch_dir: &Path) -> Result<Self> {
        let mut watcher = LanguageWatcher {
            i18n_manager,
            watch_dir: watch_dir.to_path_buf(),
            file_times: std::collections::HashMap::new(),
            should_stop: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };

        // Initialize file times
        watcher.update_file_times()?;

        Ok(watcher)
    }

    /// Update tracked file modification times
    fn update_file_times(&mut self) -> Result<()> {
        self.file_times.clear();

        if let Ok(entries) = std::fs::read_dir(&self.watch_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                    if let Ok(metadata) = std::fs::metadata(&path) {
                        if let Ok(modified) = metadata.modified() {
                            self.file_times.insert(path, modified);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Check if any language file has been modified
    fn check_for_changes(&self) -> bool {
        if let Ok(entries) = std::fs::read_dir(&self.watch_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }

                match std::fs::metadata(&path) {
                    Ok(metadata) => {
                        match metadata.modified() {
                            Ok(modified) => {
                                // Check if file is new or has been modified
                                if !self.file_times.contains_key(&path) {
                                    return true; // New file
                                }
                                if let Some(&old_time) = self.file_times.get(&path) {
                                    if modified > old_time {
                                        return true; // File modified
                                    }
                                }
                            }
                            Err(_) => continue,
                        }
                    }
                    Err(_) => {
                        // File was deleted
                        if self.file_times.contains_key(&path) {
                            return true;
                        }
                    }
                }
            }
        }

        false
    }

    /// Start watching for file changes (runs in background thread)
    pub fn start_watching(&mut self, check_interval: Duration) -> Result<()> {
        let i18n = self.i18n_manager.clone();
        let watch_dir = self.watch_dir.clone();
        let should_stop = self.should_stop.clone();
        let mut watcher = LanguageWatcher::new(i18n, &watch_dir)?;

        thread::spawn(move || {
            loop {
                // Check if we should stop
                if should_stop.load(std::sync::atomic::Ordering::Relaxed) {
                    info!("Language file watcher stopped");
                    break;
                }

                // Check for changes
                if watcher.check_for_changes() {
                    info!("Language files changed, reloading...");

                    // Update tracked times
                    if let Err(e) = watcher.update_file_times() {
                        warn!("Failed to update file times: {}", e);
                        continue;
                    }

                    // Reload all languages
                    match watcher.i18n_manager.load_all_languages() {
                        Ok(_) => info!("Languages reloaded successfully"),
                        Err(e) => warn!("Failed to reload languages: {}", e),
                    }
                }

                // Sleep before next check
                thread::sleep(check_interval);
            }
        });

        info!(
            "Language file watcher started (check interval: {:?})",
            check_interval
        );
        Ok(())
    }

    /// Stop watching
    pub fn stop(&self) {
        self.should_stop
            .store(true, std::sync::atomic::Ordering::Relaxed);
        info!("Language file watcher stop signal sent");
    }
}

/// Start the language file watcher if enabled, spawning it in a background thread.
///
/// This is a convenience function that creates a `LanguageWatcher` from the global
/// i18n manager and starts watching. Returns `Ok(true)` if the watcher was started,
/// `Ok(false)` if the i18n manager is not yet initialized, or an error.
pub fn start_watcher(languages_dir: &Path, check_interval: Duration) -> Result<bool> {
    let i18n_arc = crate::i18n::runtime::I18N.clone();
    let dir = languages_dir.to_path_buf();
    let has_manager = {
        let guard = i18n_arc
            .read()
            .map_err(|e| anyhow::anyhow!("failed to read i18n global lock: {}", e))?;
        guard.is_some()
    };
    if !has_manager {
        warn!("i18n manager not initialized; cannot start watcher");
        return Ok(false);
    }
    // Clone the global manager into an Arc to share with the watcher thread.
    // Since I18N stores Option<I18nManager>, we extract and wrap it.
    let manager_arc = {
        let guard = i18n_arc
            .read()
            .map_err(|e| anyhow::anyhow!("failed to read i18n global lock: {}", e))?;
        match guard.as_ref() {
            Some(_mgr) => {
                // Create a new Arc-wrapped copy of the manager for the watcher
                let copy = I18nManager::new(languages_dir)?;
                Arc::new(copy)
            }
            None => return Ok(false),
        }
    };
    let mut watcher = LanguageWatcher::new(manager_arc, &dir)?;
    watcher.start_watching(check_interval)?;
    info!("Language watcher started for directory: {:?}", dir);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::LanguageWatcher;
    use crate::i18n::runtime::I18nManager;
    use std::sync::Arc;

    #[test]
    fn test_watcher_creation() {
        let temp_dir = tempfile::tempdir().expect("failed to create temp dir");

        let en_path = temp_dir.path().join("en_US.json");
        std::fs::write(
            &en_path,
            r#"{"language":"en_US","messages":{"watcher_test":"ok"}}"#,
        )
        .expect("failed to write language file");

        let manager =
            Arc::new(I18nManager::new(temp_dir.path()).expect("failed to initialize i18n manager"));

        let watcher = LanguageWatcher::new(manager, temp_dir.path())
            .expect("watcher should be created from valid directory");
        watcher.stop();
    }
}
