//! Game agent tools: AI coaching assistant and auto-grinding scripts
//! (feature `game-agent`).

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};
use anyhow::{anyhow, Result};
use serde_json::json;

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

    fn exposure(&self) -> crate::orchestration::tool::ToolExposure {
        crate::orchestration::tool::ToolExposure::Deferred
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

    fn exposure(&self) -> crate::orchestration::tool::ToolExposure {
        crate::orchestration::tool::ToolExposure::Deferred
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
