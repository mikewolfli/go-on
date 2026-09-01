//! Online game server tools: A2S protocol queries, price tracking, and
//! matchmaking status checks (feature `game-online`).

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};
use anyhow::{anyhow, Context, Result};
use serde_json::json;
use std::net::UdpSocket;
use std::time::Duration;
use tracing::{debug, warn};

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

    fn exposure(&self) -> crate::orchestration::tool::ToolExposure {
        crate::orchestration::tool::ToolExposure::Deferred
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

    fn exposure(&self) -> crate::orchestration::tool::ToolExposure {
        crate::orchestration::tool::ToolExposure::Deferred
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
                    crate::shared::url_encode::form_url_encode(game)
                );
                let client = crate::shared::http_client::blocking_http_client()
                    .context("failed to build HTTP client")?;

                match client
                    .get(&search_url)
                    .timeout(Duration::from_secs(10))
                    .send()
                {
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

    fn exposure(&self) -> crate::orchestration::tool::ToolExposure {
        crate::orchestration::tool::ToolExposure::Deferred
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
    let client = crate::shared::http_client::blocking_http_client().ok()?;
    let resp = client
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .ok()?;
    let body: serde_json::Value = resp.json().ok()?;
    body["response"]["player_count"].as_u64()
}
