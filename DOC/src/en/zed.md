# Zed Integration

Zed can attach to `go-on` in two practical ways:

- ACP over stdio, where Zed launches the backend as a child process
- HTTP-based integration, where `go-on` runs as a long-lived local service and Zed points to its HTTP surface

Because Zed configuration shape changes across releases, treat the snippets below as transport-focused templates. Keep the transport, command line, and endpoint values the same even if a later Zed version renames a setting key.

## Mode 1: ACP over stdio

Use this when you want Zed to start `go-on` itself.

Recommended backend command:

```json
{
  "agent_servers": {
    "go-on-acp": {
      "type": "custom",
      "command": "go-on",
      "args": [
        "--config",
        "/absolute/path/to/config.toml",
        "--protocol-mode",
        "acp_stdio",
        "--verbose"
      ]
    }
  }
}
```

Windows example:

```json
{
  "agent_servers": {
    "go-on-acp": {
      "type": "custom",
      "command": "D:/Workspace/RustWorkspace/go-on/target/debug/go-on.exe",
      "args": [
        "--config",
        "D:/Workspace/RustWorkspace/go-on/config.toml",
        "--protocol-mode",
        "acp_stdio"
      ]
    }
  }
}
```

Use ACP stdio when:

- you want one Zed window to own the process lifecycle
- you do not need other clients to share the same runtime
- you want the simplest local transport with no listening socket

## Mode 2: ACP over HTTP

Use this when you want one long-running shared backend process.

Start the backend first:

```bash
go-on --config config.toml --protocol-mode acp_http --acp-http-bind 127.0.0.1:8090
```

If you want one runtime that can also satisfy MCP-style clients, use `adaptive` instead:

```bash
go-on --config config.toml --protocol-mode adaptive --acp-http-bind 127.0.0.1:8090
```

Then point Zed's external-agent style integration at the runtime base URL:

```json
{
  "agent_servers": {
    "go-on-http": {
      "type": "custom",
      "url": "http://127.0.0.1:8090"
    }
  }
}
```

The ACP HTTP health probe is:

```text
http://127.0.0.1:8090/health
```

## Mode 3: MCP or model-provider style HTTP through `/v1`

The backend also exposes OpenAI-compatible endpoints:

- `/v1/models`
- `/v1/model`
- `/v1/chat/completions`
- `/v1/responses`

If your Zed version expects an OpenAI-compatible provider or an MCP-style LLM provider, use a `/v1` base URL.

Example (latest Zed `openai_compatible` shape):

```json
{
  "language_models": {
    "openai_compatible": {
      "go-on-local": {
        "api_url": "http://127.0.0.1:8090/v1",
        "available_models": [
          {
            "name": "gpt-5.5",
            "display_name": "go-on auto (gpt-5.5)",
            "max_tokens": 400000,
            "capabilities": {
              "chat_completions": true,
              "tools": true,
              "images": false,
              "parallel_tool_calls": false,
              "prompt_cache_key": false
            }
          }
        ]
      }
    }
  }
}
```

If a model is Responses-only, set `capabilities.chat_completions=false` for that model entry.

Use this path when:

- Zed expects an OpenAI-compatible endpoint rather than ACP transport
- you want the same backend to serve editor chat and model-provider probes

## Which mode to choose

- Choose `acp_stdio` if Zed should spawn the runtime.
- Choose `acp_http` if Zed should connect to a shared ACP server.
- Choose `adaptive` with `--acp-http-bind` if multiple front ends may attach at the same time.
- Choose the `/v1` model-provider path if the relevant Zed feature is provider-oriented rather than ACP-oriented.

## Chat Modes (Ask / Plan / Edit / Safeguard / Full Auto)

The Go-On backend supports multiple chat modes that control orchestration behavior,
tool access, and approval policies. When Zed sends an ACP `chat.request` via the
agent server, the mode is passed as part of the request parameters.

### How it works

ACP `chat.request` has a `mode` field in its params. If Zed does not send a mode
(the typical case), the backend automatically defaults to **"ask"** (the safest
general-purpose mode, equivalent to a Q&A assistant).

### Mode behavior

| Mode | Description | Tool Access | Approval Required |
|------|-------------|-------------|-------------------|
| `ask` | Q&A assistant — general questions | Limited | No |
| `plan` | Planning mode — structured task breakdown | Full | No |
| `edit` | Edit/review mode — code changes | Full | No |
| `safeguard` | Safety-first — escalation on high-risk operations | Limited | Yes |
| `full_auto` | Fully autonomous — agent runs without user confirmation | Full | No (escalation only) |

### Passing mode from Zed

Most Zed versions send ACP requests without a `mode` parameter. The backend
handles this transparently by defaulting to `"ask"`. If you want to explicitly
control the mode, you can:

1. **Multiple agent entries** — Define several ACP agents in Zed settings,
one per mode, each hardcoding the mode via backend routing:

```json
{
  "agent_servers": {
    "go-on-ask": {
      "type": "custom",
      "command": "go-on",
      "args": ["--config", "./config.toml", "--protocol-mode", "acp_stdio"]
    },
    "go-on-full-auto": {
      "type": "custom",
      "command": "go-on",
      "args": ["--config", "./config.toml", "--protocol-mode", "acp_stdio"]
    }
  }
}
```

2. **Single agent, default mode** — Use one agent. The backend defaults to
`"ask"`. Mode-switching is done via the Go-On GUI (separate from Zed).

3. **OpenAI-compatible `/v1` endpoint** — If using `openai_compatible` in Zed,
the mode is not passed either; the backend again defaults to `"ask"`.

### Display in Zed

Zed's own UI does not have a mode selector. The mode is an internal parameter
of the ACP protocol. To see which mode is currently active, check:

```bash
go-on --status
```

The backend logs the effective mode on each request:
```
INFO: mode not specified by client, defaulting to 'ask'
```

### Backend default mode fallback

As of the latest build, the `ChatParams.mode` field uses `#[serde(default)]`
(empty string when absent), and the backend auto-defaults to `"ask"`.
This ensures compatibility with any ACP client that does not send a mode.

## Operational checks

Before blaming Zed configuration, verify the backend:

```bash
go-on --config config.toml --status
go-on --config config.toml --validate-config
```

For HTTP mode, verify endpoints manually:

```text
GET http://127.0.0.1:8090/health
GET http://127.0.0.1:8090/v1/models
```

## Zed external-agent updates

- As of Zed `v0.221.x+`, ACP Registry is the preferred installation path for external agents.
- Built-in external agent names in Zed commonly include `claude-acp`, `codex-acp`, and `gemini`.
- Use `zed: acp registry` and `dev: open acp logs` for installation and runtime diagnosis.

## Cross-platform paths

- Linux settings file: `~/.config/zed/settings.json`
- macOS settings file: `~/Library/Application Support/Zed/settings.json`
- Windows settings file: `%APPDATA%/Zed/settings.json`

## Model freshness policy

- Keep `available_models` aligned with latest provider docs before changing model IDs.
- For OpenAI-family models, verify context window and endpoint compatibility from current provider docs.
- For Zed-hosted model names, re-check retirements/replacements in Zed `AI > Models` docs before rollout.

## Failure patterns

- If `/health` works but Zed still rejects ACP, the configured runtime mode may be MCP-only.
- If `/v1/models` works but model chat fails, check provider readiness in `go-on --status`.
- If stdio mode fails immediately, verify the executable path and `config.toml` path first.