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
//!
//! # Sandbox note
//! The input-automation tools (`xdotool`) deliberately do NOT run inside the
//! OS command sandbox: they must reach the user's X11 display sockets
//! (`/tmp/.X11-unix`, `/run/user/<uid>`), which the sandbox intentionally
//! hides. They are host-interactive utilities gated behind explicit features
//! and user invocation — governance approvals are their control surface.
//! - `game-process`:  Game process launch, monitoring, window management
//! - `game-screen`:   Screen capture, replay recording (via system tools)
//! - `game-input`:    Keyboard/mouse input simulation via xdotool/enigo
//! - `game-agent`:    AI coaching assistant, auto-grinding scripts
//! - `game-state`:    Save file management, achievement tracking
//! - `game-modding`:  Mod installation, listing, and management

use crate::orchestration::tool::{
    RetryPolicy, ToolCapabilityProfile, ToolRegistry, ToolRiskLevel,
};
#[cfg(any(feature = "game-state", feature = "game-modding"))]
use std::path::PathBuf;

#[cfg(feature = "game-online")]
pub mod online;
#[cfg(feature = "game-process")]
pub mod process;
#[cfg(feature = "game-screen")]
pub mod screen;
#[cfg(feature = "game-input")]
pub mod input;
#[cfg(feature = "game-agent")]
pub mod agent;
#[cfg(feature = "game-state")]
pub mod state;
#[cfg(feature = "game-modding")]
pub mod modding;

#[cfg(feature = "game-online")]
pub use online::{GameMatchmakingTool, GamePriceTrackerTool, GameServerQueryTool};
#[cfg(feature = "game-process")]
pub use process::{GameLaunchTool, GameMonitorTool};
#[cfg(feature = "game-screen")]
pub use screen::{GameReplayRecorderTool, GameScreenCaptureTool};
#[cfg(feature = "game-input")]
pub use input::{GameKeyboardInputTool, GameMouseInputTool};
#[cfg(feature = "game-agent")]
pub use agent::{GameAutoGrindTool, GameCoachingAssistantTool};
#[cfg(feature = "game-state")]
pub use state::{GameAchievementTool, GameSaveManagerTool};
#[cfg(feature = "game-modding")]
pub use modding::{GameModInstallTool, GameModListTool};

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
#[cfg(feature = "game-modding")]
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
