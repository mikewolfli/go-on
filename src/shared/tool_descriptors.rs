//! Shared tool descriptor and validation functions.
//!
//! These functions are shared between the ACP request handler and the MCP tools
//! module to eliminate code duplication for built-in tool descriptors and
//! argument validation.

use anyhow::Result;
use serde_json::{json, Value};

use crate::mcp::McpTool;

/// Get the MCP tool descriptor for a given built-in tool name.
///
/// Returns a fully populated `McpTool` with name, description, and input schema.
/// For unknown tool names, returns a generic descriptor.
pub fn tool_descriptor(name: &'static str) -> McpTool {
    match name {
        "read_file" => McpTool {
            name: name.to_string(),
            description: Some("Read contents of a file".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path to read"}
                },
                "required": ["path"]
            })),
        },
        "write_file" => McpTool {
            name: name.to_string(),
            description: Some("Write contents to a file".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path to write"},
                    "content": {"type": "string", "description": "Content to write"},
                    "mode": {"type": "string", "enum": ["overwrite", "append"], "description": "Write mode"}
                },
                "required": ["path", "content"]
            })),
        },
        "search_files" => McpTool {
            name: name.to_string(),
            description: Some("Search for files matching a glob pattern".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Search pattern/glob"},
                    "directory": {"type": "string", "description": "Search directory"}
                },
                "required": ["pattern"]
            })),
        },
        "apply_patch" => McpTool {
            name: name.to_string(),
            description: Some("Apply a patch artifact".to_string()),
            input_schema: Some(json!({"type": "object"})),
        },
        "run_tests" => McpTool {
            name: name.to_string(),
            description: Some("Run test suite".to_string()),
            input_schema: Some(json!({"type": "object"})),
        },
        "inspect_git_diff" => McpTool {
            name: name.to_string(),
            description: Some("Inspect git diff".to_string()),
            input_schema: Some(json!({"type": "object"})),
        },
        "workflow_execute" => McpTool {
            name: name.to_string(),
            description: Some("Execute a workflow with the given task description".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string", "description": "Task description for the workflow"},
                    "phase": {"type": "string", "description": "Optional phase name (default: coding)"}
                },
                "required": ["task"]
            })),
        },
        "workflow_ask" => McpTool {
            name: name.to_string(),
            description: Some(
                "Ask the AI to analyze a task, create necessary skills, and execute a workflow"
                    .to_string(),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string", "description": "Natural language task description"},
                    "auto_create_skills": {"type": "boolean", "description": "Auto-create skills for workflow nodes"},
                },
                "required": ["task"]
            })),
        },
        "workflow_generate" => McpTool {
            name: name.to_string(),
            description: Some(
                "Generate a workflow plan from a task description without executing it".to_string(),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string", "description": "Task description to plan"},
                },
                "required": ["task"]
            })),
        },
        "skill-creator" => McpTool {
            name: name.to_string(),
            description: Some(
                "Create a new reusable skill from a prompt template (SKILL-CREATOR)".to_string(),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Skill name"},
                    "description": {"type": "string", "description": "Skill description"},
                    "prompt_template": {"type": "string", "description": "Prompt template for the skill"},
                    "input_schema": {"type": "object", "description": "JSON schema for skill input"}
                },
                "required": ["name", "description", "prompt_template"]
            })),
        },
        "github_search_skills" => McpTool {
            name: name.to_string(),
            description: Some(
                "Search GitHub for skill repositories matching a query. ".to_string()
                    + "Returns repos that may contain installable skills. "
                    + "Use 'import_skill' with the chosen repo to install.",
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query (e.g. 'web scraping', 'code review')"},
                    "max_results": {"type": "integer", "description": "Max results to return (1-20)", "default": 10, "minimum": 1, "maximum": 20}
                },
                "required": ["query"]
            })),
        },
        "import_skill" => McpTool {
            name: name.to_string(),
            description: Some(
                "Import a skill from a remote URL or GitHub repository. ".to_string()
                    + "Downloads the skill manifest and registers it locally. "
                    + "Supports GitHub repos (e.g. 'owner/repo') and direct URLs.",
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "source": {
                        "type": "object",
                        "oneOf": [
                            {
                                "title": "GitHub",
                                "type": "object",
                                "properties": {
                                    "repo": {"type": "string", "description": "GitHub repository (owner/repo)"},
                                    "ref": {"type": "string", "description": "Git ref (branch/tag/commit), default: main", "default": "main"},
                                    "path": {"type": "string", "description": "Path within the repo"}
                                },
                                "required": ["repo"]
                            },
                            {
                                "title": "URL",
                                "type": "object",
                                "properties": {
                                    "url": {"type": "string", "description": "Direct URL to the skill manifest JSON"}
                                },
                                "required": ["url"]
                            }
                        ],
                        "description": "Source of the skill to import"
                    }
                },
                "required": ["source"]
            })),
        },
        // ── Extended tools with full descriptors ────────────────
        "shell_exec" => McpTool {
            name: name.to_string(),
            description: Some("Execute a shell command with timeout. Returns stdout, stderr, and exit code.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Shell command to execute"},
                    "timeout_ms": {"type": "integer", "description": "Timeout in milliseconds (default: 30000)", "default": 30000},
                    "directory": {"type": "string", "description": "Working directory (default: current)"}
                },
                "required": ["command"]
            })),
        },
        "http_request" => McpTool {
            name: name.to_string(),
            description: Some("Make an HTTP request to a URL. Supports GET, POST, PUT, DELETE, PATCH, HEAD.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "method": {"type": "string", "enum": ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD"], "description": "HTTP method", "default": "GET"},
                    "url": {"type": "string", "description": "Request URL"},
                    "headers": {"type": "object", "description": "Optional HTTP headers as key-value pairs"},
                    "body": {"type": "string", "description": "Request body (for POST/PUT/PATCH)"},
                    "timeout_ms": {"type": "integer", "description": "Timeout in milliseconds", "default": 30000}
                },
                "required": ["url"]
            })),
        },
        "grep" => McpTool {
            name: name.to_string(),
            description: Some("Search file contents using a regular expression pattern. Returns matching lines with file paths.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Regex pattern to search for"},
                    "include": {"type": "string", "description": "Optional glob pattern to filter files (e.g. '**/*.rs')"},
                    "directory": {"type": "string", "description": "Search directory (default: current)"}
                },
                "required": ["pattern"]
            })),
        },
        "find_files" => McpTool {
            name: name.to_string(),
            description: Some("Find files matching a glob pattern. Returns list of matching file paths.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Glob pattern (e.g. '**/*.rs', '*.toml')"},
                    "directory": {"type": "string", "description": "Search directory (default: current)"}
                },
                "required": ["pattern"]
            })),
        },
        "git" => McpTool {
            name: name.to_string(),
            description: Some("Execute git commands (status, diff, log, add, commit, etc.). Returns command output.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "subcommand": {"type": "string", "description": "Git subcommand (e.g. 'status', 'diff', 'log', 'add', 'commit')"},
                    "args": {"type": "array", "items": {"type": "string"}, "description": "Additional arguments for the git command"},
                    "directory": {"type": "string", "description": "Git repository directory (default: current)"}
                },
                "required": ["subcommand"]
            })),
        },
        "list_directory" => McpTool {
            name: name.to_string(),
            description: Some("List files and directories in a given path.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Directory path to list"}
                },
                "required": ["path"]
            })),
        },
        "file_move" => McpTool {
            name: name.to_string(),
            description: Some("Move or rename a file or directory.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "source": {"type": "string", "description": "Source path"},
                    "destination": {"type": "string", "description": "Destination path"}
                },
                "required": ["source", "destination"]
            })),
        },
        "file_delete" => McpTool {
            name: name.to_string(),
            description: Some("Delete a file (requires explicit confirmation).".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the file to delete"},
                    "confirm": {"type": "boolean", "description": "Must be true to confirm deletion"}
                },
                "required": ["path", "confirm"]
            })),
        },
        "cargo_check" => McpTool {
            name: name.to_string(),
            description: Some("Run 'cargo check' in a Rust project directory to verify compilation without producing artifacts.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "directory": {"type": "string", "description": "Project directory containing Cargo.toml"},
                    "features": {"type": "string", "description": "Optional feature flags (e.g. '--features local')"}
                },
                "required": []
            })),
        },
        "cargo_test" => McpTool {
            name: name.to_string(),
            description: Some("Run 'cargo test' in a Rust project directory.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "directory": {"type": "string", "description": "Project directory containing Cargo.toml"},
                    "filter": {"type": "string", "description": "Optional test name filter (alphanumeric only)"},
                    "features": {"type": "string", "description": "Optional feature flags"}
                },
                "required": []
            })),
        },
        "compress" => McpTool {
            name: name.to_string(),
            description: Some("Compress a file or directory into a compressed archive (zip/tar.gz).".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "source": {"type": "string", "description": "Source file or directory path"},
                    "destination": {"type": "string", "description": "Output archive path"},
                    "format": {"type": "string", "enum": ["zip", "tar.gz"], "description": "Archive format", "default": "zip"}
                },
                "required": ["source", "destination"]
            })),
        },
        "decompress" => McpTool {
            name: name.to_string(),
            description: Some("Decompress an archive file (zip/tar.gz/tar.bz2).".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "source": {"type": "string", "description": "Archive file path"},
                    "destination": {"type": "string", "description": "Output directory (default: current)"}
                },
                "required": ["source"]
            })),
        },
        "date_time" => McpTool {
            name: name.to_string(),
            description: Some("Get current date/time, or convert between timezones / formats.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "format": {"type": "string", "description": "Output format (e.g. 'iso', 'unix', 'rfc2822')", "default": "iso"},
                    "timezone": {"type": "string", "description": "Target timezone (e.g. 'UTC', 'Asia/Shanghai')"}
                },
                "required": []
            })),
        },
        "dns_lookup" => McpTool {
            name: name.to_string(),
            description: Some("Perform a DNS lookup for a hostname, returning IP addresses.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "hostname": {"type": "string", "description": "Hostname to look up"}
                },
                "required": ["hostname"]
            })),
        },
        "ping" => McpTool {
            name: name.to_string(),
            description: Some("Ping a host to check network reachability.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "host": {"type": "string", "description": "Hostname or IP address to ping"},
                    "count": {"type": "integer", "description": "Number of ping packets", "default": 4}
                },
                "required": ["host"]
            })),
        },
        "port_scan" => McpTool {
            name: name.to_string(),
            description: Some("Scan a range of TCP ports on a host.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "host": {"type": "string", "description": "Hostname or IP address"},
                    "ports": {"type": "string", "description": "Port range (e.g. '80,443' or '1-1024')"},
                    "timeout_ms": {"type": "integer", "description": "Timeout per port", "default": 1000}
                },
                "required": ["host"]
            })),
        },
        "skill_execute" => McpTool {
            name: name.to_string(),
            description: Some(
                "Execute a registered skill by name with the given input. ".to_string()
                    + "Skills are reusable prompt templates or programmatic capabilities. ",
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "skill_name": {"type": "string", "description": "Name of the skill to execute"},
                    "input": {"type": "object", "description": "Input parameters for the skill"}
                },
                "required": ["skill_name"]
            })),
        },
        "skill_create" => McpTool {
            name: name.to_string(),
            description: Some(
                "Create a new reusable skill from a prompt template. ".to_string()
                    + "The skill is immediately registered and available via skill_execute.",
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Skill name (alphanumeric + hyphens)"},
                    "description": {"type": "string", "description": "Human-readable description"},
                    "prompt_template": {"type": "string", "description": "Prompt template for the skill (may include {{input}} placeholder)"},
                    "input_schema": {"type": "object", "description": "Optional JSON schema for skill input parameters"}
                },
                "required": ["name", "description", "prompt_template"]
            })),
        },
        "skill_reload" => McpTool {
            name: name.to_string(),
            description: Some(
                "Immediately reload skills from the local skills directory. ".to_string()
                    + "Scans ~/.agents/skills/ for new or changed SKILL.md files and registers them. "
                    + "Returns counts of new, skipped, and errored skills.",
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "directory": {"type": "string", "description": "Optional custom skills directory (default: ~/.agents/skills/)"}
                },
                "required": []
            })),
        },
        "skill_list" => McpTool {
            name: name.to_string(),
            description: Some(
                "List all registered skills with their name, description, and score. ".to_string()
                    + "No arguments required — returns an array of skill descriptors.",
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {},
                "required": []
            })),
        },
        "create_directory" => McpTool {
            name: name.to_string(),
            description: Some("Create a new directory (including parent directories if needed)".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Directory path to create"}
                },
                "required": ["path"]
            })),
        },
        "copy_path" => McpTool {
            name: name.to_string(),
            description: Some("Copy a file or directory from source to destination".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "source": {"type": "string", "description": "Source path"},
                    "destination": {"type": "string", "description": "Destination path"}
                },
                "required": ["source", "destination"]
            })),
        },
        "read_file_lines" => McpTool {
            name: name.to_string(),
            description: Some("Read specific lines from a file by line number range. Returns the requested lines with their line numbers.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path to read from"},
                    "start_line": {"type": "integer", "description": "Starting line number (1-based)"},
                    "end_line": {"type": "integer", "description": "Ending line number (inclusive)"}
                },
                "required": ["path", "start_line", "end_line"]
            })),
        },
        "file_diff" => McpTool {
            name: name.to_string(),
            description: Some("Compare two files and show the differences between them.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "file1": {"type": "string", "description": "First file path"},
                    "file2": {"type": "string", "description": "Second file path"}
                },
                "required": ["file1", "file2"]
            })),
        },
        "code_index_search" => McpTool {
            name: name.to_string(),
            description: Some("Search across indexed codebases for symbols, definitions, or references.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query (symbol name, function name, etc.)"}
                },
                "required": ["query"]
            })),
        },
        "archive_inspect" => McpTool {
            name: name.to_string(),
            description: Some("List the contents of an archive file (zip, tar, tar.gz) without extracting it.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the archive file"}
                },
                "required": ["path"]
            })),
        },
        "archive_extract" => McpTool {
            name: name.to_string(),
            description: Some("Extract an archive file (zip, tar, tar.gz) to a destination directory.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the archive file"},
                    "destination": {"type": "string", "description": "Destination directory (default: current directory)"}
                },
                "required": ["path"]
            })),
        },
        "diagnostics" => McpTool {
            name: name.to_string(),
            description: Some("Get project compilation errors and warnings. Optionally specify a file path to scope results.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Optional file path to scope diagnostics (default: entire project)"}
                },
                "required": []
            })),
        },
        "environment_info" => McpTool {
            name: name.to_string(),
            description: Some("Get information about the current environment: OS, CPU, memory, disk, and language runtime details.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {},
                "required": []
            })),
        },
        "rss_read" => McpTool {
            name: name.to_string(),
            description: Some("Fetch and parse an RSS/Atom feed from a URL. Returns feed entries with titles, links, and descriptions.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "RSS/Atom feed URL"},
                    "max_items": {"type": "integer", "description": "Maximum number of items to return (default: 20)"}
                },
                "required": ["url"]
            })),
        },
        "jsonl_read" => McpTool {
            name: name.to_string(),
            description: Some("Read a JSONL (JSON Lines) file and return parsed JSON objects.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the JSONL file"},
                    "limit": {"type": "integer", "description": "Maximum number of lines to read (default: all)"}
                },
                "required": ["path"]
            })),
        },
        "web_search" => McpTool {
            name: name.to_string(),
            description: Some("Search the web for information. Returns a list of results with titles, URLs, and snippets. Uses DuckDuckGo by default (free, no API key needed).".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query (required)"},
                    "max_results": {"type": "integer", "description": "Maximum number of results to return (default: 5)", "default": 5, "minimum": 1, "maximum": 20}
                },
                "required": ["query"]
            })),
        },
        "jsonl_write" => McpTool {
            name: name.to_string(),
            description: Some("Write data as JSONL (JSON Lines) to a file. Each object is written as a separate line.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Output file path"},
                    "data": {"type": "array", "description": "Array of JSON objects to write as lines"}
                },
                "required": ["path", "data"]
            })),
        },
        "spawn_agent" => McpTool {
            name: name.to_string(),
            description: Some(
                "Spawn a sub-agent with a specific task, wait for it to complete, and return the result. "
                    .to_string(),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "The task for the sub-agent to perform"
                    },
                    "agent_name": {
                        "type": "string",
                        "description": "Which agent to use (e.g. \"deepseek\", \"copilot\"). Default: \"deepseek\"",
                        "default": "deepseek"
                    },
                    "model": {
                        "type": "string",
                        "description": "Optional model override (e.g. \"deepseek-v4-flash\")"
                    },
                    "timeout_seconds": {
                        "type": "number",
                        "description": "Timeout in seconds. Default: 120, max: 300",
                        "default": 120
                    }
                },
                "required": ["task"]
            })),
        },
        other => McpTool {
            name: other.to_string(),
            description: Some("Registered MCP tool".to_string()),
            input_schema: Some(json!({"type": "object"})),
        },
    }
}

/// Get the JSON `Value` representation of a tool descriptor (used by ACP handler).
///
/// This is a convenience wrapper around `tool_descriptor` that returns the
/// serialized JSON value. It is used by the ACP request handler in `request.rs`
/// for building MCP tool descriptor lists.
pub fn tool_descriptor_value(name: &'static str) -> Value {
    let tool = tool_descriptor(name);
    serde_json::to_value(tool).unwrap_or_else(|_| {
        json!({
            "name": name,
            "description": "Registered MCP tool",
            "input_schema": {"type": "object"}
        })
    })
}

/// Validate required arguments for a built-in tool.
///
/// Checks that the tool's required arguments are present in the provided input.
/// Returns an error with a descriptive message if any required argument is missing.
pub fn validate_required_arguments(tool_name: &str, tool_input: &Value) -> Result<()> {
    match tool_name {
        "read_file" => {
            tool_input
                .get("path")
                .and_then(|value| value.as_str())
                .ok_or_else(|| anyhow::anyhow!("read_file requires arguments.path"))?;
        }
        "write_file" => {
            tool_input
                .get("path")
                .and_then(|value| value.as_str())
                .ok_or_else(|| anyhow::anyhow!("write_file requires arguments.path"))?;
            tool_input
                .get("content")
                .and_then(|value| value.as_str())
                .ok_or_else(|| anyhow::anyhow!("write_file requires arguments.content"))?;
        }
        "search_files" => {
            tool_input
                .get("pattern")
                .and_then(|value| value.as_str())
                .ok_or_else(|| anyhow::anyhow!("search_files requires arguments.pattern"))?;
        }
        "github_search_skills" => {
            tool_input
                .get("query")
                .and_then(|value| value.as_str())
                .ok_or_else(|| anyhow::anyhow!("github_search_skills requires arguments.query"))?;
        }
        "workflow_execute" | "workflow_ask" | "workflow_generate"
            if tool_input.get("task").is_none() =>
        {
            return Err(anyhow::anyhow!("{} tool requires 'task' field", tool_name));
        }
        // ── Extended tool validation ────────────────────────
        "shell_exec" => {
            tool_input
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("shell_exec requires arguments.command"))?;
        }
        "grep" => {
            tool_input
                .get("pattern")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("grep requires arguments.pattern"))?;
        }
        "find_files" => {
            tool_input
                .get("pattern")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("find_files requires arguments.pattern"))?;
        }
        "list_directory" => {
            tool_input
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("list_directory requires arguments.path"))?;
        }
        "file_move" => {
            tool_input
                .get("source")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("file_move requires arguments.source"))?;
            tool_input
                .get("destination")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("file_move requires arguments.destination"))?;
        }
        "file_delete" => {
            tool_input
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("file_delete requires arguments.path"))?;
            tool_input
                .get("confirm")
                .and_then(|v| v.as_bool())
                .ok_or_else(|| {
                    anyhow::anyhow!("file_delete requires arguments.confirm (boolean)")
                })?;
        }
        "git" => {
            tool_input
                .get("subcommand")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("git requires arguments.subcommand"))?;
        }
        "compress" | "decompress" => {
            tool_input
                .get("source")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("{} requires arguments.source", tool_name))?;
        }
        "dns_lookup" => {
            tool_input
                .get("hostname")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("dns_lookup requires arguments.hostname"))?;
        }
        "ping" => {
            tool_input
                .get("host")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("ping requires arguments.host"))?;
        }
        "port_scan" => {
            tool_input
                .get("host")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("port_scan requires arguments.host"))?;
        }
        "skill_execute" => {
            tool_input
                .get("skill_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("skill_execute requires arguments.skill_name"))?;
        }
        "skill_create" => {
            tool_input
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("skill_create requires arguments.name"))?;
            tool_input
                .get("description")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("skill_create requires arguments.description"))?;
            tool_input
                .get("prompt_template")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    anyhow::anyhow!("skill_create requires arguments.prompt_template")
                })?;
        }
        "skill_reload" => {}
        "workflow_execute" | "workflow_ask" | "workflow_generate" => {}
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// List of all known built-in tool names.
    const KNOWN_TOOLS: &[&str] = &[
        "read_file",
        "write_file",
        "search_files",
        "apply_patch",
        "run_tests",
        "inspect_git_diff",
        "workflow_execute",
        "workflow_ask",
        "workflow_generate",
        "skill-creator",
        "github_search_skills",
        "import_skill",
        // Extended tools with full descriptors
        "shell_exec",
        "http_request",
        "grep",
        "find_files",
        "git",
        "list_directory",
        "file_move",
        "file_delete",
        "cargo_check",
        "cargo_test",
        "compress",
        "decompress",
        "date_time",
        "dns_lookup",
        "ping",
        "port_scan",
        // Skill tools
        "skill_execute",
        "skill_list",
        // Round 2 additions
        "diagnostics",
        "environment_info",
        // Web search
        "web_search",
    ];

    // ── Known tool descriptors ───────────────────────────────────────

    /// Verify that known tools have valid descriptors with name, description,
    /// and input_schema populated.
    #[test]
    fn test_known_tools_have_valid_descriptors() {
        for &name in KNOWN_TOOLS {
            let desc = tool_descriptor(name);
            assert_eq!(desc.name, name, "descriptor name should match for {}", name);
            assert!(
                desc.description.is_some(),
                "descriptor for {} should have a description",
                name
            );
            assert!(
                desc.description.as_deref().unwrap().len() > 5,
                "description for {} should be meaningful (length > 5)",
                name
            );
            assert!(
                desc.input_schema.is_some(),
                "descriptor for {} should have input_schema",
                name
            );
        }
    }

    /// Verify that tools with required arguments are properly reflected.
    #[test]
    fn test_tool_descriptors_have_correct_schema_format() {
        let desc = tool_descriptor("read_file");
        let schema = desc.input_schema.unwrap();
        assert_eq!(schema["type"], "object");
        let required = schema["required"].as_array().unwrap();
        assert!(
            required.contains(&serde_json::Value::String("path".to_string())),
            "read_file should require 'path'"
        );

        let desc = tool_descriptor("write_file");
        let schema = desc.input_schema.unwrap();
        let required = schema["required"].as_array().unwrap();
        assert!(
            required.contains(&serde_json::Value::String("path".to_string())),
            "write_file should require 'path'"
        );
        assert!(
            required.contains(&serde_json::Value::String("content".to_string())),
            "write_file should require 'content'"
        );
    }

    // ── validate_required_arguments ──────────────────────────────────

    /// Verify that `validate_required_arguments` passes for known tools
    /// with correct inputs.
    #[test]
    fn test_validate_required_arguments_known_tools() {
        // read_file requires path
        assert!(validate_required_arguments("read_file", &json!({"path": "foo.txt"})).is_ok());
        // write_file requires path + content
        assert!(validate_required_arguments(
            "write_file",
            &json!({"path": "foo.txt", "content": "hello"})
        )
        .is_ok());
        // search_files requires pattern
        assert!(validate_required_arguments("search_files", &json!({"pattern": "*.rs"})).is_ok());
        // workflow_execute requires task
        assert!(
            validate_required_arguments("workflow_execute", &json!({"task": "do something"}))
                .is_ok()
        );
        // workflow_ask requires task
        assert!(
            validate_required_arguments("workflow_ask", &json!({"task": "analyze this"})).is_ok()
        );
        // workflow_generate requires task
        assert!(
            validate_required_arguments("workflow_generate", &json!({"task": "plan this"})).is_ok()
        );
    }

    /// Verify that `validate_required_arguments` rejects missing arguments.
    #[test]
    fn test_validate_required_arguments_missing() {
        // read_file without path
        let err = validate_required_arguments("read_file", &json!({})).unwrap_err();
        assert!(err
            .to_string()
            .contains("read_file requires arguments.path"));

        // write_file without content
        let err = validate_required_arguments("write_file", &json!({"path": "x.txt"})).unwrap_err();
        assert!(err
            .to_string()
            .contains("write_file requires arguments.content"));

        // search_files without pattern
        let err = validate_required_arguments("search_files", &json!({})).unwrap_err();
        assert!(err
            .to_string()
            .contains("search_files requires arguments.pattern"));
    }

    /// Verify that unknown tools are validated successfully (no required args).
    #[test]
    fn test_validate_required_arguments_unknown_tool() {
        // Unknown tools have no validation rules, so any input should pass
        assert!(validate_required_arguments("unknown_tool", &json!({})).is_ok());
        assert!(validate_required_arguments("unknown_tool", &json!({"anything": 42})).is_ok());
    }

    // ── Unknown tools ────────────────────────────────────────────────

    /// Verify that unknown tools get a generic descriptor with "Registered MCP tool"
    /// description and an empty object schema.
    #[test]
    fn test_unknown_tool_gets_generic_descriptor() {
        let desc = tool_descriptor("some_unknown_tool");
        assert_eq!(desc.name, "some_unknown_tool");
        assert_eq!(
            desc.description.as_deref(),
            Some("Registered MCP tool"),
            "unknown tools should get a generic description"
        );
        let schema = desc.input_schema.unwrap();
        assert_eq!(schema["type"], "object");
    }

    /// Verify that `tool_descriptor_value` also returns generic structure for unknown tools.
    #[test]
    fn test_unknown_tool_descriptor_value() {
        let val = tool_descriptor_value("nonexistent");
        assert_eq!(val["name"], "nonexistent");
        assert_eq!(val["description"], "Registered MCP tool");
    }
}
