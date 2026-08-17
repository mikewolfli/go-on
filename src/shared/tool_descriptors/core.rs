//! Descriptors for the original core built-in tools.

use crate::mcp::McpTool;
use serde_json::json;

/// Returns the MCP tool descriptor for a known core tool name, or `None`.
pub(super) fn descriptor(name: &'static str) -> Option<McpTool> {
    match name {
        "read_file" => Some(McpTool {
            name: name.to_string(),
            description: Some("Read contents of a file".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path to read"}
                },
                "required": ["path"]
            })),
        }),
        "write_file" => Some(McpTool {
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
        }),
        "search_files" => Some(McpTool {
            name: name.to_string(),
            description: Some("Find files and directories matching a glob pattern".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Search pattern/glob"},
                    "directory": {"type": "string", "description": "Search directory"},
                    "max_results": {"type": "integer", "description": "Maximum number of file paths to return (default 1000)", "default": 1000}
                },
                "required": ["pattern"]
            })),
        }),

        "apply_patch" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Apply a unified diff patch to a file or directory via `git apply` (piped via stdin).".to_string(),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "patch": {"type": "string", "description": "The unified diff/patch content to apply"},
                    "check": {"type": "boolean", "description": "If true, only validate the patch with `git apply --check` without applying it"},
                    "directory": {"type": "string", "description": "Working directory to apply the patch in (default: current directory)"}
                },
                "required": ["patch"]
            })),
        }),
        "run_tests" => Some(McpTool {
            name: name.to_string(),
            description: Some("Run tests for a project using an allowlisted command (cargo, npm, yarn, pnpm, make, go, python, pytest, mvn, gradle, git).".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "directory": {"type": "string", "description": "Project directory to run the tests in"},
                    "command": {"type": "string", "enum": ["cargo", "npm", "yarn", "pnpm", "make", "go", "python", "pytest", "mvn", "gradle", "git"], "description": "Test runner command (default: cargo)"},
                    "args": {"type": "array", "items": {"type": "string"}, "description": "Arguments passed to the command (default: [\"test\"])"}
                },
                "required": ["directory"]
            })),
        }),
        "inspect_git_diff" => Some(McpTool {
            name: name.to_string(),
            description: Some("Inspect the current git diff for a project.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "directory": {"type": "string", "description": "Git repository directory"},
                    "staged": {"type": "boolean", "description": "If true, show staged diff (--cached); otherwise show unstaged"},
                    "files": {"type": "array", "items": {"type": "string"}, "description": "Optional list of file paths to filter the diff"}
                },
                "required": ["directory"]
            })),
        }),
        "workflow_execute" => Some(McpTool {
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
        }),
        "workflow_ask" => Some(McpTool {
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
        }),
        "workflow_generate" => Some(McpTool {
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
        }),
        "skill-creator" => Some(McpTool {
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
        }),
        "github_search_skills" => Some(McpTool {
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
        }),
        "import_skill" => Some(McpTool {
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
        }),
        _ => None,
    }
}
