//! Hot-reload watcher for language files
//!
//! Monitors language files for changes and automatically reloads translations

#![allow(dead_code)]

use crate::i18n::I18nManager;
use anyhow::Result;
use log::{info, warn};
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

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

#[cfg(test)]
mod tests {
    #[test]
    fn test_watcher_creation() {
        // Test will only run if we have proper i18n setup
        // This is a placeholder for CI/CD
    }
}
