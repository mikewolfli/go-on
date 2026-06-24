//! Native tool bridge for provider-native function calling
//!
//! Converts ToolRegistry tools into OpenAI/Anthropic-compatible JSON Schema
//! function definitions, parses native function call responses back into
//! ToolInput, and falls back to custom `__tool_call__:` protocol when
//! a provider does not support native function calling.

use crate::orchestration::autonomy_runtime::{build_tool_call_token, parse_tool_call_token};
use crate::orchestration::tool::{ToolInput, ToolRegistry};
use serde_json::{json, Value};
use std::collections::HashMap;

/// Bridge between go-on ToolRegistry and provider-native function calling.
pub struct NativeToolBridge {
    registry: ToolRegistry,
    /// Maps tool names to their JSON Schema function definitions
    schema_cache: HashMap<String, Value>,
}

impl NativeToolBridge {
    /// Create a new bridge backed by the given ToolRegistry.
    pub fn new(registry: ToolRegistry) -> Self {
        let mut bridge = Self {
            registry,
            schema_cache: HashMap::new(),
        };
        bridge.rebuild_cache();
        bridge
    }

    /// Rebuild the internal schema cache from the registry.
    fn rebuild_cache(&mut self) {
        self.schema_cache.clear();
        for name in self.registry.names() {
            let schema = self.build_tool_schema(name);
            self.schema_cache.insert(name.to_string(), schema);
        }
    }

    /// Build a JSON Schema function definition for a single tool.
    fn build_tool_schema(&self, tool_name: &str) -> Value {
        match tool_name {
            "read_file" => json!({
                "type": "function",
                "function": {
                    "name": "read_file",
                    "description": "Read the contents of a file at the given path",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Path to the file to read"
                            }
                        },
                        "required": ["path"]
                    }
                }
            }),
            "write_file" => json!({
                "type": "function",
                "function": {
                    "name": "write_file",
                    "description": "Write content to a file at the given path",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Path to the file to write"
                            },
                            "content": {
                                "type": "string",
                                "description": "Content to write to the file"
                            },
                            "mode": {
                                "type": "string",
                                "enum": ["overwrite", "append"],
                                "description": "Write mode: overwrite or append"
                            }
                        },
                        "required": ["path", "content"]
                    }
                }
            }),
            "search_files" => json!({
                "type": "function",
                "function": {
                    "name": "search_files",
                    "description": "Search for files matching a glob pattern",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "pattern": {
                                "type": "string",
                                "description": "Glob pattern to match files (e.g. **/*.rs)"
                            },
                            "directory": {
                                "type": "string",
                                "description": "Root directory to search from"
                            }
                        },
                        "required": ["pattern"]
                    }
                }
            }),
            "apply_patch" => json!({
                "type": "function",
                "function": {
                    "name": "apply_patch",
                    "description": "Apply a unified diff patch using git apply",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "patch": {
                                "type": "string",
                                "description": "Unified diff patch content to apply"
                            },
                            "check": {
                                "type": "boolean",
                                "description": "If true, only check if patch applies without modifying files"
                            },
                            "directory": {
                                "type": "string",
                                "description": "Working directory for the patch operation"
                            }
                        },
                        "required": ["patch"]
                    }
                }
            }),
            "run_tests" => json!({
                "type": "function",
                "function": {
                    "name": "run_tests",
                    "description": "Run tests using a configured test command with arguments",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "command": {
                                "type": "string",
                                "description": "Test command to run (e.g. cargo, npm, pytest)"
                            },
                            "args": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Arguments to pass to the test command"
                            },
                            "directory": {
                                "type": "string",
                                "description": "Working directory for test execution"
                            }
                        },
                        "required": ["command", "args"]
                    }
                }
            }),
            "inspect_git_diff" => json!({
                "type": "function",
                "function": {
                    "name": "inspect_git_diff",
                    "description": "Inspect the current git diff in the working directory",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "directory": {
                                "type": "string",
                                "description": "Working directory for git diff"
                            },
                            "staged": {
                                "type": "boolean",
                                "description": "If true, show staged (cached) changes instead"
                            },
                            "files": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Specific files to diff"
                            }
                        },
                        "required": []
                    }
                }
            }),
            "shell_exec" => json!({
                "type": "function",
                "function": {
                    "name": "shell_exec",
                    "description": "Execute a shell command with a timeout and capture stdout/stderr",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "command": {
                                "type": "string",
                                "description": "Shell command to execute"
                            },
                            "timeout_ms": {
                                "type": "integer",
                                "description": "Timeout in milliseconds for command execution"
                            },
                            "directory": {
                                "type": "string",
                                "description": "Working directory for command execution"
                            }
                        },
                        "required": ["command"]
                    }
                }
            }),
            "http_request" => json!({
                "type": "function",
                "function": {
                    "name": "http_request",
                    "description": "Make an HTTP GET or POST request",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "url": {
                                "type": "string",
                                "description": "URL to send the request to"
                            },
                            "method": {
                                "type": "string",
                                "enum": ["GET", "POST"],
                                "description": "HTTP method"
                            },
                            "headers": {
                                "type": "object",
                                "description": "HTTP headers as key-value pairs"
                            },
                            "body": {
                                "type": "string",
                                "description": "Request body for POST requests"
                            },
                            "timeout_ms": {
                                "type": "integer",
                                "description": "Request timeout in milliseconds"
                            }
                        },
                        "required": ["url"]
                    }
                }
            }),
            "grep" => json!({
                "type": "function",
                "function": {
                    "name": "grep",
                    "description": "Search file contents using a regex pattern",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "pattern": {
                                "type": "string",
                                "description": "Regular expression pattern to search for"
                            },
                            "directory": {
                                "type": "string",
                                "description": "Root directory to search in"
                            },
                            "include": {
                                "type": "string",
                                "description": "Glob pattern to filter files (e.g. **/*.rs)"
                            }
                        },
                        "required": ["pattern"]
                    }
                }
            }),
            "find_files" => json!({
                "type": "function",
                "function": {
                    "name": "find_files",
                    "description": "Find files matching a glob pattern",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "pattern": {
                                "type": "string",
                                "description": "Glob pattern to match (e.g. **/*.rs)"
                            },
                            "directory": {
                                "type": "string",
                                "description": "Root directory to search from"
                            }
                        },
                        "required": ["pattern"]
                    }
                }
            }),
            "git" => json!({
                "type": "function",
                "function": {
                    "name": "git",
                    "description": "Run git commands: status, log, diff, show, stash",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "subcommand": {
                                "type": "string",
                                "enum": ["status", "log", "diff", "show", "stash"],
                                "description": "Git subcommand to execute"
                            },
                            "args": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Additional arguments for the subcommand"
                            },
                            "directory": {
                                "type": "string",
                                "description": "Working directory for the git command"
                            }
                        },
                        "required": ["subcommand"]
                    }
                }
            }),
            "list_directory" => json!({
                "type": "function",
                "function": {
                    "name": "list_directory",
                    "description": "List the contents of a directory",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Path to the directory to list"
                            }
                        },
                        "required": ["path"]
                    }
                }
            }),
            "cargo_check" => json!({
                "type": "function",
                "function": {
                    "name": "cargo_check",
                    "description": "Run cargo check and parse compilation errors",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "directory": {
                                "type": "string",
                                "description": "Working directory for cargo check"
                            }
                        },
                        "required": []
                    }
                }
            }),
            "cargo_test" => json!({
                "type": "function",
                "function": {
                    "name": "cargo_test",
                    "description": "Run cargo test with an optional test name filter",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "filter": {
                                "type": "string",
                                "description": "Test name filter to run specific tests"
                            },
                            "directory": {
                                "type": "string",
                                "description": "Working directory for cargo test"
                            }
                        },
                        "required": []
                    }
                }
            }),
            "file_move" => json!({
                "type": "function",
                "function": {
                    "name": "file_move",
                    "description": "Move or rename a file or directory atomically",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "source": {
                                "type": "string",
                                "description": "Source path of the file or directory to move"
                            },
                            "destination": {
                                "type": "string",
                                "description": "Destination path"
                            }
                        },
                        "required": ["source", "destination"]
                    }
                }
            }),
            "file_delete" => json!({
                "type": "function",
                "function": {
                    "name": "file_delete",
                    "description": "Delete a file or directory (requires confirmation)",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Path to the file or directory to delete"
                            },
                            "confirm": {
                                "type": "boolean",
                                "description": "Explicit confirmation to proceed with deletion"
                            }
                        },
                        "required": ["path", "confirm"]
                    }
                }
            }),
            // ── Game tools: Server & Online ────────────────────────────────
            "game_server_query" => json!({
                "type": "function",
                "function": {
                    "name": "game_server_query",
                    "description": "Query an online game server for status info (players, map, gamemode)",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "server_address": {"type": "string", "description": "Server address in host:port format"},
                            "protocol": {"type": "string", "enum": ["a2s", "http"], "description": "Query protocol"}
                        },
                        "required": ["server_address"]
                    }
                }
            }),
            "game_price_tracker" => json!({
                "type": "function",
                "function": {
                    "name": "game_price_tracker",
                    "description": "Track game prices across stores (Steam, GOG, Epic)",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "game_name": {"type": "string", "description": "Name of the game to look up"},
                            "stores": {"type": "array", "items": {"type": "string", "enum": ["steam", "gog", "epic", "all"]}, "description": "Stores to check"}
                        },
                        "required": ["game_name"]
                    }
                }
            }),
            // ── Game tools: Process Management ────────────────────────────
            "game_launch" => json!({
                "type": "function",
                "function": {
                    "name": "game_launch",
                    "description": "Launch a game executable with optional arguments",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "executable": {"type": "string", "description": "Path to the game executable"},
                            "args": {"type": "string", "description": "Optional command-line arguments"},
                            "working_dir": {"type": "string", "description": "Working directory for the process"}
                        },
                        "required": ["executable"]
                    }
                }
            }),
            "game_process_list" => json!({
                "type": "function",
                "function": {
                    "name": "game_process_list",
                    "description": "List running game processes, optionally filtered by name",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "filter": {"type": "string", "description": "Optional process name filter"}
                        },
                        "required": []
                    }
                }
            }),
            "game_process_stop" => json!({
                "type": "function",
                "function": {
                    "name": "game_process_stop",
                    "description": "Terminate a game process by PID (single-player use only)",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "pid": {"type": "integer", "description": "Process ID to terminate"},
                            "signal": {"type": "string", "enum": ["term", "kill"], "description": "Signal to send (term=SIGTERM, kill=SIGKILL)"}
                        },
                        "required": ["pid"]
                    }
                }
            }),
            // ── Game tools: Screen Capture & Recording ────────────────────
            "screen_capture" => json!({
                "type": "function",
                "function": {
                    "name": "screen_capture",
                    "description": "Capture a screenshot of the display or a specific window",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "output_path": {"type": "string", "description": "File path to save the screenshot"},
                            "format": {"type": "string", "enum": ["png", "jpg"], "description": "Image format"},
                            "window_name": {"type": "string", "description": "Optional window name/title to capture"}
                        },
                        "required": ["output_path"]
                    }
                }
            }),
            "screen_record" => json!({
                "type": "function",
                "function": {
                    "name": "screen_record",
                    "description": "Record a video of the screen or game window (requires ffmpeg)",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "output_path": {"type": "string", "description": "Output video file path"},
                            "duration_secs": {"type": "integer", "description": "Recording duration in seconds"},
                            "fps": {"type": "integer", "description": "Frames per second"},
                            "window_id": {"type": "string", "description": "X11 window ID (Linux) or display identifier"}
                        },
                        "required": ["output_path"]
                    }
                }
            }),
            "screen_ocr" => json!({
                "type": "function",
                "function": {
                    "name": "screen_ocr",
                    "description": "Extract visible text from a screenshot using OCR (requires tesseract)",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "image_path": {"type": "string", "description": "Path to the screenshot image"},
                            "language": {"type": "string", "description": "OCR language (e.g. eng, chi_sim)"}
                        },
                        "required": ["image_path"]
                    }
                }
            }),
            // ── Game tools: Input Simulation ──────────────────────────────
            "keyboard_input" => json!({
                "type": "function",
                "function": {
                    "name": "keyboard_input",
                    "description": "Send keyboard input to the focused window (accessibility/automation)",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "text": {"type": "string", "description": "Text to type"},
                            "key_delay_ms": {"type": "integer", "description": "Delay between keystrokes in ms"}
                        },
                        "required": ["text"]
                    }
                }
            }),
            "mouse_input" => json!({
                "type": "function",
                "function": {
                    "name": "mouse_input",
                    "description": "Send mouse click at screen coordinates (accessibility/automation)",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "x": {"type": "integer", "description": "X coordinate"},
                            "y": {"type": "integer", "description": "Y coordinate"},
                            "button": {"type": "string", "enum": ["left", "middle", "right"], "description": "Mouse button"},
                            "click_type": {"type": "string", "enum": ["click", "down", "up"], "description": "Click type"}
                        },
                        "required": ["x", "y"]
                    }
                }
            }),
            // ── Game tools: AI Agent & Coaching ───────────────────────────
            "game_agent" => json!({
                "type": "function",
                "function": {
                    "name": "game_agent",
                    "description": "AI game agent — coordinates screen capture, analysis, and input for game automation",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "action": {"type": "string", "enum": ["observe", "act"], "description": "observe=examine game state, act=execute action plan"},
                            "screenshot_path": {"type": "string", "description": "Path to existing screenshot (optional)"},
                            "target": {"type": "string", "description": "Game window name or identifier"},
                            "actions": {"type": "array", "description": "Array of action objects for 'act' mode"}
                        },
                        "required": ["action"]
                    }
                }
            }),
            // ── Game tools: Save & Achievement Management ─────────────────
            "save_file" => json!({
                "type": "function",
                "function": {
                    "name": "save_file",
                    "description": "Manage game save files — list, backup, or restore",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "operation": {"type": "string", "enum": ["list", "backup", "restore"], "description": "Operation to perform"},
                            "game": {"type": "string", "description": "Game name for save file discovery"},
                            "save_path": {"type": "string", "description": "Path to save file (for backup/restore)"},
                            "backup_path": {"type": "string", "description": "Path to backup file (for restore)"}
                        },
                        "required": ["operation", "game"]
                    }
                }
            }),
            "achievement_tracker" => json!({
                "type": "function",
                "function": {
                    "name": "achievement_tracker",
                    "description": "Track game achievements locally — list or unlock",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "game": {"type": "string", "description": "Game name"},
                            "operation": {"type": "string", "enum": ["list", "unlock"], "description": "Operation"},
                            "achievement_id": {"type": "string", "description": "Achievement ID for unlock"}
                        },
                        "required": ["game", "operation"]
                    }
                }
            }),
            // ── Game tools: Mod Management ────────────────────────────────
            "mod_manager" => json!({
                "type": "function",
                "function": {
                    "name": "mod_manager",
                    "description": "List installed mods for supported games",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "game": {"type": "string", "description": "Game name (minecraft, stardew_valley, skyrim)"},
                            "operation": {"type": "string", "enum": ["list"], "description": "Operation"}
                        },
                        "required": ["game", "operation"]
                    }
                }
            }),
            _ => json!({
                "type": "function",
                "function": {
                    "name": tool_name,
                    "description": format!("Execute the {} tool", tool_name),
                    "parameters": {
                        "type": "object",
                        "properties": {},
                        "required": []
                    }
                }
            }),
        }
    }

    /// Return all tool definitions in OpenAI-compatible JSON Schema format.
    pub fn to_openai_tools(&self) -> Vec<Value> {
        self.schema_cache.values().cloned().collect()
    }

    /// Return all tool definitions in Anthropic-compatible format.
    ///
    /// Anthropic uses a slightly different schema: `name`, `description`, `input_schema`.
    pub fn to_anthropic_tools(&self) -> Vec<Value> {
        self.schema_cache
            .values()
            .map(|schema| {
                let func = &schema["function"];
                json!({
                    "name": func["name"],
                    "description": func["description"],
                    "input_schema": func["parameters"]
                })
            })
            .collect()
    }

    /// Parse a native function call response (from OpenAI format) into a ToolInput.
    ///
    /// Expects a JSON object with `name` and `arguments` from the tool_calls response.
    pub fn parse_openai_tool_call(
        &self,
        tool_name: &str,
        arguments: &Value,
        task_id: &str,
        phase: &str,
        agent_role: &str,
        allowed_base_dir: Option<&std::path::Path>,
    ) -> Option<ToolInput> {
        self.registry.get(tool_name)?;
        Some(ToolInput {
            task_id: task_id.to_string(),
            phase: phase.to_string(),
            agent_role: agent_role.to_string(),
            objective: format!("execute tool: {}", tool_name),
            constraints: None,
            evidence: None,
            payload: arguments.clone(),
            allowed_base_dir: allowed_base_dir.map(|p| p.to_path_buf()),
        })
    }

    /// Convert a native tool call into the custom `__tool_call__:` protocol token.
    ///
    /// This is the fallback for providers that do not support native function calling.
    pub fn to_custom_protocol_token(tool_name: &str, arguments_json: &str) -> String {
        build_tool_call_token(tool_name, arguments_json)
    }

    /// Parse a custom protocol token back into tool name and arguments.
    pub fn parse_custom_protocol_token(token: &str) -> Option<(&str, &str)> {
        parse_tool_call_token(token)
    }

    /// Get a reference to the underlying ToolRegistry.
    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builds_openai_tools_from_registry() {
        let registry = ToolRegistry::new();
        let bridge = NativeToolBridge::new(registry);
        let tools = bridge.to_openai_tools();
        // Should have all 16 built-in tools
        assert!(tools.len() >= 6);
        let first = &tools[0];
        assert_eq!(first["type"], "function");
        assert!(first["function"]["name"].is_string());
    }

    #[test]
    fn builds_anthropic_tools_from_registry() {
        let registry = ToolRegistry::new();
        let bridge = NativeToolBridge::new(registry);
        let tools = bridge.to_anthropic_tools();
        assert!(tools.len() >= 6);
        let first = &tools[0];
        assert!(first["name"].is_string());
        assert!(first["input_schema"].is_object());
    }

    #[test]
    fn parses_openai_tool_call_into_input() {
        let registry = ToolRegistry::new();
        let bridge = NativeToolBridge::new(registry);
        let input = bridge
            .parse_openai_tool_call(
                "read_file",
                &json!({"path": "src/main.rs"}),
                "task-1",
                "act",
                "coder",
                None,
            )
            .expect("should parse known tool");
        assert_eq!(input.task_id, "task-1");
        assert_eq!(input.payload["path"], "src/main.rs");
    }

    #[test]
    fn rejects_unknown_tool() {
        let registry = ToolRegistry::new();
        let bridge = NativeToolBridge::new(registry);
        assert!(bridge
            .parse_openai_tool_call("nonexistent", &json!({}), "t1", "act", "coder", None)
            .is_none());
    }

    #[test]
    fn custom_protocol_roundtrip() {
        let token =
            NativeToolBridge::to_custom_protocol_token("read_file", r#"{"path":"test.txt"}"#);
        assert!(token.starts_with("__tool_call__:"));
        let (name, args) =
            NativeToolBridge::parse_custom_protocol_token(&token).expect("should parse");
        assert_eq!(name, "read_file");
        assert_eq!(args, r#"{"path":"test.txt"}"#);
    }
}
