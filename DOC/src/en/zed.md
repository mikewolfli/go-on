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

Example:

```json
{
  "language_models": {
    "go-on-local": {
      "provider": "openai_compatible",
      "api_url": "http://127.0.0.1:8090/v1",
      "model": "auto"
    }
  }
}
```

Use this path when:

- Zed expects an OpenAI-compatible endpoint rather than ACP transport
- you want the same backend to serve editor chat and model-provider probes

## Which mode to choose

- Choose `acp_stdio` if Zed should spawn the runtime.
- Choose `acp_http` if Zed should connect to a shared ACP server.
- Choose `adaptive` with `--acp-http-bind` if multiple front ends may attach at the same time.
- Choose the `/v1` model-provider path if the relevant Zed feature is provider-oriented rather than ACP-oriented.

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

## Failure patterns

- If `/health` works but Zed still rejects ACP, the configured runtime mode may be MCP-only.
- If `/v1/models` works but model chat fails, check provider readiness in `go-on --status`.
- If stdio mode fails immediately, verify the executable path and `config.toml` path first.