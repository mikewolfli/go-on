use std::path::{Path, PathBuf};

/// Returns the project's config directory using `directories::ProjectDirs`.
/// This is the canonical location for storing persistent app data
/// (e.g. font cache, chat sessions, templates).
pub fn project_config_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("com", "goon", "go-on-gui")
        .map(|dirs| dirs.config_dir().to_path_buf())
}

/// Get current Unix timestamp in seconds since epoch.
/// Returns 0 if the system clock is before 1970 (extremely unlikely).
pub fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Atomically write content to a file: write to `.tmp` then rename.
/// On crash mid-write, the original file (if any) remains intact.
pub fn atomic_write(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp_path = path.with_extension("tmp");
    std::fs::write(&tmp_path, content)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}


/// Load JSON with automatic corruption recovery from backup.
pub fn load_json_with_backup<T>(path: &Path, label: &str) -> T
where
    T: serde::de::DeserializeOwned + Default,
{
    match std::fs::read_to_string(path) {
        Ok(content) => {
            match serde_json::from_str::<T>(&content) {
                Ok(data) => data,
                Err(e) => {
                    eprintln!(
                        "WARNING: {} file corrupted at {}: {e}. Trying backup...",
                        label,
                        path.display()
                    );
                    let bak_path = path.with_extension("json.bak");
                    match std::fs::read_to_string(&bak_path) {
                        Ok(bak) => {
                            match serde_json::from_str::<T>(&bak) {
                                Ok(data) => {
                                    eprintln!("Recovered {} from backup.", label);
                                    // Restore the backup to the main path
                                    let _ = atomic_write(path, &bak);
                                    data
                                }
                                Err(_) => {
                                    eprintln!(
                                        "ERROR: Backup also corrupted for {}. Using defaults.",
                                        label
                                    );
                                    T::default()
                                }
                            }
                        }
                        Err(_) => {
                            eprintln!(
                                "ERROR: No backup found for {}. Data lost. Using defaults.",
                                label
                            );
                            T::default()
                        }
                    }
                }
            }
        }
        Err(_) => T::default(),
    }
}
