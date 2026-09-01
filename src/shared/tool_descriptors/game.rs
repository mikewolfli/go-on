//! Descriptors for game tools.

use crate::mcp::McpTool;
use serde_json::json;

/// Returns the MCP tool descriptor for a known game tool name, or `None`.
pub(super) fn descriptor(name: &str) -> Option<McpTool> {
    match name {
        // ── Game tools ────────────────────────────────────────────────
        "game_server_query" => Some(McpTool {
            name: name.to_string(),
            description: Some("Query an online game server (A2S protocol) for status and player info.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "game_name": {"type": "string", "description": "Game name"},
                    "server_address": {"type": "string", "description": "Server address (host:port)"},
                    "store": {"type": "string", "description": "Store identifier"},
                    "timeout_secs": {"type": "integer", "description": "Query timeout in seconds"}
                },
                "required": ["server_address"]
            })),
        }),
        "game_price_tracker" => Some(McpTool {
            name: name.to_string(),
            description: Some("Track game prices across stores.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "game_name": {"type": "string", "description": "Game name"},
                    "store": {"type": "string", "description": "Store identifier"}
                },
                "required": ["game_name"]
            })),
        }),
        "game_matchmaking" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Query the current Steam player count for a known game (cs2, dota 2, tf2, rust, gmod). For server details use game_server_query.".to_string(),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "game": {"type": "string", "description": "Game name (cs2, dota 2, tf2, rust, gmod)"}
                },
                "required": ["game"]
            })),
        }),
        "game_launch" => Some(McpTool {
            name: name.to_string(),
            description: Some("Launch a game process.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "executable": {"type": "string", "description": "Executable path"},
                    "working_directory": {"type": "string", "description": "Working directory"},
                    "args": {"type": "array", "items": {"type": "string"}, "description": "Launch arguments"},
                    "detached": {"type": "boolean", "description": "Run detached"}
                },
                "required": ["executable"]
            })),
        }),
        "game_monitor" => Some(McpTool {
            name: name.to_string(),
            description: Some("Monitor a running game process by PID.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "pid": {"type": "integer", "description": "Process ID to monitor"}
                },
                "required": ["pid"]
            })),
        }),
        "game_screen_capture" => Some(McpTool {
            name: name.to_string(),
            description: Some("Capture a screenshot of a game window or display.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "output_path": {"type": "string", "description": "Screenshot output path"},
                    "window_title": {"type": "string", "description": "Target window title"}
                },
                "required": ["output_path"]
            })),
        }),
        "game_replay_recorder" => Some(McpTool {
            name: name.to_string(),
            description: Some("Record a game replay or screen recording.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "output_path": {"type": "string", "description": "Recording output path"},
                    "duration_secs": {"type": "integer", "description": "Recording duration in seconds"},
                    "fps": {"type": "integer", "description": "Frames per second"},
                    "display": {"type": "string", "description": "Display identifier"},
                    "window_title": {"type": "string", "description": "Window title"},
                    "keys": {"type": "array", "items": {"type": "string"}, "description": "Key sequence"},
                    "delay_ms": {"type": "integer", "description": "Delay before recording in ms"}
                },
                "required": ["output_path"]
            })),
        }),
        "game_keyboard_input" => Some(McpTool {
            name: name.to_string(),
            description: Some("Simulate keyboard input (keys or button actions).".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "description": "Action to perform"},
                    "button": {"type": "string", "description": "Button name"},
                    "keys": {"type": "array", "items": {"type": "string"}, "description": "Keys to press"},
                    "window_title": {"type": "string", "description": "Target window title"},
                    "delay_ms": {"type": "integer", "description": "Delay in ms"},
                    "x": {"type": "integer", "description": "X coordinate"},
                    "y": {"type": "integer", "description": "Y coordinate"}
                },
                "required": ["action"]
            })),
        }),
        "game_mouse_input" => Some(McpTool {
            name: name.to_string(),
            description: Some("Simulate mouse input (move, click, scroll).".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "description": "Action (move/click/scroll)"},
                    "button": {"type": "string", "description": "Mouse button"},
                    "amount": {"type": "integer", "description": "Scroll amount"},
                    "window_title": {"type": "string", "description": "Target window title"},
                    "x": {"type": "integer", "description": "X coordinate"},
                    "y": {"type": "integer", "description": "Y coordinate"}
                },
                "required": ["action"]
            })),
        }),
        "game_coaching_assistant" => Some(McpTool {
            name: name.to_string(),
            description: Some("AI coaching assistant for a game: answers strategy questions.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "game": {"type": "string", "description": "Game name"},
                    "query": {"type": "string", "description": "Coaching question"}
                },
                "required": ["game", "query"]
            })),
        }),
        "game_auto_grind" => Some(McpTool {
            name: name.to_string(),
            description: Some("Run an auto-grinding script for a game task.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "game": {"type": "string", "description": "Game name"},
                    "task": {"type": "string", "description": "Grinding task description"},
                    "max_iterations": {"type": "integer", "description": "Maximum iterations"},
                    "interval_ms": {"type": "integer", "description": "Interval between iterations in ms"}
                },
                "required": ["game", "task"]
            })),
        }),
        "game_save_manager" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Manage game save files: list, backup, restore, show info, or list known save-path games.".to_string(),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["list", "backup", "restore", "info", "known-games"], "description": "Save action"},
                    "game": {"type": "string", "description": "Game name (not needed for known-games)"},
                    "path": {"type": "string", "description": "Custom save path (skips known-path lookup)"},
                    "backup_path": {"type": "string", "description": "Backup directory to restore from (required for restore)"}
                },
                "required": ["action"]
            })),
        }),
        "game_achievements" => Some(McpTool {
            name: name.to_string(),
            description: Some("List achievements for a game.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "game": {"type": "string", "description": "Game name"}
                },
                "required": ["game"]
            })),
        }),
        "game_mod_install" => Some(McpTool {
            name: name.to_string(),
            description: Some("Install a mod for a game from a source archive.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "game": {"type": "string", "description": "Game name"},
                    "mod_name": {"type": "string", "description": "Mod name"},
                    "mod_source": {"type": "string", "description": "Mod source URL or path"},
                    "path": {"type": "string", "description": "Mod archive path"},
                    "target_directory": {"type": "string", "description": "Install target directory"}
                },
                "required": ["game", "mod_name"]
            })),
        }),
        "game_mod_list" => Some(McpTool {
            name: name.to_string(),
            description: Some("List installed mods for a game.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "game": {"type": "string", "description": "Game name"},
                    "path": {"type": "string", "description": "Mod directory path"}
                },
                "required": ["game"]
            })),
        }),
        _ => None,
    }
}
