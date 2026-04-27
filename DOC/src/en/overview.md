# Architecture Overview

`go-on` is a three-surface runtime around a Rust backend:

- Backend: the executable owns config loading, provider selection, routing, setup, health checks, protocol negotiation, and HTTP or stdio transport.
- GUI: the Tauri desktop console manages backend discovery, process lifecycle, integration probes, and local operator workflows.
- VS Code addon: the extension launches or probes the runtime, exposes RPC-backed commands, and can override protocol mode per workspace.

## Current runtime model

The backend supports five access modes:

- `adaptive`: keep dual-stack capability and route requests by client type while deriving startup transport from runtime prerequisites.
- `acp_stdio`: run ACP over stdio for editor-launched child-process integrations.
- `acp_http`: expose ACP-style HTTP endpoints from a long-running backend process.
- `mcp_stdio`: expose MCP over stdio.
- `mcp_http`: expose MCP and OpenAI-compatible HTTP endpoints.

In this model, explicit fixed modes are still config-driven. `adaptive` is not a silent rewrite to one fixed interface; today it selects an HTTP entry when `--acp-http-bind` is present and otherwise keeps a stdio entry while preserving ACP/MCP request dispatch compatibility.

The HTTP runtime exposes a practical integration surface around `http://127.0.0.1:8090` by default when started with `--acp-http-bind`:

- `/health`
- `/chat`
- `/chat/stream`
- `/v1/models`
- `/v1/model`
- `/v1/chat/completions`
- `/v1/responses`

That split matters for the three clients:

- Zed external agent flows can use ACP over stdio or ACP over HTTP.
- Zed model-provider style flows can use the OpenAI-compatible `/v1` endpoints.
- The VS Code addon can either use runtime RPC over spawned stdio or probe the runtime through HTTP.
- The GUI uses a local backend executable plus a working directory that contains `config.toml`.

## Repository areas that map to the architecture

- `src/`: backend runtime, CLI, setup, ACP and MCP implementation.
- `GUI/`: Tauri desktop console.
- `vscode-addon/`: VS Code extension.
- `config.toml` and `config.toml.autopilot-adaptive`: runtime configuration baseline.
- `tests/requests/`: request and benchmark fixtures.
- `scripts/`: support scripts.

## Recommended operator flow

For a new machine or a new workspace, the shortest path is:

1. Build or obtain the `go-on` backend executable.
2. Run `go-on --setup --setup-level standard`.
3. Verify readiness with `go-on --status`.
4. If an HTTP client is involved, start the backend with `--protocol-mode adaptive --acp-http-bind 127.0.0.1:8090`.
5. Attach Zed, the VS Code addon, or the GUI depending on your front end.

The next chapters expand each part in detail.