# VS Code Addon

The VS Code addon is the richest editor integration in this repository. It exposes runtime-backed commands, can probe runtime health, and can override the backend transport mode from extension settings.

## What the addon expects

The extension needs:

- a reachable `go-on` executable
- a valid `config.toml`
- a runtime mode that matches the selected workflow

The addon manifest currently exposes these protocol override values:

- `from_config`
- `adaptive`
- `acp_stdio`
- `acp_http`
- `mcp_stdio`
- `mcp_http`

`from_config` keeps the extension aligned with the backend config. The others force an explicit override.

## Recommended first-time setup

1. Build the backend or use an existing binary.
2. Run `go-on --setup --setup-level standard`.
3. In VS Code, configure the executable path and config path if auto-discovery is insufficient.
4. Keep `protocolMode` on `from_config` unless you are debugging a specific transport problem.

## When to use each protocol mode

- `from_config`: default choice for normal daily use.
- `adaptive`: preferred override when you want one runtime that can satisfy mixed probes.
- `acp_stdio`: use when the extension should drive a spawned stdio runtime contract.
- `acp_http`: use when the backend is already running as a shared local HTTP service.
- `mcp_stdio`: only when your extension workflow explicitly requires MCP over stdio.
- `mcp_http`: use when you want explicit `/v1` HTTP semantics.

## Runtime health expectations

The extension contract expects the runtime health path at:

```text
/health
```

The OpenAI-compatible probe path is:

```text
/v1/models
```

The addon also knows about:

- `/v1/model`
- `/v1/chat/completions`
- `/v1/responses`

## Practical workspace settings pattern

Example workspace-level settings:

```json
{
  "go-on.runtime.protocolMode": "from_config",
  "go-on.runtime.executablePath": "D:/Workspace/RustWorkspace/go-on/target/debug/go-on.exe",
  "go-on.runtime.configPath": "D:/Workspace/RustWorkspace/go-on/config.toml"
}
```

When you want to force a shared HTTP runtime:

```json
{
  "go-on.runtime.protocolMode": "acp_http"
}
```

## Operational workflow

For the addon, a good debugging sequence is:

1. `go-on --validate-config`
2. `go-on --status`
3. confirm the executable path in VS Code settings
4. confirm the config path in VS Code settings
5. only then override the protocol mode if required

## When to use HTTP instead of stdio

Prefer HTTP when:

- you want the GUI and VS Code to target the same backend instance
- you want manual probing through `/health` and `/v1/models`
- you want long-running local service behavior

Prefer stdio when:

- you want VS Code to own process startup and shutdown
- you want each workspace session isolated from the others

## Common failure patterns

- If the addon can spawn the executable but reports provider not ready, the issue is configuration or credentials, not transport.
- If HTTP mode is selected and `/health` is unreachable, the backend was not started with `--acp-http-bind`.
- If forcing `mcp_http`, make sure the consuming feature really expects the `/v1` surface rather than ACP semantics.