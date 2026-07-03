---
name: code-execution-sandbox
description: Safely executes code snippets in a sandboxed environment with timeout, resource limits, and output capture
version: 1.0.0
author: go-on-team
tags: [code, execution, sandbox, security, testing, isolation]
min_go_on_version: 1.0.0
---

# Code Execution Sandbox Skill

Safely executes code snippets in an isolated sandbox environment with configurable timeouts, resource limits, and output capture. Supports multiple programming languages and provides structured execution results including stdout, stderr, exit codes, and execution timing.

## How It Works

1. **Isolation** — Each code execution runs in a temporary sandbox directory with restricted filesystem access
2. **Timeout enforcement** — Hard timeout kills runaway processes after the configured duration
3. **Resource limits** — Optional memory and CPU limits to prevent resource exhaustion
4. **Output capture** — Captures stdout, stderr, and exit codes as structured results
5. **Language detection** — Auto-detects language from code content or accepts explicit language parameter

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `code` | string | The source code to execute |
| `language` | string | Programming language (python, javascript, rust, go, bash, ruby). Auto-detected if omitted |
| `timeout_secs` | integer | Maximum execution time in seconds (default: 30, max: 300) |
| `max_memory_mb` | integer | Optional memory limit in megabytes |
| `stdin` | string | Optional standard input to pass to the process |
| `arguments` | array | Optional command-line arguments for the executed program |

## Example

```json
{
  "code": "print('Hello from sandbox!')\nfor i in range(5):\n    print(f'Count: {i}')",
  "language": "python",
  "timeout_secs": 10,
  "stdin": ""
}
```

**Example output:**

```json
{
  "success": true,
  "stdout": "Hello from sandbox!\nCount: 0\nCount: 1\nCount: 2\nCount: 3\nCount: 4\n",
  "stderr": "",
  "exit_code": 0,
  "execution_time_ms": 45,
  "language": "python",
  "timed_out": false
}
```
