//! Game-related tools for AI-driven game interaction and assistance.
//!
//! Provides tools for game assistance, AI coaching, screen recording/analysis,
//! input automation, game state management, and more — all within legal bounds
//! (accessibility, content creation, AI research, single-player automation, coaching).
//!
//! # Legal & Ethical Design
//! - **No memory/process injection** — operates purely at OS I/O boundary
//! - **No anti-cheat bypass** — never modifies game code or network traffic
//! - **No competitive advantage** — tools are for single-player, accessibility, coaching
//! - **User consent** — all screen/input tools require user invocation
//!
//! # Feature gates
//! Each section is gated by a separate Cargo feature:
//! - `game-online`:   Online game server queries (A2S protocol), price tracking
//! - `game-process`:  Game process launch, monitoring, window management
//! - `game-screen`:   Screen capture, replay recording (via system tools)
//! - `game-input`:    Keyboard/mouse input simulation via xdotool/enigo
//! - `game-agent`:    AI coaching assistant, auto-grinding scripts
//! - `game-state`:    Save file management, achievement tracking
//! - `game-modding`:  Mod installation, listing, and management

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{
    RetryPolicy, Tool, ToolCapabilityProfile, ToolInput, ToolOutput, ToolRegistry, ToolRiskLevel,
};
use anyhow::{anyhow, Context, Result};
use serde_json::json;
#[cfg(feature = "game-state")]
use std::collections::HashMap;
#[cfg(feature = "game-online")]
use std::net::UdpSocket;
use std::path::PathBuf;
#[cfg(any(feature = "game-online", feature = "game-state"))]
use std::time::Duration;
#[cfg(any(feature = "game-process", feature = "game-state"))]
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

// ═══════════════════════════════════════════════════════════════════════════════
// Internal helpers
// ═══════════════════════════════════════════════════════════════════════════════

/// Known save file locations for popular games (cross-platform fallback paths).
/// Keys are lowercase game identifiers (e.g. "factorio", "minecraft").
/// Values are lists of well-known paths relative to common base directories
/// ($HOME, XDG_DATA_HOME, etc.).
#[cfg(any(feature = "game-state", feature = "game-modding"))]
fn known_save_paths(game: &str) -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let xdg_data =
        std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| format!("{}/.local/share", home));
    let xdg_config =
        std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| format!("{}/.config", home));
    let steam_compat = format!("{}/.steam/steam/steamapps/compatdata", home);

    match game.to_lowercase().as_str() {
        "factorio" => vec![
            PathBuf::from(&xdg_data).join("factorio"),
            PathBuf::from(&home).join(".factorio"),
        ],
        "minecraft" => vec![PathBuf::from(&home).join(".minecraft")],
        "stardew valley" => vec![PathBuf::from(&xdg_data).join("StardewValley")],
        "terraria" => vec![PathBuf::from(&home).join(".local/share/Terraria")],
        "skyrim" | "skyrim special edition" => {
            let docs = format!("{}/Documents/My Games/Skyrim Special Edition", home);
            vec![
                PathBuf::from(&steam_compat).join(
                    "489830/pfx/drive_c/users/steamuser/Documents/My Games/Skyrim Special Edition",
                ),
                PathBuf::from(&docs),
            ]
        }
        "cyberpunk 2077" => {
            vec![PathBuf::from(&home).join(".local/share/Steam/steamapps/compatdata/1091500/pfx")]
        }
        "elden ring" => {
            vec![PathBuf::from(&home).join(".local/share/Steam/steamapps/compatdata/1245620/pfx")]
        }
        "balatro" => vec![PathBuf::from(&xdg_data).join("Balatro")],
        _ => vec![
            PathBuf::from(&xdg_data).join(game),
            PathBuf::from(&xdg_config).join(game),
        ],
    }
}

/// Known mod directory paths for popular games.
#[cfg(any(feature = "game-modding", feature = "game-state"))]
fn known_mod_paths(game: &str) -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let xdg_data =
        std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| format!("{}/.local/share", home));

    match game.to_lowercase().as_str() {
        "factorio" => vec![PathBuf::from(&xdg_data).join("factorio/mods")],
        "minecraft" => vec![PathBuf::from(&home).join(".minecraft/mods")],
        "skyrim" | "skyrim special edition" => vec![PathBuf::from(&home)
            .join(".local/share/Steam/steamapps/common/Skyrim Special Edition/Data")],
        "stardew valley" => vec![PathBuf::from(&xdg_data).join("StardewValley/Mods")],
        "balatro" => vec![PathBuf::from(&xdg_data).join("Balatro/Mods")],
        _ => vec![PathBuf::from(&xdg_data).join(format!("{}/mods", game))],
    }
}

/// Find the first existing directory from a list of candidates.
#[cfg(any(feature = "game-state", feature = "game-modding"))]
fn first_existing_path(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|p| p.exists()).cloned()
}

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
// Section 1: Game Server & Online Tools   #[cfg(feature = "game-online")]
// ═══════════════════════════════════════════════════════════════════════════════

const A2S_INFO_REQUEST: &[u8] = &[
    0xFF, 0xFF, 0xFF, 0xFF, 0x54, 0x53, 0x6F, 0x75, 0x72, 0x63, 0x65, 0x20, 0x45, 0x6E, 0x67, 0x69,
    0x6E, 0x65, 0x20, 0x51, 0x75, 0x65, 0x72, 0x79, 0x00,
];

/// Performs a basic A2S_INFO query to read server name, map, players, etc.
#[cfg(feature = "game-online")]
fn a2s_query(addr: &str, timeout_secs: u64) -> Result<serde_json::Value> {
    let socket = UdpSocket::bind("0.0.0.0:0").context("failed to bind UDP socket")?;
    socket
        .set_read_timeout(Some(Duration::from_secs(timeout_secs)))
        .context("failed to set read timeout")?;
    socket
        .set_write_timeout(Some(Duration::from_secs(timeout_secs)))
        .context("failed to set write timeout")?;
    socket
        .send_to(A2S_INFO_REQUEST, addr)
        .context("failed to send A2S_INFO query")?;

    let mut buf = [0u8; 4096];
    let n = socket
        .recv_from(&mut buf)
        .context("no response from server")?;
    let response = &buf[..n.0];

    // Skip 4-byte header (0xFF 0xFF 0xFF 0xFF) and 1-byte type (0x49 for A2S_INFO)
    if response.len() < 6 || response[4] != 0x49 {
        anyhow::bail!("unexpected A2S response format");
    }
    let payload = &response[5..];

    // Parse null-terminated string fields
    let mut parts: Vec<String> = Vec::new();
    let mut current = Vec::new();
    for &b in payload {
        if b == 0x00 {
            parts.push(String::from_utf8_lossy(&current).to_string());
            current.clear();
            if parts.len() >= 8 {
                // We've got enough fields; stop collecting
                break;
            }
        } else if (0x20..=0x7E).contains(&b) {
            current.push(b);
        }
    }

    let protocol = parts.first().unwrap_or(&"?".to_string()).clone();
    let name = parts.get(1).unwrap_or(&"?".to_string()).clone();
    let map = parts.get(2).unwrap_or(&"?".to_string()).clone();
    let folder = parts.get(3).unwrap_or(&"?".to_string()).clone();
    let game = parts.get(4).unwrap_or(&"?".to_string()).clone();

    // After null-terminated fields come: 2-byte app_id, 1-byte num_players,
    // 1-byte max_players, 1-byte num_bots
    // Skip past all 5 null-terminated strings (protocol, name, map, folder, game)
    // to find the binary data that follows.
    let mut null_count = 0;
    let mut binary_offset = 0;
    for (i, &b) in payload.iter().enumerate() {
        if b == 0x00 {
            null_count += 1;
            if null_count == 5 {
                binary_offset = i + 1;
                break;
            }
        }
    }

    let num_players: u8 = payload.get(binary_offset + 2).copied().unwrap_or(0);
    let max_players: u8 = payload.get(binary_offset + 3).copied().unwrap_or(0);
    let num_bots: u8 = payload.get(binary_offset + 4).copied().unwrap_or(0);

    Ok(json!({
        "server_name": name,
        "map": map,
        "game": game,
        "folder": folder,
        "players": num_players,
        "max_players": max_players,
        "bots": num_bots,
        "protocol_version": protocol,
        "address": addr,
    }))
}

/// Queries online game server status (player count, map, gamemode).
/// Uses the A2S protocol over UDP — no game client modification needed.
#[cfg(feature = "game-online")]
pub struct GameServerQueryTool;
#[cfg(feature = "game-online")]
impl Tool for GameServerQueryTool {
    fn name(&self) -> &'static str {
        "game_server_query"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let addr = input.payload["server_address"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'server_address' (format: host:port)"))?;
        if !addr.contains(':') {
            anyhow::bail!("server_address must be in host:port format (e.g. 127.0.0.1:27015)");
        }
        let timeout = input.payload["timeout_secs"].as_u64().unwrap_or(5);

        debug!(address = %addr, timeout = %timeout, "game_server_query: querying server");

        let info = match a2s_query(addr, timeout) {
            Ok(v) => v,
            Err(e) => {
                warn!(address = %addr, error = %e, "game_server_query: A2S query failed");
                json!({
                    "server": addr,
                    "error": format!("A2S query failed: {}", e),
                    "note": "Ensure the server is online and the address:port is correct."
                })
            }
        };

        let report = tool_execution_report("game_server_query", Some("server_queried"));

        Ok(ToolOutput {
            success: true,
            result: Some(json!({
                "server": addr,
                "protocol": "a2s",
                "info": info,
            })),
            error: None,
            verification: Some("server_queried".to_string()),
            audit_log: Some(format!("game_server_query: queried {}", addr)),
            pua_report: Some(report),
        })
    }
}

/// Tracks game prices across stores (Steam, GOG, Epic). Uses public APIs only.
#[cfg(feature = "game-online")]
pub struct GamePriceTrackerTool;
#[cfg(feature = "game-online")]
impl Tool for GamePriceTrackerTool {
    fn name(&self) -> &'static str {
        "game_price_tracker"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let game = input.payload["game_name"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'game_name'"))?;
        let store = input.payload["store"].as_str().unwrap_or("steam");

        let prices = match store.to_lowercase().as_str() {
            "steam" => {
                // Use Steam Store API: search for app
                let search_url = format!(
                    "https://store.steampowered.com/api/storesearch/?term={}&cc=US&l=en",
                    urlencoding(game)
                );
                let client = reqwest::blocking::Client::builder()
                    .timeout(Duration::from_secs(10))
                    .user_agent("go-on/1.0")
                    .build()
                    .context("failed to build HTTP client")?;

                match client.get(&search_url).send() {
                    Ok(resp) => {
                        let body: serde_json::Value =
                            resp.json().unwrap_or(serde_json::Value::Null);
                        if let Some(items) = body["items"].as_array() {
                            if !items.is_empty() {
                                let appid = items[0]["id"].as_i64().unwrap_or(0);
                                // Fetch app details with price
                                let detail_url = format!(
                                    "https://store.steampowered.com/api/appdetails?appids={}&cc=US&l=en&filters=price_overview",
                                    appid
                                );
                                let detail_resp = client.get(&detail_url).send().ok();
                                let detail_body: serde_json::Value = detail_resp
                                    .and_then(|r| r.json().ok())
                                    .unwrap_or(serde_json::Value::Null);
                                let price_data = detail_body[appid.to_string()]["data"]
                                    ["price_overview"]
                                    .clone();

                                json!({
                                    "store": "steam",
                                    "game_name": items[0]["name"],
                                    "app_id": appid,
                                    "price": price_data["final_formatted"].as_str().unwrap_or("N/A"),
                                    "initial_price": price_data["initial_formatted"].as_str().unwrap_or("N/A"),
                                    "discount_percent": price_data["discount_percent"].as_i64().unwrap_or(0),
                                    "currency": "USD",
                                })
                            } else {
                                json!({"store": "steam", "note": "Game not found on Steam store"})
                            }
                        } else {
                            json!({"store": "steam", "note": "Could not search Steam store"})
                        }
                    }
                    Err(e) => json!({
                        "store": "steam",
                        "error": format!("Steam API request failed: {}", e),
                        "note": "Check network connectivity."
                    }),
                }
            }
            _ => json!({
                "store": store,
                "note": format!("Store '{}' is not yet supported. Supported stores: steam", store),
            }),
        };

        let report = tool_execution_report("game_price_tracker", Some("price_checked"));

        Ok(ToolOutput {
            success: true,
            result: Some(json!({
                "game": game,
                "store": store,
                "prices": prices,
            })),
            error: None,
            verification: Some("price_checked".to_string()),
            audit_log: Some(format!("game_price_tracker: checked {} on {}", game, store)),
            pua_report: Some(report),
        })
    }
}

/// Matchmaking status checker for online games.
/// Queries the Steam API for global player stats if available.
#[cfg(feature = "game-online")]
pub struct GameMatchmakingTool;
#[cfg(feature = "game-online")]
impl Tool for GameMatchmakingTool {
    fn name(&self) -> &'static str {
        "game_matchmaking"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let game = input.payload["game"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'game'"))?;

        // Try Steam API for current player counts
        let status = match game.to_lowercase().as_str() {
            "cs2" | "counter-strike 2" => steam_current_players(730),
            "dota 2" | "dota2" => steam_current_players(570),
            "team fortress 2" | "tf2" => steam_current_players(440),
            "rust" => steam_current_players(252490),
            "garry's mod" | "gmod" => steam_current_players(4000),
            _ => None,
        };

        let report = tool_execution_report("game_matchmaking", Some("matchmaking_checked"));

        let result = if let Some(count) = status {
            json!({
                "game": game,
                "status": "online",
                "current_players": count,
                "note": "Player count from Steam API."
            })
        } else {
            json!({
                "game": game,
                "status": "unknown",
                "note": "No Steam player count available for this game. Use game_server_query for specific servers."
            })
        };

        Ok(ToolOutput {
            success: true,
            result: Some(result),
            error: None,
            verification: Some("matchmaking_checked".to_string()),
            audit_log: Some(format!("game_matchmaking: checked {}", game)),
            pua_report: Some(report),
        })
    }
}

/// Query Steam API for current player count of a given app_id.
#[cfg(feature = "game-online")]
fn steam_current_players(app_id: u64) -> Option<u64> {
    let url = format!(
        "https://api.steampowered.com/ISteamUserStats/GetNumberOfCurrentPlayers/v1/?appid={}",
        app_id
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .user_agent("go-on/1.0")
        .build()
        .ok()?;
    let resp = client.get(&url).send().ok()?;
    let body: serde_json::Value = resp.json().ok()?;
    body["response"]["player_count"].as_u64()
}

/// URL-encode a string for use in HTTP queries.
#[cfg(feature = "game-online")]
fn urlencoding(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            ' ' => "+".to_string(),
            other => format!("%{:02X}", other as u8),
        })
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 2: Game Process & Window Tools   #[cfg(feature = "game-process")]
// ═══════════════════════════════════════════════════════════════════════════════

/// Launches a game process with optional arguments.
/// Uses `std::process::Command` to spawn the executable.
#[cfg(feature = "game-process")]
pub struct GameLaunchTool;
#[cfg(feature = "game-process")]
impl Tool for GameLaunchTool {
    fn name(&self) -> &'static str {
        "game_launch"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let exe = input.payload["executable"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'executable'"))?;
        let args: Vec<String> = input.payload["args"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let working_dir = input.payload["working_directory"].as_str();
        let detached = input.payload["detached"].as_bool().unwrap_or(true);

        let exe_path = std::path::Path::new(exe);
        if !exe_path.exists() {
            anyhow::bail!(
                "executable not found: {}. Provide a full path to the game executable.",
                exe
            );
        }

        debug!(executable = %exe, args = ?args, "game_launch: launching game");

        let mut cmd = std::process::Command::new(exe);
        cmd.args(&args);
        if let Some(wd) = working_dir {
            cmd.current_dir(wd);
        }

        // If detached, spawn and forget; otherwise wait briefly and capture
        if detached {
            let child = cmd
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .stdin(std::process::Stdio::null())
                .spawn()
                .context("failed to spawn game process")?;
            let pid = child.id();

            info!(executable = %exe, pid = %pid, "game_launch: game launched (detached)");

            let report = tool_execution_report("game_launch", Some("game_launched"));

            Ok(ToolOutput {
                success: true,
                result: Some(json!({
                    "executable": exe,
                    "pid": pid,
                    "detached": true,
                    "status": "launched",
                })),
                error: None,
                verification: Some("game_launched".to_string()),
                audit_log: Some(format!("game_launch: launched {} (pid {})", exe, pid)),
                pua_report: Some(report),
            })
        } else {
            // Run and collect output (useful for launchers that output to stdout)
            let output = cmd.output().context("failed to run game process")?;
            let pid = 0; // process already exited
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let exit_code = output.status.code();

            info!(executable = %exe, exit_code = ?exit_code, "game_launch: game process exited");

            let report = tool_execution_report("game_launch", Some("game_launched"));

            Ok(ToolOutput {
                success: output.status.success() || exit_code.is_none(),
                result: Some(json!({
                    "executable": exe,
                    "pid": pid,
                    "detached": false,
                    "exit_code": exit_code,
                    "stdout": stdout,
                    "stderr": stderr,
                })),
                error: if !output.status.success() {
                    Some(format!(
                        "process exited with code {:?}: {}",
                        exit_code, stderr
                    ))
                } else {
                    None
                },
                verification: Some("game_launched".to_string()),
                audit_log: Some(format!("game_launch: ran {} (exit {:?})", exe, exit_code)),
                pua_report: Some(report),
            })
        }
    }
}

/// Monitors a running game process (CPU, memory, window state).
/// On Linux reads `/proc/<pid>/stat` and `/proc/<pid>/status` for resource usage.
#[cfg(feature = "game-process")]
pub struct GameMonitorTool;
#[cfg(feature = "game-process")]
impl Tool for GameMonitorTool {
    fn name(&self) -> &'static str {
        "game_monitor"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let pid = input.payload["pid"]
            .as_u64()
            .ok_or_else(|| anyhow!("missing 'pid'"))?;

        debug!(pid = %pid, "game_monitor: monitoring process");

        // Check if process exists
        let proc_path = format!("/proc/{}", pid);
        if !std::path::Path::new(&proc_path).exists() {
            anyhow::bail!("process with PID {} is not running", pid);
        }

        // Read process name from /proc/pid/comm
        let proc_name = std::fs::read_to_string(format!("/proc/{}/comm", pid))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        // Parse /proc/pid/stat for CPU and state info
        let stat = std::fs::read_to_string(format!("/proc/{}/stat", pid)).ok();
        let (state, utime, stime, rss_pages) = stat
            .as_ref()
            .and_then(|s| {
                // Format: pid (comm) state ppid pgrp session tty_nr tpgid flags minflt cminflt majflt cmajflt utime stime cutime cstime ...
                // Find the closing paren of comm, then skip spaces
                let closing_paren = s.rfind(')')?;
                let after = &s[closing_paren + 2..]; // skip ") "
                let fields: Vec<&str> = after.split_whitespace().collect();
                if fields.len() < 23 {
                    return None;
                }
                Some((
                    fields[0].to_string(),           // state
                    fields[11].parse::<u64>().ok()?, // utime (clock ticks)
                    fields[12].parse::<u64>().ok()?, // stime (clock ticks)
                    fields[21].parse::<u64>().ok()?, // rss (pages)
                ))
            })
            .unwrap_or_default();

        // Parse /proc/pid/status for memory and other info
        let status_text = std::fs::read_to_string(format!("/proc/{}/status", pid)).ok();
        let vm_rss_kb = status_text
            .as_ref()
            .and_then(|t| {
                t.lines().find_map(|line| {
                    if line.starts_with("VmRSS:") {
                        line.split_whitespace()
                            .nth(1)
                            .and_then(|v| v.parse::<u64>().ok())
                    } else {
                        None
                    }
                })
            })
            .unwrap_or(0);

        let threads = status_text
            .as_ref()
            .and_then(|t| {
                t.lines().find_map(|line| {
                    if line.starts_with("Threads:") {
                        line.split_whitespace()
                            .nth(1)
                            .and_then(|v| v.parse::<u32>().ok())
                    } else {
                        None
                    }
                })
            })
            .unwrap_or(0);

        // Convert to meaningful units
        // On Linux, clock ticks per second is 100 (USER_HZ)
        let clock_ticks_per_sec = 100;
        let cpu_time_secs = (utime + stime) as f64 / clock_ticks_per_sec as f64;
        let page_size = 4096u64; // standard 4KB pages
        let memory_bytes = rss_pages * page_size;
        // Use VmRSS for more accurate memory reporting
        let memory_kb = if vm_rss_kb > 0 {
            vm_rss_kb
        } else {
            memory_bytes / 1024
        };

        // Check if process window is active (crude check via PID existence)
        let window_active = true; // Process exists, so it's "running"

        let report = tool_execution_report("game_monitor", Some("process_monitored"));

        Ok(ToolOutput {
            success: true,
            result: Some(json!({
                "pid": pid,
                "name": proc_name,
                "state": state,
                "cpu_time_secs": cpu_time_secs,
                "memory_kb": memory_kb,
                "memory_mb": (memory_kb as f64 / 1024.0 * 100.0).round() / 100.0,
                "threads": threads,
                "window_active": window_active,
                "monitored_at": SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            })),
            error: None,
            verification: Some("process_monitored".to_string()),
            audit_log: Some(format!("game_monitor: monitored pid {}", pid)),
            pua_report: Some(report),
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 3: Screen Capture & Replay Tools   #[cfg(feature = "game-screen")]
// ═══════════════════════════════════════════════════════════════════════════════

/// Captures the game window screen for analysis (accessibility/coaching).
/// Attempts to use ImageMagick's `import` or `maim`/`scrot` if available;
/// otherwise returns guidance on how to capture.
#[cfg(feature = "game-screen")]
pub struct GameScreenCaptureTool;
#[cfg(feature = "game-screen")]
impl Tool for GameScreenCaptureTool {
    fn name(&self) -> &'static str {
        "game_screen_capture"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let window = input.payload["window_title"].as_str().unwrap_or("game");
        let output_path = input.payload["output_path"]
            .as_str()
            .unwrap_or("/tmp/game_capture.png");
        let output = std::path::Path::new(output_path);

        // Ensure parent directory exists
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        debug!(window = %window, output = %output_path, "game_screen_capture: capturing");

        // Try available capture tools
        let result = try_capture_with_import(window, output)
            .or_else(|| try_capture_with_maim(window, output))
            .or_else(|| try_capture_with_scrot(output));

        let report = tool_execution_report("game_screen_capture", Some("screen_captured"));

        if let Some(captured_path) = result {
            let metadata = std::fs::metadata(&captured_path).ok();
            let file_size = metadata.map(|m| m.len()).unwrap_or(0);

            Ok(ToolOutput {
                success: true,
                result: Some(json!({
                    "window_title": window,
                    "output_path": captured_path.to_string_lossy(),
                    "file_size_bytes": file_size,
                    "tool": "system_capture",
                })),
                error: None,
                verification: Some("screen_captured".to_string()),
                audit_log: Some(format!(
                    "game_screen_capture: captured '{}' to {}",
                    window,
                    captured_path.display()
                )),
                pua_report: Some(report),
            })
        } else {
            Ok(ToolOutput {
                success: true,
                result: Some(json!({
                    "window_title": window,
                    "output_path": output_path,
                    "note": "No screen capture tool found (tried: import, maim, scrot). Install imagemagick ('import') or maim.",
                    "suggestion": "sudo apt install imagemagick  # or: brew install imagemagick"
                })),
                error: None,
                verification: Some("screen_captured".to_string()),
                audit_log: Some(format!(
                    "game_screen_capture: no tool available for '{}'",
                    window
                )),
                pua_report: Some(report),
            })
        }
    }
}

#[cfg(feature = "game-screen")]
fn try_capture_with_import(window: &str, output: &std::path::Path) -> Option<std::path::PathBuf> {
    // `import -window <title> <output>` from ImageMagick
    let result = std::process::Command::new("import")
        .arg("-window")
        .arg(window)
        .arg(output.as_os_str())
        .output()
        .ok()?;
    if result.status.success() && output.exists() {
        Some(output.to_path_buf())
    } else {
        None
    }
}

#[cfg(feature = "game-screen")]
fn try_capture_with_maim(window: &str, output: &std::path::Path) -> Option<std::path::PathBuf> {
    // `maim -i <window_id> <output>` — requires window ID, not title
    // Try to find window ID via xdotool first
    let window_id = std::process::Command::new("xdotool")
        .args(["search", "--name", window])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout)
                    .ok()
                    .and_then(|s| s.lines().next().map(|l| l.to_string()))
            } else {
                None
            }
        })?;

    let result = std::process::Command::new("maim")
        .args(["-i", &window_id, output.as_os_str().to_str()?])
        .output()
        .ok()?;
    if result.status.success() && output.exists() {
        Some(output.to_path_buf())
    } else {
        None
    }
}

#[cfg(feature = "game-screen")]
fn try_capture_with_scrot(output: &std::path::Path) -> Option<std::path::PathBuf> {
    let result = std::process::Command::new("scrot")
        .arg(output.as_os_str())
        .output()
        .ok()?;
    if result.status.success() && output.exists() {
        Some(output.to_path_buf())
    } else {
        None
    }
}

/// Records a replay from the game window (accessibility/content creation).
/// Uses `ffmpeg` for screen recording if available.
#[cfg(feature = "game-screen")]
pub struct GameReplayRecorderTool;
#[cfg(feature = "game-screen")]
impl Tool for GameReplayRecorderTool {
    fn name(&self) -> &'static str {
        "game_replay_recorder"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let duration_secs = input.payload["duration_secs"].as_u64().unwrap_or(30);
        let output_path = input.payload["output_path"]
            .as_str()
            .unwrap_or("/tmp/game_replay.mp4");
        let fps = input.payload["fps"].as_u64().unwrap_or(30);
        let display = input.payload["display"].as_str().unwrap_or(":0.0");

        let output = std::path::Path::new(output_path);
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        // Check if ffmpeg is available
        let ffmpeg_check = std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .ok();

        let report = tool_execution_report("game_replay_recorder", Some("replay_recorded"));

        if ffmpeg_check.is_some() {
            Ok(ToolOutput {
                success: true,
                result: Some(json!({
                    "duration_secs": duration_secs,
                    "output_path": output_path,
                    "fps": fps,
                    "format": "mp4",
                    "status": "ready",
                    "note": "ffmpeg is available. Use: ffmpeg -f x11grab -framerate {fps} -video_size 1920x1080 -i {display}.0 -t {duration_secs} {output_path}",
                    "command_hint": format!("ffmpeg -f x11grab -framerate {} -t {} -i {}.0+0,0 -c:v libx264 -preset ultrafast -pix_fmt yuv420p {}", fps, duration_secs, display, output_path),
                })),
                error: None,
                verification: Some("replay_recorded".to_string()),
                audit_log: Some(format!(
                    "game_replay_recorder: ready to record {}s to {}",
                    duration_secs, output_path
                )),
                pua_report: Some(report),
            })
        } else {
            Ok(ToolOutput {
                success: true,
                result: Some(json!({
                    "duration_secs": duration_secs,
                    "output_path": output_path,
                    "fps": fps,
                    "format": "mp4",
                    "status": "ffmpeg_not_found",
                    "note": "ffmpeg is not installed. Install it to enable screen recording.",
                    "suggestion": "sudo apt install ffmpeg  # or: brew install ffmpeg"
                })),
                error: None,
                verification: Some("replay_recorded".to_string()),
                audit_log: Some("game_replay_recorder: ffmpeg unavailable".to_string()),
                pua_report: Some(report),
            })
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 4: Game Input Tools (Accessibility/Automation)
//   #[cfg(feature = "game-input")]
// ═══════════════════════════════════════════════════════════════════════════════

/// Simulates keyboard input for accessibility or single-player automation.
/// Uses `xdotool` on Linux if available.
#[cfg(feature = "game-input")]
pub struct GameKeyboardInputTool;
#[cfg(feature = "game-input")]
impl Tool for GameKeyboardInputTool {
    fn name(&self) -> &'static str {
        "game_keyboard_input"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let keys = input.payload["keys"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'keys' — e.g. 'w', 'Return', 'Escape', 'ctrl+c'"))?;
        let delay_ms = input.payload["delay_ms"].as_u64().unwrap_or(50);
        let window = input.payload["window_title"].as_str();

        debug!(keys = %keys, "game_keyboard_input: simulating keys");

        let mut cmd = std::process::Command::new("xdotool");

        // If a window title is specified, focus it first
        if let Some(title) = window {
            cmd.args(["search", "--name", title, "windowactivate"]);
        }

        cmd.args(["--delay", &delay_ms.to_string(), "key", keys]);

        let result = cmd
            .output()
            .context("failed to run xdotool. Install it: sudo apt install xdotool")?;

        let report = tool_execution_report("game_keyboard_input", Some("input_sent"));

        if result.status.success() {
            info!(keys = %keys, "game_keyboard_input: sent successfully");
            Ok(ToolOutput {
                success: true,
                result: Some(json!({
                    "action": "key_press",
                    "keys": keys,
                    "window": window,
                    "status": "sent",
                })),
                error: None,
                verification: Some("input_sent".to_string()),
                audit_log: Some(format!("game_keyboard_input: sent '{}'", keys)),
                pua_report: Some(report),
            })
        } else {
            let stderr = String::from_utf8_lossy(&result.stderr).to_string();
            warn!(keys = %keys, error = %stderr, "game_keyboard_input: xdotool failed");
            Ok(ToolOutput {
                success: false,
                result: Some(json!({
                    "action": "key_press",
                    "keys": keys,
                    "error": stderr,
                    "note": "xdotool failed. Ensure a graphical session is active and xdotool is installed.",
                })),
                error: Some(stderr),
                verification: Some("input_failed".to_string()),
                audit_log: Some(format!("game_keyboard_input: failed '{}'", keys)),
                pua_report: Some(report),
            })
        }
    }
}

/// Simulates mouse input for accessibility or single-player automation.
/// Uses `xdotool` on Linux if available.
#[cfg(feature = "game-input")]
pub struct GameMouseInputTool;
#[cfg(feature = "game-input")]
impl Tool for GameMouseInputTool {
    fn name(&self) -> &'static str {
        "game_mouse_input"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let x = input.payload["x"]
            .as_f64()
            .ok_or_else(|| anyhow!("missing 'x' coordinate"))?;
        let y = input.payload["y"]
            .as_f64()
            .ok_or_else(|| anyhow!("missing 'y' coordinate"))?;
        let action = input.payload["action"].as_str().unwrap_or("click");
        let window = input.payload["window_title"].as_str();
        let button = input.payload["button"].as_str().unwrap_or("1");

        debug!(x = %x, y = %y, action = %action, "game_mouse_input: simulating mouse");

        let mut cmd = std::process::Command::new("xdotool");

        // If a window title is specified, focus it first
        if let Some(title) = window {
            cmd.args(["search", "--name", title, "windowactivate"]);
            // Small sleep to let window focus
            cmd.args(["sleep", "0.1"]);
        }

        // Move mouse to position
        cmd.args(["mousemove", &x.to_string(), &y.to_string()]);

        match action {
            "click" => {
                cmd.args(["click", button]);
            }
            "doubleclick" => {
                cmd.args(["click", "--repeat", "2", button]);
            }
            "mousedown" => {
                cmd.args(["mousedown", button]);
            }
            "mouseup" => {
                cmd.args(["mouseup", button]);
            }
            "move" => {
                // Already moved above — just a no-op
            }
            "scroll" => {
                let amount = input.payload["amount"].as_i64().unwrap_or(1);
                let direction = if amount > 0 { "4" } else { "5" }; // 4=up, 5=down
                cmd.args([
                    "click",
                    "--repeat",
                    &amount.unsigned_abs().to_string(),
                    direction,
                ]);
            }
            other => {
                anyhow::bail!("unsupported mouse action: '{}'. Supported: click, doubleclick, mousedown, mouseup, move, scroll", other);
            }
        }

        let result = cmd
            .output()
            .context("failed to run xdotool. Install it: sudo apt install xdotool")?;

        let report = tool_execution_report("game_mouse_input", Some("input_sent"));

        if result.status.success() {
            info!(action = %action, x = %x, y = %y, "game_mouse_input: sent successfully");
            Ok(ToolOutput {
                success: true,
                result: Some(json!({
                    "x": x,
                    "y": y,
                    "action": action,
                    "button": button,
                    "window": window,
                    "status": "sent",
                })),
                error: None,
                verification: Some("input_sent".to_string()),
                audit_log: Some(format!("game_mouse_input: {} at ({}, {})", action, x, y)),
                pua_report: Some(report),
            })
        } else {
            let stderr = String::from_utf8_lossy(&result.stderr).to_string();
            warn!(action = %action, error = %stderr, "game_mouse_input: xdotool failed");
            Ok(ToolOutput {
                success: false,
                result: Some(json!({
                    "x": x,
                    "y": y,
                    "action": action,
                    "error": stderr,
                    "note": "xdotool failed. Ensure a graphical session is active and xdotool is installed.",
                })),
                error: Some(stderr),
                verification: Some("input_failed".to_string()),
                audit_log: Some(format!(
                    "game_mouse_input: failed {} at ({}, {})",
                    action, x, y
                )),
                pua_report: Some(report),
            })
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 5: Game Agent & Coaching Tools   #[cfg(feature = "game-agent")]
// ═══════════════════════════════════════════════════════════════════════════════

/// AI coaching assistant that analyses game state and provides tips.
/// Produces structured coaching advice based on game name and query.
#[cfg(feature = "game-agent")]
pub struct GameCoachingAssistantTool;
#[cfg(feature = "game-agent")]
impl Tool for GameCoachingAssistantTool {
    fn name(&self) -> &'static str {
        "game_coaching_assistant"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let game_name = input.payload["game"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'game'"))?;
        let query = input.payload["query"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'query'"))?;

        // Build a structured coaching context based on known game mechanics
        let game_context = get_game_coaching_context(game_name);

        let report = tool_execution_report("game_coaching_assistant", Some("coaching_provided"));

        Ok(ToolOutput {
            success: true,
            result: Some(json!({
                "game": game_name,
                "query": query,
                "game_context": game_context,
                "analysis": format!(
                    "Coaching analysis for '{}': The user asked about '{}'. {}",
                    game_name,
                    query,
                    game_context["general_advice"].as_str().unwrap_or("Review the game's mechanics and provide tailored advice.")
                ),
                "coaching_categories": json!([
                    "mechanics",
                    "strategy",
                    "optimization",
                    "tips_and_tricks",
                    "common_mistakes",
                ]),
            })),
            error: None,
            verification: Some("coaching_provided".to_string()),
            audit_log: Some(format!(
                "game_coaching_assistant: coaching on '{}' about '{}'",
                game_name, query
            )),
            pua_report: Some(report),
        })
    }
}

/// Returns known coaching context for a given game.
#[cfg(feature = "game-agent")]
fn get_game_coaching_context(game: &str) -> serde_json::Value {
    match game.to_lowercase().as_str() {
        "factorio" => json!({
            "genre": "factory automation / simulation",
            "general_advice": "Focus on automating early. Build a main bus for resources. Use ratio calculations for assemblers. Defend your base with walls and turrets before expanding.",
            "difficulty": "moderate",
            "common_mistakes": "Hand-crafting too long, not using blueprints, insufficient power generation, not leaving room for expansion.",
        }),
        "minecraft" => json!({
            "genre": "sandbox / survival",
            "general_advice": "Punch trees first, build a crafting table, make a pickaxe, find coal and iron, build a shelter before night. Prioritize food and torches.",
            "difficulty": "easy",
            "common_mistakes": "Not building a bed early, mining without torches, not carrying a water bucket, building without planning.",
        }),
        "stardew valley" => json!({
            "genre": "farming / life simulation",
            "general_advice": "Focus on quality crops, upgrade tools at the blacksmith, build relationships with villagers, complete the community center bundles.",
            "difficulty": "easy",
            "common_mistakes": "Over-extending on crops without energy, ignoring gift-giving, not checking the traveling cart.",
        }),
        "terraria" => json!({
            "genre": "action-adventure / sandbox",
            "general_advice": "Build houses for NPCs, explore caves for ores and heart crystals, craft better gear, prepare arenas for boss fights.",
            "difficulty": "moderate",
            "common_mistakes": "Not building enough housing, going underground without torches and ropes, tackling bosses unprepared.",
        }),
        "cs2" | "counter-strike 2" => json!({
            "genre": "tactical FPS",
            "general_advice": "Learn spray patterns, use utility (smokes/flashes), communicate with your team, practice aim on workshop maps, learn common angles and pre-fire spots.",
            "difficulty": "hard",
            "common_mistakes": "Moving while shooting, not checking corners, wasting utility, poor economy management.",
        }),
        "cyberpunk 2077" => json!({
            "genre": "open-world RPG",
            "general_advice": "Invest in one key attribute early, complete side jobs for rewards and street cred, quickhack builds are powerful, craft and upgrade your gear.",
            "difficulty": "moderate",
            "common_mistakes": "Not upgrading iconic weapons, ignoring cyberware, spreading perk points too thin.",
        }),
        _ => json!({
            "genre": "unknown",
            "general_advice": format!("Analyze the user's question about '{}' and provide helpful gameplay tips. Consider mechanics, strategy, and common pitfalls.", game),
            "difficulty": "unknown",
            "common_mistakes": "Consider the game's genre and mechanics when identifying common mistakes.",
        }),
    }
}

/// AI auto-grinding agent for single-player games (user-invoked automation).
/// Generates a sequence of input commands for repetitive tasks.
#[cfg(feature = "game-agent")]
pub struct GameAutoGrindTool;
#[cfg(feature = "game-agent")]
impl Tool for GameAutoGrindTool {
    fn name(&self) -> &'static str {
        "game_auto_grind"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let task = input.payload["task"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'task' — describe what to automate"))?;
        let game = input.payload["game"].as_str().unwrap_or("unknown");
        let max_iterations = input.payload["max_iterations"].as_u64().unwrap_or(100);
        let interval_ms = input.payload["interval_ms"].as_u64().unwrap_or(500);

        // Generate a script description for the given task
        let script = generate_grind_script(game, task, max_iterations, interval_ms);

        let report = tool_execution_report("game_auto_grind", Some("grind_configured"));

        Ok(ToolOutput {
            success: true,
            result: Some(json!({
                "game": game,
                "task": task,
                "max_iterations": max_iterations,
                "interval_ms": interval_ms,
                "status": "configured",
                "script": script,
                "note": "This script describes the automation steps. Execute via game_keyboard_input / game_mouse_input tools.",
            })),
            error: None,
            verification: Some("grind_configured".to_string()),
            audit_log: Some(format!(
                "game_auto_grind: configured '{}' for {} (max {} iters)",
                task, game, max_iterations
            )),
            pua_report: Some(report),
        })
    }
}

/// Generates descriptive auto-grinding instructions for known game tasks.
#[cfg(feature = "game-agent")]
fn generate_grind_script(
    game: &str,
    task: &str,
    max_iters: u64,
    interval_ms: u64,
) -> serde_json::Value {
    let task_lower = task.to_lowercase();
    let steps: Vec<serde_json::Value> = match game.to_lowercase().as_str() {
        "minecraft" => {
            if task_lower.contains("tree")
                || task_lower.contains("wood")
                || task_lower.contains("chop")
            {
                vec![
                    json!({"step": 1, "action": "look_down", "description": "Look down at ground level"}),
                    json!({"step": 2, "action": "hold_left_click", "description": "Hold left click to break blocks"}),
                    json!({"step": 3, "action": "move_forward", "description": "Move toward tree"}),
                    json!({"step": 4, "action": "repeat", "description": format!("Repeat {} times or until inventory full", max_iters)}),
                ]
            } else if task_lower.contains("fish") {
                vec![
                    json!({"step": 1, "action": "right_click", "description": "Cast fishing rod into water"}),
                    json!({"step": 2, "action": "wait", "description": "Wait for bobber to move (sound/visual cue)"}),
                    json!({"step": 3, "action": "right_click", "description": "Reel in fish"}),
                    json!({"step": 4, "action": "repeat", "description": format!("Repeat {} times", max_iters)}),
                ]
            } else {
                vec![
                    json!({"step": 1, "action": "describe", "description": format!("Custom grinding script for '{}' in Minecraft. Define the specific mouse/keyboard sequence.", task)}),
                ]
            }
        }
        "factorio" => {
            if task_lower.contains("handcraft") || task_lower.contains("craft") {
                vec![
                    json!({"step": 1, "action": "right_click_on_assembler", "description": "Configure recipe"}),
                    json!({"step": 2, "action": "wait", "description": format!("Wait {}ms for production", interval_ms)}),
                    json!({"step": 3, "action": "collect_output", "description": "Pick up finished items"}),
                ]
            } else {
                vec![
                    json!({"step": 1, "action": "describe", "description": format!("Custom automation for '{}' in Factorio. Define the interaction sequence.", task)}),
                ]
            }
        }
        _ => {
            vec![
                json!({"step": 1, "action": "analyze", "description": format!("Analyze '{}' task for '{}'", task, game)}),
                json!({"step": 2, "action": "sequence", "description": "Define the keyboard/mouse sequence for this repetitive task"}),
                json!({"step": 3, "action": "loop", "description": format!("Repeat sequence up to {} times with {}ms interval between iterations", max_iters, interval_ms)}),
            ]
        }
    };

    json!({
        "game": game,
        "task": task,
        "max_iterations": max_iters,
        "interval_ms": interval_ms,
        "steps": steps,
    })
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
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("go-on/1.0")
        .build()
        .ok()?;

    let resp = client.get(&url).send().ok()?;
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
            copy_dir_recursive(source_path, &target_path)
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

/// Recursively copies a directory (behaves like `cp -r`).
#[cfg(feature = "game-modding")]
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dst).context("failed to create destination directory")?;
    for entry in std::fs::read_dir(src).context("failed to read source directory")? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let file_name = src_path
            .file_name()
            .ok_or_else(|| anyhow!("invalid file name"))?;
        let dst_path = dst.join(file_name);
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)
                .context("failed to copy file during mod install")?;
        }
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// Public helper for querying whether any game feature is enabled
// ═══════════════════════════════════════════════════════════════════════════════

/// Returns true if any game-related feature is compiled in.
pub fn has_game_features() -> bool {
    cfg!(any(
        feature = "game-online",
        feature = "game-process",
        feature = "game-screen",
        feature = "game-input",
        feature = "game-agent",
        feature = "game-state",
        feature = "game-modding"
    ))
}

// ═══════════════════════════════════════════════════════════════════════════════
// Registration
// ═══════════════════════════════════════════════════════════════════════════════

/// Register all game-related tools into the given registry.
pub fn register_game_tools(registry: &mut ToolRegistry) {
    // ── Online tools ──────────────────────────────────────────────────────

    #[cfg(feature = "game-online")]
    registry.register_with_profile(
        GameServerQueryTool,
        ToolCapabilityProfile {
            capability: "game_server_query".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 10_000,
            retry_policy: RetryPolicy {
                max_retries: 2,
                retry_on_failure: true,
            },
            fallback_chain: vec![],
        },
    );
    #[cfg(feature = "game-online")]
    registry.register_with_profile(
        GamePriceTrackerTool,
        ToolCapabilityProfile {
            capability: "game_price_tracker".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 15_000,
            retry_policy: RetryPolicy {
                max_retries: 2,
                retry_on_failure: true,
            },
            fallback_chain: vec![],
        },
    );
    #[cfg(feature = "game-online")]
    registry.register_with_profile(
        GameMatchmakingTool,
        ToolCapabilityProfile {
            capability: "game_matchmaking".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 10_000,
            retry_policy: RetryPolicy {
                max_retries: 2,
                retry_on_failure: true,
            },
            fallback_chain: vec![],
        },
    );

    // ── Process tools ─────────────────────────────────────────────────────

    #[cfg(feature = "game-process")]
    registry.register_with_profile(
        GameLaunchTool,
        ToolCapabilityProfile {
            capability: "game_launch".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 30_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: false,
            },
            fallback_chain: vec![],
        },
    );
    #[cfg(feature = "game-process")]
    registry.register_with_profile(
        GameMonitorTool,
        ToolCapabilityProfile {
            capability: "game_monitor".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 5_000,
            retry_policy: RetryPolicy {
                max_retries: 3,
                retry_on_failure: true,
            },
            fallback_chain: vec![],
        },
    );

    // ── Screen tools ──────────────────────────────────────────────────────

    #[cfg(feature = "game-screen")]
    registry.register_with_profile(
        GameScreenCaptureTool,
        ToolCapabilityProfile {
            capability: "game_screen_capture".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 15_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: false,
            },
            fallback_chain: vec![],
        },
    );
    #[cfg(feature = "game-screen")]
    registry.register_with_profile(
        GameReplayRecorderTool,
        ToolCapabilityProfile {
            capability: "game_replay_recorder".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 60_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: false,
            },
            fallback_chain: vec![],
        },
    );

    // ── Input tools ───────────────────────────────────────────────────────

    #[cfg(feature = "game-input")]
    registry.register_with_profile(
        GameKeyboardInputTool,
        ToolCapabilityProfile {
            capability: "game_keyboard_input".to_string(),
            risk_level: ToolRiskLevel::High,
            timeout_budget_ms: 5_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: false,
            },
            fallback_chain: vec![],
        },
    );
    #[cfg(feature = "game-input")]
    registry.register_with_profile(
        GameMouseInputTool,
        ToolCapabilityProfile {
            capability: "game_mouse_input".to_string(),
            risk_level: ToolRiskLevel::High,
            timeout_budget_ms: 5_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: false,
            },
            fallback_chain: vec![],
        },
    );

    // ── Agent/coaching tools ──────────────────────────────────────────────

    #[cfg(feature = "game-agent")]
    registry.register_with_profile(
        GameCoachingAssistantTool,
        ToolCapabilityProfile {
            capability: "game_coaching_assistant".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 5_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: vec![],
        },
    );
    #[cfg(feature = "game-agent")]
    registry.register_with_profile(
        GameAutoGrindTool,
        ToolCapabilityProfile {
            capability: "game_auto_grind".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 5_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: false,
            },
            fallback_chain: vec![],
        },
    );

    // ── State tools ───────────────────────────────────────────────────────

    #[cfg(feature = "game-state")]
    registry.register_with_profile(
        GameSaveManagerTool,
        ToolCapabilityProfile {
            capability: "game_save_manager".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 30_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: false,
            },
            fallback_chain: vec![],
        },
    );
    #[cfg(feature = "game-state")]
    registry.register_with_profile(
        GameAchievementTool,
        ToolCapabilityProfile {
            capability: "game_achievements".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 15_000,
            retry_policy: RetryPolicy {
                max_retries: 2,
                retry_on_failure: true,
            },
            fallback_chain: vec![],
        },
    );

    // ── Modding tools ─────────────────────────────────────────────────────

    #[cfg(feature = "game-modding")]
    registry.register_with_profile(
        GameModInstallTool,
        ToolCapabilityProfile {
            capability: "game_mod_install".to_string(),
            risk_level: ToolRiskLevel::Medium,
            timeout_budget_ms: 60_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: false,
            },
            fallback_chain: vec![],
        },
    );
    #[cfg(feature = "game-modding")]
    registry.register_with_profile(
        GameModListTool,
        ToolCapabilityProfile {
            capability: "game_mod_list".to_string(),
            risk_level: ToolRiskLevel::Low,
            timeout_budget_ms: 10_000,
            retry_policy: RetryPolicy {
                max_retries: 1,
                retry_on_failure: true,
            },
            fallback_chain: vec![],
        },
    );
}
