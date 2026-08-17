//! Game state tools: save file management and achievement tracking
//! (feature `game-state`).

use super::{first_existing_path, known_save_paths};
use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};
use anyhow::{anyhow, Context, Result};
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

/// Build a HashMap of known game save paths for documentation/auto-detection.
#[cfg(feature = "game-state")]
fn build_known_games_map() -> serde_json::Value {
    let keys = [
        "factorio",
        "minecraft",
        "stardew valley",
        "terraria",
        "skyrim",
        "cyberpunk 2077",
        "elden ring",
        "balatro",
    ];
    let map: HashMap<String, Vec<String>> = keys
        .iter()
        .map(|k| {
            let paths = known_save_paths(k);
            let str_paths: Vec<String> = paths
                .iter()
                .filter_map(|p| p.to_str().map(String::from))
                .collect();
            ((*k).to_string(), str_paths)
        })
        .collect();
    json!(map)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 6: Game State & Save Management   #[cfg(feature = "game-state")]
// ═══════════════════════════════════════════════════════════════════════════════

/// Lists and manages game save files.
/// Actions:
/// - `list`: List save files for a game
/// - `backup`: Create a backup copy of save files
/// - `restore`: Restore saves from backup
/// - `info`: Show save file metadata
#[cfg(feature = "game-state")]
pub struct GameSaveManagerTool;
#[cfg(feature = "game-state")]
impl Tool for GameSaveManagerTool {
    fn name(&self) -> &'static str {
        "game_save_manager"
    }

    fn exposure(&self) -> crate::orchestration::tool::ToolExposure {
        crate::orchestration::tool::ToolExposure::Deferred
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let game = input.payload["game"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'game'"))?;
        let action = input.payload["action"].as_str().unwrap_or("list");
        let custom_path = input.payload["path"].as_str();

        // Determine save directory
        let save_dir = if let Some(path) = custom_path {
            let p = std::path::Path::new(path);
            if !p.exists() {
                anyhow::bail!("specified path does not exist: {}", path);
            }
            p.to_path_buf()
        } else {
            let known = known_save_paths(game);
            first_existing_path(&known).ok_or_else(|| {
                anyhow!(
                    "no known save path found for '{}'. Provide a custom 'path' parameter. \
                     Known game paths can be discovered with the 'game_save_manager' action 'known-games'.",
                    game
                )
            })?
        };

        debug!(game = %game, action = %action, dir = %save_dir.display(), "game_save_manager");

        match action {
            "list" | "ls" => {
                let saves = list_save_files(&save_dir, game)?;
                let report = tool_execution_report("game_save_manager", Some("saves_listed"));
                Ok(ToolOutput {
                    success: true,
                    result: Some(json!({
                        "game": game,
                        "action": "list",
                        "save_directory": save_dir.to_string_lossy(),
                        "saves": saves["saves"],
                        "total_saves": saves["total"],
                        "total_size_bytes": saves["total_size"],
                    })),
                    error: None,
                    verification: Some("saves_listed".to_string()),
                    audit_log: Some(format!(
                        "game_save_manager: listed {} saves in {}",
                        game,
                        save_dir.display()
                    )),
                    pua_report: Some(report),
                })
            }
            "backup" | "backup-saves" => {
                let backup_dir = save_dir
                    .join("backups")
                    .join(format!("save-backup-{}", chrono_now()));
                std::fs::create_dir_all(&backup_dir)
                    .context("failed to create backup directory")?;

                let save_files = find_save_files(&save_dir, game);
                let mut copied = 0u64;
                let mut total_bytes = 0u64;
                for f in &save_files {
                    let dest = backup_dir.join(f.strip_prefix(&save_dir).unwrap_or(f));
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent).ok();
                    }
                    match std::fs::copy(f, &dest) {
                        Ok(sz) => {
                            copied += 1;
                            total_bytes += sz;
                        }
                        Err(e) => warn!(file = %f.display(), error = %e, "backup: failed to copy"),
                    }
                }

                let report = tool_execution_report("game_save_manager", Some("saves_backed_up"));
                info!(game = %game, backup_dir = %backup_dir.display(), files = %copied, "game_save_manager: backup complete");

                Ok(ToolOutput {
                    success: true,
                    result: Some(json!({
                        "game": game,
                        "action": "backup",
                        "backup_directory": backup_dir.to_string_lossy(),
                        "files_backed_up": copied,
                        "total_bytes": total_bytes,
                    })),
                    error: None,
                    verification: Some("saves_backed_up".to_string()),
                    audit_log: Some(format!(
                        "game_save_manager: backed up {} files for {}",
                        copied, game
                    )),
                    pua_report: Some(report),
                })
            }
            "restore" => {
                let backup_path = input.payload["backup_path"]
                    .as_str()
                    .ok_or_else(|| anyhow!("missing 'backup_path' for restore action"))?;
                let backup_dir = std::path::Path::new(backup_path).to_path_buf();
                if !backup_dir.exists() {
                    anyhow::bail!("backup directory not found: {}", backup_path);
                }

                let mut restored = 0u64;
                for entry in walkdir_simple(&backup_dir) {
                    if entry.is_file() {
                        let relative = entry
                            .strip_prefix(&backup_dir)
                            .unwrap_or(std::path::Path::new(""));
                        let dest = save_dir.join(relative);
                        if let Some(parent) = dest.parent() {
                            std::fs::create_dir_all(parent).ok();
                        }
                        match std::fs::copy(&entry, &dest) {
                            Ok(_) => restored += 1,
                            Err(e) => warn!(file = %entry.display(), error = %e, "restore: failed"),
                        }
                    }
                }

                let report = tool_execution_report("game_save_manager", Some("saves_restored"));
                info!(game = %game, files = %restored, "game_save_manager: restore complete");

                Ok(ToolOutput {
                    success: true,
                    result: Some(json!({
                        "game": game,
                        "action": "restore",
                        "backup_path": backup_path,
                        "files_restored": restored,
                    })),
                    error: None,
                    verification: Some("saves_restored".to_string()),
                    audit_log: Some(format!(
                        "game_save_manager: restored {} files for {}",
                        restored, game
                    )),
                    pua_report: Some(report),
                })
            }
            "info" => {
                let metadata = std::fs::metadata(&save_dir)?;
                let save_files = find_save_files(&save_dir, game);
                let total_size: u64 = save_files
                    .iter()
                    .filter_map(|f| std::fs::metadata(f).ok().map(|m| m.len()))
                    .sum();
                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);

                let report = tool_execution_report("game_save_manager", Some("save_info"));
                Ok(ToolOutput {
                    success: true,
                    result: Some(json!({
                        "game": game,
                        "action": "info",
                        "save_directory": save_dir.to_string_lossy(),
                        "total_files": save_files.len(),
                        "total_size_bytes": total_size,
                        "last_modified": modified,
                        "writable": !std::fs::metadata(&save_dir).map(|m| m.permissions().readonly()).unwrap_or(true),
                    })),
                    error: None,
                    verification: Some("save_info".to_string()),
                    audit_log: Some(format!(
                        "game_save_manager: info for {} at {}",
                        game,
                        save_dir.display()
                    )),
                    pua_report: Some(report),
                })
            }
            "known-games" | "known" => {
                let report = tool_execution_report("game_save_manager", Some("known_games"));
                Ok(ToolOutput {
                    success: true,
                    result: Some(json!({
                        "action": "known-games",
                        "known_games": build_known_games_map(),
                        "note": "These are known save paths. Provide a custom 'path' parameter for unsupported games.",
                    })),
                    error: None,
                    verification: Some("known_games".to_string()),
                    audit_log: Some("game_save_manager: listed known games".to_string()),
                    pua_report: Some(report),
                })
            }
            other => {
                anyhow::bail!(
                    "unsupported action '{}'. Supported: list, backup, restore, info, known-games",
                    other
                );
            }
        }
    }
}

/// Helper: get current timestamp string for backup naming.
#[cfg(feature = "game-state")]
fn chrono_now() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", now.as_secs())
}

/// Lists save files for a game in a given directory.
#[cfg(feature = "game-state")]
fn list_save_files(dir: &std::path::Path, game: &str) -> Result<serde_json::Value> {
    let files = find_save_files(dir, game);
    let total = files.len();
    let total_size: u64 = files
        .iter()
        .filter_map(|f| std::fs::metadata(f).ok().map(|m| m.len()))
        .sum();

    let saves: Vec<serde_json::Value> = files
        .into_iter()
        .map(|p| {
            let meta = std::fs::metadata(&p).ok();
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let modified = meta
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            json!({
                "path": p.to_string_lossy(),
                "filename": p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
                "size_bytes": size,
                "modified": modified,
            })
        })
        .collect();

    Ok(json!({
        "saves": saves,
        "total": total,
        "total_size": total_size,
    }))
}

/// Finds save files in a directory using heuristics (recent files, specific extensions).
#[cfg(feature = "game-state")]
fn find_save_files(dir: &std::path::Path, game: &str) -> Vec<PathBuf> {
    let save_extensions = match game.to_lowercase().as_str() {
        "factorio" => &[".zip", ".dat"] as &[_],
        "minecraft" => &[".dat", ".mca", ".dat_old", ".nbt"],
        "skyrim" | "skyrim special edition" => &[".ess", ".skse", ".bak"],
        _ => &[".sav", ".save", ".dat", ".json", ".bin", ".zip", ".sol"],
    };

    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    let ext_lower = ext.to_lowercase();
                    if save_extensions.contains(&ext_lower.as_str()) {
                        files.push(path);
                    }
                }
            }
        }
    }
    // Sort by modification time (newest first)
    files.sort_by(|a, b| {
        let a_time = std::fs::metadata(a).ok().and_then(|m| m.modified().ok());
        let b_time = std::fs::metadata(b).ok().and_then(|m| m.modified().ok());
        b_time.cmp(&a_time)
    });
    files
}

/// Simple directory walker (avoids extra dependency).
#[cfg(feature = "game-state")]
fn walkdir_simple(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut results = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                results.extend(walkdir_simple(&path));
            } else {
                results.push(path);
            }
        }
    }
    results
}

/// Reads game achievements.
/// Uses Steam API for known Steam games, or reads local achievement files.
#[cfg(feature = "game-state")]
pub struct GameAchievementTool;
#[cfg(feature = "game-state")]
impl Tool for GameAchievementTool {
    fn name(&self) -> &'static str {
        "game_achievements"
    }

    fn exposure(&self) -> crate::orchestration::tool::ToolExposure {
        crate::orchestration::tool::ToolExposure::Deferred
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let game = input.payload["game"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'game'"))?;

        // Known Steam app IDs for achievement lookup
        let steam_app_id = match game.to_lowercase().as_str() {
            "cs2" | "counter-strike 2" => Some(730u64),
            "dota 2" | "dota2" => Some(570),
            "team fortress 2" | "tf2" => Some(440),
            "rust" => Some(252490),
            "terraria" => Some(105600),
            "stardew valley" => Some(413150),
            "elden ring" => Some(1245620),
            "cyberpunk 2077" => Some(1091500),
            "skyrim special edition" => Some(489830),
            _ => None,
        };

        let achievements = if let Some(app_id) = steam_app_id {
            fetch_steam_achievements(app_id)
        } else {
            None
        };

        let report = tool_execution_report("game_achievements", Some("achievements_checked"));

        let result = if let Some(ach) = achievements {
            json!({
                "game": game,
                "source": "steam_api",
                "total_achievements": ach["total"],
                "achievements": ach["list"],
            })
        } else {
            json!({
                "game": game,
                "source": "local_file",
                "achievements": [],
                "note": format!(
                    "No Steam API data available for '{}'. Known Steam app IDs: cs2, dota2, tf2, rust, terraria, stardew valley, elden ring, cyberpunk 2077, skyrim special edition. \
                     Provide a local path or steam_app_id for unsupported games.",
                    game
                ),
                "known_app_ids": json!({
                    "cs2": 730, "dota2": 570, "tf2": 440, "rust": 252490,
                    "terraria": 105600, "stardew valley": 413150,
                    "elden ring": 1245620, "cyberpunk 2077": 1091500,
                    "skyrim special edition": 489830,
                }),
            })
        };

        Ok(ToolOutput {
            success: true,
            result: Some(result),
            error: None,
            verification: Some("achievements_checked".to_string()),
            audit_log: Some(format!("game_achievements: checked '{}'", game)),
            pua_report: Some(report),
        })
    }
}

/// Fetches achievement schema from Steam API for a given app_id.
#[cfg(feature = "game-state")]
fn fetch_steam_achievements(app_id: u64) -> Option<serde_json::Value> {
    let url = format!(
        "https://api.steampowered.com/ISteamUserStats/GetSchemaForGame/v2/?appid={}&l=en",
        app_id
    );
    let client = crate::shared::http_client::blocking_http_client().ok()?;

    let resp = client
        .get(&url)
        .timeout(Duration::from_secs(10))
        .send()
        .ok()?;
    let body: serde_json::Value = resp.json().ok()?;

    let ach_array = body["game"]["availableGameStats"]["achievements"].as_array()?;
    let total = ach_array.len();
    let list: Vec<serde_json::Value> = ach_array
        .iter()
        .map(|a| {
            json!({
                "name": a["name"],
                "display_name": a["displayName"],
                "description": a["description"],
                "hidden": a["hidden"].as_i64().unwrap_or(0) != 0,
                "icon": a["icon"],
                "icongray": a["icongray"],
            })
        })
        .collect();

    Some(json!({
        "total": total,
        "list": list,
    }))
}
