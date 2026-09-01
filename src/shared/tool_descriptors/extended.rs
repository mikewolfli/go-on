//! Descriptors for the extended built-in tools.

use crate::mcp::McpTool;
use serde_json::json;

/// Returns the MCP tool descriptor for a known extended tool name, or `None`.
pub(super) fn descriptor(name: &str) -> Option<McpTool> {
    match name {
        // ── Extended tools with full descriptors ────────────────
        "shell_exec" => Some(McpTool {
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
        }),
        "http_request" => Some(McpTool {
            name: name.to_string(),
            description: Some("Make HTTP requests (GET/POST/PUT/DELETE/PATCH/HEAD/OPTIONS) to external APIs. Only http:// and https:// URLs are allowed. Private/internal IPs are blocked for security.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "method": {"type": "string", "enum": ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"], "description": "HTTP method", "default": "GET"},
                    "url": {"type": "string", "description": "Request URL"},
                    "headers": {"type": "object", "description": "Optional HTTP headers as key-value pairs", "additionalProperties": {"type": "string"}},
                    "body": {"type": "string", "description": "Request body (for POST/PUT/PATCH)"},
                    "auth": {"type": "object", "properties": {"bearer": {"type": "string", "description": "Bearer token for Authorization header"}}},
                    "query": {"type": "object", "description": "Query parameters as key-value pairs", "additionalProperties": {"type": "string"}},
                    "timeout_ms": {"type": "integer", "description": "Timeout in milliseconds (default: 15000)", "default": 15000}
                },
                "required": ["url"]
            })),
        }),
        "grep" => Some(McpTool {
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
        }),

        "git" => Some(McpTool {
            name: name.to_string(),
            // Read-only whitelist: keep in sync with ALLOWED_GIT_SUBCOMMANDS
            // in orchestration/tool/extended/git.rs.
            description: Some(
                "Execute safe, read-only git commands (status, diff, log, show, stash). Returns command output."
                    .to_string(),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "subcommand": {"type": "string", "description": "Git subcommand (e.g. 'status', 'diff', 'log', 'show', 'stash')"},
                    "args": {"type": "array", "items": {"type": "string"}, "description": "Additional arguments for the git command"},
                    "directory": {"type": "string", "description": "Git repository directory (default: current)"}
                },
                "required": ["subcommand"]
            })),
        }),
        "list_directory" => Some(McpTool {
            name: name.to_string(),
            description: Some("List files and directories in a given path.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Directory path to list"}
                },
                "required": ["path"]
            })),
        }),
        "move_path" => Some(McpTool {
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
        }),
        "edit_file" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Edit a file by replacing exact old_text with new_text (single occurrence)."
                    .to_string(),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path to edit"},
                    "old_text": {"type": "string", "description": "Exact text to find and replace"},
                    "new_text": {"type": "string", "description": "Replacement text"}
                },
                "required": ["path", "old_text", "new_text"]
            })),
        }),
        "delete_path" => Some(McpTool {
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
        }),
        "cargo_check" => Some(McpTool {
            name: name.to_string(),
            description: Some("Run 'cargo check' in a Rust project directory to verify compilation without producing artifacts.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "directory": {"type": "string", "description": "Project directory containing Cargo.toml"}
                },
                "required": []
            })),
        }),

        "compress" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Compress a file using gzip compression into an output .gz file.".to_string(),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Source file path"},
                    "output_path": {"type": "string", "description": "Output .gz file path"},
                    "level": {"type": "integer", "description": "gzip compression level 1-9 (default 6)"}
                },
                "required": ["path", "output_path"]
            })),
        }),
        "decompress" => Some(McpTool {
            name: name.to_string(),
            description: Some("Decompress a gzip file into an output file.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Compressed .gz file path"},
                    "output_path": {"type": "string", "description": "Output file path"}
                },
                "required": ["path", "output_path"]
            })),
        }),
        "date_time" => Some(McpTool {
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
        }),
        "dns_lookup" => Some(McpTool {
            name: name.to_string(),
            description: Some("Perform a DNS lookup for a hostname, returning IP addresses.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "hostname": {"type": "string", "description": "Hostname to look up"}
                },
                "required": ["hostname"]
            })),
        }),
        "ping" => Some(McpTool {
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
        }),
        "port_scan" => Some(McpTool {
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
        }),
        "skill_execute" => Some(McpTool {
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
        }),
        "skill_create" => Some(McpTool {
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
        }),
        "skill_reload" => Some(McpTool {
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
        }),
        "skill_list" => Some(McpTool {
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
        }),
        "create_directory" => Some(McpTool {
            name: name.to_string(),
            description: Some("Create a new directory (including parent directories if needed)".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Directory path to create"}
                },
                "required": ["path"]
            })),
        }),
        "copy_path" => Some(McpTool {
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
        }),
        "read_file_lines" => Some(McpTool {
            name: name.to_string(),
            description: Some("Read specific lines from a file by line number range. Returns the requested lines with their line numbers.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path to read from"},
                    "start_line": {"type": "integer", "description": "First line number to read (1-based, inclusive)", "default": 1},
                    "end_line": {"type": "integer", "description": "Last line number to read (1-based, inclusive)", "default": 50}
                },
                "required": ["path"]
            })),
        }),
        "file_diff" => Some(McpTool {
            name: name.to_string(),
            description: Some("Compare two files and return the unified diff".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "file_a": {"type": "string", "description": "Path to the first (original) file"},
                    "file_b": {"type": "string", "description": "Path to the second (modified) file"},
                    "context_lines": {"type": "integer", "description": "Number of context lines around each change (default: 3)", "default": 3}
                },
                "required": ["file_a", "file_b"]
            })),
        }),
        "code_index_search" => Some(McpTool {
            name: name.to_string(),
            description: Some("Search across indexed codebases for symbols, definitions, or references.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query (symbol name, function name, etc.)"}
                },
                "required": ["query"]
            })),
        }),
        "archive_inspect" => Some(McpTool {
            name: name.to_string(),
            description: Some("List the contents of an archive file (zip, tar, tar.gz) without extracting it.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the archive file"}
                },
                "required": ["path"]
            })),
        }),
        "archive_extract" => Some(McpTool {
            name: name.to_string(),
            description: Some("Extract an archive file (zip, tar, tar.gz).".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the archive file"},
                    "output_dir": {"type": "string", "description": "Destination directory"}
                },
                "required": ["path", "output_dir"]
            })),
        }),
        "diagnostics" => Some(McpTool {
            name: name.to_string(),
            description: Some("Get project compilation errors and warnings. Optionally specify a file path to scope results.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Optional file path to scope diagnostics (default: entire project)"}
                },
                "required": []
            })),
        }),
        "environment_info" => Some(McpTool {
            name: name.to_string(),
            description: Some("Get information about the current environment: OS, CPU, memory, disk, and language runtime details.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {},
                "required": []
            })),
        }),
        "rss_read" => Some(McpTool {
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
        }),
        "jsonl_read" => Some(McpTool {
            name: name.to_string(),
            description: Some("Read a JSONL (JSON Lines) file and return parsed JSON objects.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the JSONL file"},
                    "limit": {"type": "integer", "description": "Maximum number of lines to read (default: 1000)"}
                },
                "required": ["path"]
            })),
        }),
        "web_search" => Some(McpTool {
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
                }),
        "go_to_definition" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Find the definition of a symbol (fn, struct, enum, trait, impl, type, const) "
                    .to_string()
                    + "in the codebase. Searches source files for declaration patterns. "
                    + "Returns file path, line number, and surrounding context.",
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "symbol": {"type": "string", "description": "The symbol name to find the definition of"},
                    "directory": {"type": "string", "description": "Optional directory scope (default: project root)"},
                    "language": {
                        "type": "string",
                        "enum": ["auto", "rust", "python", "typescript", "javascript", "go", "java"],
                        "description": "Language hint for definition patterns (default: auto-detect from file extension)"
                    },
                    "lsp_address": {"type": "string", "description": "Optional LSP TCP address (e.g. '127.0.0.1:9258'). Uses LSP protocol for accurate results. Requires path, line, and column."},
                    "path": {"type": "string", "description": "File path for the symbol usage position (required with lsp_address)"},
                    "line": {"type": "integer", "description": "Line number (1-based) for the symbol usage (required with lsp_address)"},
                    "column": {"type": "integer", "description": "Column number (1-based) for the symbol usage (required with lsp_address)"}
                },
                "required": ["symbol"]
            })),
        }),
        "find_references" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Find all references to a symbol across the codebase. "
                    .to_string()
                    + "Searches source files for the symbol name and returns matching "
                    + "file paths, line numbers, and surrounding lines.",
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "symbol": {"type": "string", "description": "The symbol name to find references for"},
                    "directory": {"type": "string", "description": "Optional directory scope (default: project root)"},
                    "include": {"type": "string", "description": "Optional glob pattern to filter files (e.g. '**/*.rs')"},
                    "lsp_address": {"type": "string", "description": "Optional LSP TCP address (e.g. '127.0.0.1:9258'). Uses LSP protocol for accurate results. Requires path, line, and column."},
                    "path": {"type": "string", "description": "File path for the symbol usage position (required with lsp_address)"},
                    "line": {"type": "integer", "description": "Line number (1-based) for the symbol usage (required with lsp_address)"},
                    "column": {"type": "integer", "description": "Column number (1-based) for the symbol usage (required with lsp_address)"}
                },
                "required": ["symbol"]
            })),
        }),
        "apply_code_action" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Apply code actions at a specific location. "
                    .to_string()
                    + "Supported actions: add_import (insert a use/import statement), "
                    + "fix_lint (add #[allow(...)] attribute), "
                    + "auto_fix_diagnostic (run cargo clippy --fix).",
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path to apply the action at"},
                    "action": {
                        "type": "string",
                        "enum": ["add_import", "fix_lint", "auto_fix_diagnostic"],
                        "description": "Type of code action to apply"
                    },
                    "detail": {"type": "string", "description": "Action-specific detail (e.g. 'HashMap' for add_import, or the lint rule name)"},
                    "line": {"type": "integer", "description": "Line number for the action (1-based, default: 1)"},
                    "lsp_address": {"type": "string", "description": "Optional LSP TCP address (e.g. '127.0.0.1:9258'). Uses LSP protocol for code actions."},
                    "column": {"type": "integer", "description": "Column number (1-based, used with lsp_address, default: 1)"}
                },
                "required": ["path", "action"]
            })),
        }),
        "jsonl_write" => Some(McpTool {
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
        }),
        "format_code" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Auto-format code files using the appropriate formatter. "
                    .to_string()
                    + "Detects formatter by file extension: .rs->rustfmt, .js/.ts->prettier, "
                    + ".py->black, .go->gofmt, .java->google-java-format, .c/.h->clang-format.",
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File or directory path to format"},
                    "check": {"type": "boolean", "description": "Check mode: only report if files need formatting"},
                    "formatter": {"type": "string", "description": "Optional formatter override"}
                },
                "required": ["path"]
            })),
        }),
        "search_packages" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Search package registries for available libraries. ".to_string()
                    + "Supports: crates.io (rust), npm (js/ts), pypi (python), go (golang). "
                    + "Returns package name, description, version, and download counts.",
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query"},
                    "registry": {
                        "type": "string",
                        "enum": ["auto", "crates.io", "npm", "pypi", "go"],
                        "description": "Package registry to search"
                    },
                    "max_results": {"type": "integer", "description": "Maximum results (default: 5, max: 20)"}
                },
                "required": ["query"]
            })),
        }),
        "uuid_gen" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Generate a UUID v4 (random). Returns a universally unique identifier string."
                    .to_string(),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {},
                "required": []
            })),
        }),
        "random_token" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Generate a cryptographically secure random token. ".to_string()
                    + "Supports: hex, base64, alphanumeric formats. Default: 32-char hex.",
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "length": {"type": "integer", "description": "Token length in characters (default: 32)"},
                    "format": {
                        "type": "string",
                        "enum": ["hex", "base64", "alphanumeric"],
                        "description": "Token format (default: hex)"
                    }
                },
                "required": []
            })),
        }),
        "encode_decode" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Encode or decode data using various formats. ".to_string()
                    + "Supports: base64, hex, url encoding/decoding.",
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "operation": {
                        "type": "string",
                        "enum": ["base64_encode", "base64_decode", "hex_encode", "hex_decode", "url_encode", "url_decode"],
                        "description": "Encoding/decoding operation"
                    },
                    "input": {"type": "string", "description": "Input text to encode or decode"}
                },
                "required": ["operation", "input"]
            })),
        }),
        "hash_file" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Compute a cryptographic hash of a file. ".to_string()
                    + "Supports SHA-256 (default) and SHA-512. Returns the hash as a hex string.",
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path to hash"},
                    "algorithm": {
                        "type": "string",
                        "enum": ["sha256", "sha512"],
                        "description": "Hash algorithm (default: sha256)"
                    }
                },
                "required": ["path"]
            })),
        }),
        "spawn_agent" => Some(McpTool {
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
        }),
        "memory_search" => Some(McpTool {
            name: name.to_string(),
            description: Some("Full-text search across cross-session memory (warm tier). Returns memory entries whose content matches the query, ranked by relevance. Supports Chinese/Japanese/Korean (CJK) substring queries.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query text"},
                    "limit": {"type": "integer", "description": "Maximum number of hits to return (default: 10, max: 50)", "default": 10, "minimum": 1, "maximum": 50}
                },
                "required": ["query"]
            })),
        }),
        _ => None,
    }
}
