# Go-On VS Code Extension

VS Code extension for operating and interacting with go-on runtime.

## Version

- Extension: 1.1.0
- Target runtime: go-on 1.1.0

## What Is Implemented (Current)

- Runtime process start/stop and status monitoring
- Chat panel and chat command entry
- Settings panel and process status panel
- Workflow and process-flow webviews (local persisted definitions)
- Advanced edit/refactor commands (chat-driven generation)
- Runtime download and path configuration support

## Command To Backend Mapping

The following commands are wired to concrete backend RPC methods:

| VS Code command | Runtime RPC method |
|---|---|
| `go-on.sendRequest` | `chat` |
| `go-on.healthCheck` | `runtime.health` |
| `go-on.breakerStatus` | `breaker.status` |
| `go-on.cacheClear` | `cache.clear` |
| `go-on.vectorClear` | `vector.clear` |
| `go-on.configReload` | `config.reload` |
| `go-on.shutdown` | `shutdown` |
| `go-on.editCode` | `chat` |
| `go-on.refactorCode` | `chat` |
| `go-on.workflowExecute` | `workflow.execute` |
| `go-on.taskPlan` | `task.plan` |
| `go-on.taskExecute` | `task.execute` |
| `go-on.learningSummary` | `learning.summary` |
| `go-on.autotuneStatus` | `autotune.status` |

The following are extension/UI behaviors (not direct dedicated runtime RPC endpoints):

- `go-on.openChat`, `go-on.closeChat`, `go-on.openSettings`
- `go-on.newSession`, `go-on.switchSession`, `go-on.clearChat`, `go-on.exportChat`
- `go-on.createWorkflow`, `go-on.runWorkflow`, `go-on.showProcessFlow`

Notes about workflow/process views:

- Definitions are stored in extension global state.
- Chat-type steps call runtime `chat`.
- Non-chat steps are handled in extension-side logic.

## Available Commands (Contributed)

- `go-on.start`
- `go-on.stop`
- `go-on.sendRequest`
- `go-on.healthCheck`
- `go-on.breakerStatus`
- `go-on.cacheClear`
- `go-on.vectorClear`
- `go-on.configReload`
- `go-on.shutdown`
- `go-on.openChat`
- `go-on.closeChat`
- `go-on.openSettings`
- `go-on.clearChat`
- `go-on.exportChat`
- `go-on.newSession`
- `go-on.switchSession`
- `go-on.createWorkflow`
- `go-on.runWorkflow`
- `go-on.editCode`
- `go-on.refactorCode`
- `go-on.showProcessFlow`
- `go-on.workflowExecute`
- `go-on.taskPlan`
- `go-on.taskExecute`
- `go-on.learningSummary`
- `go-on.autotuneStatus`

## Settings

Key settings include:

- `go-on.configPath`
- `go-on.executablePath` (recommended: set to a local absolute path)
- `go-on.autoDownloadBinary` (default: false)
- `go-on.releaseRepository`
- `go-on.releaseTag`
- `go-on.autoStart`
- `go-on.runtime.protocolMode` (`from_config` / `adaptive` / `acp_stdio` / `acp_http` / `mcp_stdio` / `mcp_http`)
- `go-on.chat.*`
- `go-on.cache.enabled`
- `go-on.vector.enabled`
- `go-on.health.interval`
- `go-on.ui.*`
- `go-on.advancedAI.*`

When `go-on.runtime.protocolMode` is not `from_config`, the extension starts runtime with `--protocol-mode <value>` so protocol switching is applied on the main startup chain.

Error Trace Alignment (BLUE14 AGENT4)

- Runtime JSON-RPC errors are surfaced as `rpc_error:<code>:<kind>:<message> (context=<...>)`.
- `kind` is aligned with backend governance classes: `PuaViolation`, `BudgetExceeded`, `SandboxBlocked`.
- When available, context includes the backend dispatch prefix `acp.handle_request.dispatch` for fast diagnosis.

## Development

```bash
cd vscode-addon
npm install
npm run compile
```

## Quick Start

1. Install the extension — it activates automatically on VS Code startup (`onStartupFinished`).
2. Open the Go-On Chat view from the activity bar or command palette (`go-on.openChat`).
3. On first use, the extension will:
   - Auto-create a config file at `~/.config/go-on/config.toml`
   - Prompt to download the go-on runtime or select a local binary
   - Guide you through provider API key setup
4. Set `go-on.executablePath` and `go-on.configPath` in settings if you have a custom runtime.
5. `go-on.runtime.protocolMode` supports: `from_config` / `adaptive` / `acp_stdio` / `acp_http` / `mcp_stdio` / `mcp_http`.
6. Use `go-on.start` to start the runtime or let the chat view start it automatically.


## Sync Policy

Extension docs must stay aligned with actual implemented commands and runtime RPC wiring in `vscode-addon/src`.
