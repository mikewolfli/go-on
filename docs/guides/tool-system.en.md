# Tool System

## Overview

Go-On provides a unified tool system built around `ToolRegistry`, `Tool` trait, and `ToolPipeline`. Tools are the primary mechanism for AI agents to interact with the filesystem, execute commands, search code, and perform other operations.

## Architecture

```text
ToolRegistry (global singleton)
  ├── Tool trait (read_file, write_file, grep, etc.)
  ├── ToolCapabilityProfile (risk, timeout, fallback)
  └── Aliases (semantic_search → code_index_search)
         │
         ▼
  ToolPipeline (sequential composition)
         │
         ▼
  SandboxPolicy (governance gate)
```

## Built-in Tools

### Core Tools (always available)

| Tool | Type | Description |
|------|------|-------------|
| `read_file` | Read | Read file contents |
| `write_file` | Write | Create or overwrite files |
| `search_files` | Search | Find files by pattern |
| `apply_patch` | Write | Apply diffs to files |
| `run_tests` | Shell | Execute test commands |
| `inspect_git_diff` | Read | Show git diff |

### Extended Tools

#### Search & Discovery

| Tool | Description |
|------|-------------|
| `grep` | Search file contents with regex |
| `find_files` | Find files by name pattern |
| `code_index_search` | Semantic code symbol search |
| `diagnostics` | Get project diagnostics |

#### File Operations

| Tool | Description |
|------|-------------|
| `list_directory` | List directory contents |
| `file_move` | Move/rename files |
| `file_delete` | Delete files (requires confirmation) |
| `archive_inspect` | Inspect archive contents |
| `archive_extract` | Extract archives |
| `compress`/`decompress` | File compression |

#### Shell & Execution

| Tool | Description |
|------|-------------|
| `shell_exec` | Execute shell commands |
| `cargo_check` | Run cargo check |
| `cargo_test` | Run cargo test |
| `git` | Execute git commands |

#### Network

| Tool | Description |
|------|-------------|
| `http_request` | Make HTTP requests |
| `dns_lookup` | DNS resolution |
| `ping` | Network ping |
| `port_scan` | Port scanning |

#### Skill Management

| Tool | Description |
|------|-------------|
| `skill_list` | List registered skills |
| `skill_execute` | Execute a skill by name |
| `skill_create` | Create a prompt-based skill |
| `skill_reload` | Reload skills from disk |

## Creating a Custom Tool

1. Implement the `Tool` trait:
```rust
pub struct MyTool;

impl Tool for MyTool {
    fn name(&self) -> &'static str { "my_tool" }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        // Implementation
    }
}
```

2. Register in `ToolRegistry::new()`:
```rust
registry.register_with_profile(
    MyTool,
    ToolCapabilityProfile {
        capability: "my_capability".to_string(),
        risk_level: ToolRiskLevel::Low,
        timeout_budget_ms: 30_000,
        retry_policy: RetryPolicy { max_retries: 1, retry_on_failure: true },
        fallback_chain: vec!["alternative_tool".to_string()],
    },
);
```

3. Add to pipeline tool-to-action mapping in `pipeline.rs`.

## Sandbox Integration

Each tool is classified by action type (read, search, write, shell, network) and checked against the active `SandboxLevel`. See [Governance](../guides/governance.en.md) for details.
