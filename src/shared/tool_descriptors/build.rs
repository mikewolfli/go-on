//! Descriptors for build/lint, data query, template, metrics, security,
//! docker, and file watch tools.

use crate::mcp::McpTool;
use serde_json::json;

/// Returns the MCP tool descriptor for a known build/ops tool name, or `None`.
pub(super) fn descriptor(name: &str) -> Option<McpTool> {
    match name {
        // ── Build/lint/dependency tools (P1) ─────────────────────
        "build_run" => Some(McpTool {
            name: name.to_string(),
            description: Some("Detect and run the project's build system: cargo build, npm run build, python -m build, or make".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "directory": {"type": "string", "description": "Project directory (default: current)"}
                },
                "required": []
            })),
        }),
        "lint_run" => Some(McpTool {
            name: name.to_string(),
            description: Some("Detect and run the project's linter: cargo clippy, npx eslint, ruff, or pylint".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "directory": {"type": "string", "description": "Project directory (default: current)"}
                },
                "required": []
            })),
        }),
        "dependency_add" => Some(McpTool {
            name: name.to_string(),
            description: Some("Add a dependency to the project: cargo add, npm install, pip install, or go get".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "package": {"type": "string", "description": "Package/dependency name to add"},
                    "directory": {"type": "string", "description": "Project directory (default: current)"}
                },
                "required": ["package"]
            })),
        }),
        // ── Structured data query tools (P1) ─────────────────────
        "json_query" => Some(McpTool {
            name: name.to_string(),
            description: Some("Read a JSON file and query it using a simple path syntax like 'obj.key[0].nested'".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the JSON file"},
                    "query": {"type": "string", "description": "Query path like 'obj.key[0].nested' (default: root)"}
                },
                "required": ["path"]
            })),
        }),
        "yaml_query" => Some(McpTool {
            name: name.to_string(),
            description: Some("Read a YAML file and query it using a simple path syntax like 'obj.key[0].nested'".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the YAML file"},
                    "query": {"type": "string", "description": "Query path like 'obj.key[0].nested' (default: root)"}
                },
                "required": ["path"]
            })),
        }),
        // ── Template rendering tool (P1) ────────────────────────
        "template_render" => Some(McpTool {
            name: name.to_string(),
            description: Some("Render a template with {{variable}} replacement, {{#each}} loops, and {{#if}} conditionals".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "template": {"type": "string", "description": "Template string with {{variable}} placeholders"},
                    "variables": {"type": "object", "description": "Key-value pairs for template variables"},
                    "output_path": {"type": "string", "description": "Optional file path to write the rendered output"}
                },
                "required": ["template"]
            })),
        }),
        // ── Code metrics tool (P2) ────────────────────────────
        "code_metrics" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Analyze source code files and compute code quality metrics. ".to_string()
                    + "Returns lines of code, cyclomatic complexity estimate, function/class counts, "
                    + "and estimated function sizes.",
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "directory": {"type": "string", "description": "Directory to scan (default: current)"},
                    "pattern": {"type": "string", "description": "Glob pattern to match files (default: **/*.rs)"}
                },
                "required": []
            })),
        }),
        // ── Security scan tool (P2) ────────────────────────────
        "security_scan" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Scan project dependencies for known vulnerabilities. ".to_string()
                    + "Supports Cargo.lock, package-lock.json, requirements.txt, go.sum, "
                    + "and other lock files. Queries the OSV API for CVE information. "
                    + "Results are cached locally for 24 hours by default.",
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "directory": {"type": "string", "description": "Project directory to scan (default: current)"},
                    "cache_ttl_hours": {"type": "integer", "description": "Cache TTL in hours for OSV query results (default: 24)", "default": 24}
                },
                "required": []
            })),
        }),
        // ── Docker container tools (P2) ────────────────────────
        "docker_ps" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "List Docker containers. Wraps `docker ps`. ".to_string()
                    + "Returns container IDs, names, images, status, and port mappings.",
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "all": {"type": "boolean", "description": "List all containers (including stopped)", "default": false},
                    "format": {"type": "string", "enum": ["json", "text"], "description": "Output format", "default": "json"}
                },
                "required": []
            })),
        }),
        "docker_exec" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Execute a command inside a running Docker container. ".to_string()
                    + "Wraps `docker exec`. Returns stdout, stderr, and exit code.",
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "container": {"type": "string", "description": "Container name or ID"},
                    "command": {"type": "string", "description": "Command to execute"},
                    "interactive": {"type": "boolean", "description": "Run interactively (-i)", "default": false},
                    "workdir": {"type": "string", "description": "Working directory inside container"}
                },
                "required": ["container", "command"]
            })),
        }),
        "docker_logs" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "View logs from a Docker container. ".to_string()
                    + "Wraps `docker logs`. Supports tail, since, and timestamps.",
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "container": {"type": "string", "description": "Container name or ID"},
                    "tail": {"type": "string", "description": "Number of lines to show from the end (default: 50)", "default": "50"},
                    "since": {"type": "string", "description": "Show logs since timestamp (e.g. 2024-01-01T00:00:00)"},
                    "timestamps": {"type": "boolean", "description": "Show timestamps", "default": false}
                },
                "required": ["container"]
            })),
        }),
        // ── Docker build, push, and compose tools (P2) ────────────
        "docker_build" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Build a Docker image from a Dockerfile. ".to_string()
                    + "Supports build args, tags, and docker compose build.",
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Build context directory (default: .)"},
                    "tag": {"type": "string", "description": "Image tag (e.g. 'myapp:latest')", "default": "latest"},
                    "dockerfile": {"type": "string", "description": "Path to Dockerfile (default: Dockerfile)"},
                    "build_args": {"type": "object", "description": "Build-time variables as key-value pairs"},
                    "no_cache": {"type": "boolean", "description": "Build without cache (default: false)"},
                },
                "required": []
            })),
        }),
        "docker_push" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Push a Docker image to a registry. ".to_string()
                    + "Wraps `docker push`.",
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "image": {"type": "string", "description": "Image name with tag (e.g. 'myapp:latest')"},
                    "registry": {"type": "string", "description": "Registry URL (e.g. 'docker.io/user')"},
                },
                "required": ["image"]
            })),
        }),
        "docker_compose" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Run docker-compose commands (up, down, build, logs, ps). ".to_string()
                    + "Wraps `docker compose`.",
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "subcommand": {
                        "type": "string",
                        "enum": ["up", "down", "build", "logs", "ps", "restart", "stop", "start"],
                        "description": "Docker compose subcommand"
                    },
                    "file": {"type": "string", "description": "Path to compose file (default: docker-compose.yml)"},
                    "service": {"type": "string", "description": "Target service name (optional)"},
                    "detach": {"type": "boolean", "description": "Run containers in background (default: true for up)"},
                    "build": {"type": "boolean", "description": "Build images before starting (for up)"},
                    "tail": {"type": "string", "description": "Number of log lines to show (for logs)"},
                },
                "required": ["subcommand"]
            })),
        }),
        // ── File watch tool (P2) ────────────────────────────────
        "file_watch" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Watch files or directories for changes. ".to_string()
                    + "On first call records a baseline of file modification times. "
                    + "Subsequent calls return files that have been added, modified, or removed "
                    + "since the last check.",
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "directory": {"type": "string", "description": "Directory to watch (default: current)"},
                    "pattern": {"type": "string", "description": "Glob pattern for files to track (default: **/*)"},
                    "session": {"type": "string", "description": "Watch session identifier for independent tracking (default: default)"},
                    "reset": {"type": "boolean", "description": "Reset the baseline and start fresh", "default": false}
                },
                "required": []
            })),
        }),
        "tool_search" => Some(McpTool {
            name: name.to_string(),
            description: Some("Search for available tools by name or description. Use this to discover niche or specialized tools that are not shown in the default tool list.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query to find relevant tools"},
                    "top_k": {"type": "integer", "description": "Maximum number of results to return (default: 8, max: 20)", "default": 8}
                },
                "required": ["query"]
            })),
        }),
        _ => None,
    }
}
