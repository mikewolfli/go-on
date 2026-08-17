//! Game modding tools: mod installation and listing (feature `game-modding`).

use super::{first_existing_path, known_mod_paths};
use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::extended::utils::copy_dir_recursive;
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};
use anyhow::{anyhow, Context, Result};
use serde_json::json;
use std::time::UNIX_EPOCH;
use tracing::{debug, info};

// ═══════════════════════════════════════════════════════════════════════════════
// Section 7: Game Modding Tools   #[cfg(feature = "game-modding")]
// ═══════════════════════════════════════════════════════════════════════════════

/// Installs mods for supported games (user-invoked, single-player only).
/// Copies mod files to the game's mod directory.
#[cfg(feature = "game-modding")]
pub struct GameModInstallTool;
#[cfg(feature = "game-modding")]
impl Tool for GameModInstallTool {
    fn name(&self) -> &'static str {
        "game_mod_install"
    }

    fn exposure(&self) -> crate::orchestration::tool::ToolExposure {
        crate::orchestration::tool::ToolExposure::Deferred
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let mod_source = input.payload["mod_source"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'mod_source' — path to mod file or directory"))?;
        let game = input.payload["game"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'game'"))?;
        let mod_name = input.payload["mod_name"].as_str().unwrap_or_else(|| {
            std::path::Path::new(mod_source)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("mod")
        });

        let source_path = std::path::Path::new(mod_source);
        if !source_path.exists() {
            anyhow::bail!("mod source not found: {}", mod_source);
        }

        // Determine target mod directory
        let custom_target = input.payload["target_directory"].as_str();
        let target_dir = if let Some(t) = custom_target {
            std::path::Path::new(t).to_path_buf()
        } else {
            let known = known_mod_paths(game);
            first_existing_path(&known).ok_or_else(|| {
                anyhow!(
                    "no known mod directory found for '{}'. Provide a custom 'target_directory' parameter. \
                     Use game_mod_list to discover available mod paths.",
                    game
                )
            })?
        };

        // Create target directory if needed
        std::fs::create_dir_all(&target_dir).context("failed to create mod target directory")?;

        let target_path = target_dir.join(mod_name);

        debug!(
            source = %source_path.display(),
            target = %target_path.display(),
            "game_mod_install: installing mod"
        );

        // Copy mod file or directory
        if source_path.is_dir() {
            copy_dir_recursive(source_path, target_path.as_path())
                .context("failed to copy mod directory")?;
        } else {
            std::fs::copy(source_path, &target_path).context("failed to copy mod file")?;
        }

        let file_size = std::fs::metadata(&target_path)
            .ok()
            .map(|m| m.len())
            .unwrap_or(0);

        let report = tool_execution_report("game_mod_install", Some("mod_installed"));
        info!(game = %game, mod_name = %mod_name, target = %target_path.display(), "game_mod_install: installed");

        Ok(ToolOutput {
            success: true,
            result: Some(json!({
                "game": game,
                "mod_name": mod_name,
                "source": mod_source,
                "target_path": target_path.to_string_lossy(),
                "file_size_bytes": file_size,
                "status": "installed",
            })),
            error: None,
            verification: Some("mod_installed".to_string()),
            audit_log: Some(format!(
                "game_mod_install: installed '{}' for '{}' at {}",
                mod_name,
                game,
                target_path.display()
            )),
            pua_report: Some(report),
        })
    }
}

/// Lists available mods for a game.
/// Scans the game's mod directory for installed mods.
#[cfg(feature = "game-modding")]
pub struct GameModListTool;
#[cfg(feature = "game-modding")]
impl Tool for GameModListTool {
    fn name(&self) -> &'static str {
        "game_mod_list"
    }

    fn exposure(&self) -> crate::orchestration::tool::ToolExposure {
        crate::orchestration::tool::ToolExposure::Deferred
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let game = input.payload["game"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'game'"))?;
        let custom_path = input.payload["path"].as_str();

        let mod_dir = if let Some(path) = custom_path {
            let p = std::path::Path::new(path);
            if !p.exists() {
                anyhow::bail!("specified mod path does not exist: {}", path);
            }
            p.to_path_buf()
        } else {
            let known = known_mod_paths(game);
            first_existing_path(&known).ok_or_else(|| {
                anyhow!(
                    "no known mod directory found for '{}'. Provide a custom 'path' parameter. \
                     Known games with mod support: factorio, minecraft, skyrim, stardew valley, balatro",
                    game
                )
            })?
        };

        let mods = list_mods_in_dir(&mod_dir);

        let report = tool_execution_report("game_mod_list", Some("mods_listed"));

        Ok(ToolOutput {
            success: true,
            result: Some(json!({
                "game": game,
                "mod_directory": mod_dir.to_string_lossy(),
                "total_mods": mods.len(),
                "mods": mods,
                "known_mod_paths": known_mod_paths(game).iter()
                    .filter_map(|p| p.to_str().map(String::from))
                    .collect::<Vec<_>>(),
            })),
            error: None,
            verification: Some("mods_listed".to_string()),
            audit_log: Some(format!(
                "game_mod_list: listed {} mods for '{}' in {}",
                mods.len(),
                game,
                mod_dir.display()
            )),
            pua_report: Some(report),
        })
    }
}

/// Lists mods in a given directory with metadata.
#[cfg(feature = "game-modding")]
fn list_mods_in_dir(dir: &std::path::Path) -> Vec<serde_json::Value> {
    let mut mods: Vec<serde_json::Value> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let meta = std::fs::metadata(&path).ok();
            let is_dir = path.is_dir();
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let modified = meta
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);

            // Filter out hidden files and non-mod files
            let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if file_name.starts_with('.') {
                continue;
            }

            mods.push(json!({
                "name": file_name,
                "path": path.to_string_lossy(),
                "is_directory": is_dir,
                "size_bytes": size,
                "modified": modified,
            }));
        }
    }
    // Sort by name
    mods.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    mods
}
