# go-on as Zed Agent Server

This guide explains how to configure [go-on](https://github.com/mark3labs/go-on) as a
[Zed Agent Server](https://zed.dev/docs/assistant/agent-servers), enabling Zed's AI panel to
communicate with go-on via the Agent Communication Protocol (ACP).

---

## Quick Start

1. Open Zed's settings: `Ctrl+Shift+P` → **"Zed: Open Settings"** (or open your `settings.json`
   directly from `~/.config/zed/settings.json`).

2. Add the following entry to the `agent_servers` object:

```json
{
  "agent_servers": {
    "go-on": {
      "command": {
        "command": "go-on",
        "args": ["--protocol-mode", "acp_stdio"],
        "env": {
          "GO_ON_CONFIG": "/path/to/your/config.toml"
        }
      }
    }
  }
}
```

3. Open the AI panel (`Ctrl+Enter`), select **"go-on"** from the agent dropdown, and start
   chatting.

> **Note:** The `go-on` binary must be on your `PATH`. If it is not, replace the `"command"`
> value with an absolute path such as `"/usr/local/bin/go-on"`.

---

## How It Works

When go-on is configured as a Zed Agent Server, Zed spawns it as a subprocess and communicates
over **ACP over stdio**. The lifecycle is:

1. **Session creation** — Zed sends `session/new` to begin a conversation.
2. **Tool discovery** — Zed calls `tools/list` to learn what tools go-on exposes (60+ tools).
3. **Prompt execution** — Zed sends `session/prompt` with the user's message; go-on streams
   responses back, including `<thinking>` blocks rendered as collapsible sections in the UI.
4. **Tool calls** — When go-on decides to invoke a tool, Zed executes `tools/call` and routes
   results back into the conversation.
5. **Session close** — Zed sends `session/close` when the conversation ends.

Zed reuses the same subprocess for sequential conversations; only the `session/close` → `new`
cycle triggers a full restart.

---

## Features

| Feature | Details |
|---|---|
| **Session lifecycle** | `session/new` → `session/prompt` → `session/close` |
| **Tool discovery** | `tools/list` — lists all 60+ go-on tools |
| **Tool execution** | `tools/call` — execute any go-on tool |
| **Streaming chat** | `session/prompt` with streaming responses |
| **Thinking / reasoning** | `<thinking>` blocks rendered as collapsible sections |
| **Subprocess reuse** | Zed keeps the process alive across consecutive sessions |

---

## Full Configuration

```json
{
  "agent_servers": {
    "go-on": {
      "command": {
        "command": "/path/to/go-on",
        "args": [
          "--protocol-mode",
          "acp_stdio",
          "-b",
          "127.0.0.1:8090"
        ],
        "env": {
          "OPENAI_API_KEY": "sk-...",
          "ANTHROPIC_API_KEY": "sk-ant-...",
          "DEEPSEEK_API_KEY": "sk-...",
          "GO_ON_CONFIG": "/home/user/.goon/config.toml"
        }
      }
    }
  }
}
```

---

## Configuration Options

### Command Arguments

| Argument | Description |
|---|---|
| `--protocol-mode acp_stdio` | ACP over stdio **(required for Zed)** |
| `-b 127.0.0.1:8090` | Optional HTTP bind address for health checks / metrics |
| `--verbose` | Enable verbose logging to stderr (useful for debugging) |
| `--config <path>` | Path to go-on's `config.toml` (alternative to `GO_ON_CONFIG` env var) |

### Environment Variables

| Variable | Description |
|---|---|
| `OPENAI_API_KEY` | API key for OpenAI models |
| `ANTHROPIC_API_KEY` | API key for Anthropic models |
| `DEEPSEEK_API_KEY` | API key for DeepSeek models |
| `GO_ON_CONFIG` | Path to the go-on configuration file |
| `GOON_LOG` | Log level (e.g., `debug`, `info`, `warn`, `error`) |

Only set the environment variables you actually need for the models you intend to use.

---

## Agent Panel Modes

Zed's agent panel supports several interaction modes when using go-on:

| Mode | Description |
|---|---|
| **Ask** | General Q&A — ask anything and get answers |
| **Plan** | Structured task breakdown before execution |
| **Edit** | Code changes with review before applying |
| **SafeGuard** | Safety-first mode with risk assessment |
| **FullAuto** | Fully autonomous execution (use with caution) |

These modes affect how go-on structures its responses and when it requests user confirmation
before invoking tools.

---

## Running the HTTP Server Alongside the Agent Server

go-on can serve two roles simultaneously:

- **ACP stdio** (required by Zed) — used for agent communication.
- **HTTP server** (optional) — serves the go-on web GUI and exposes health/metrics endpoints.

When you pass `-b 127.0.0.1:8090`, go-on starts an HTTP server on that address while still
communicating with Zed over stdio. This is useful for monitoring or debugging.

```json
{
  "command": "go-on",
  "args": ["--protocol-mode", "acp_stdio", "-b", "127.0.0.1:8090"]
}
```

You can then visit `http://127.0.0.1:8090` in your browser to access the go-on web GUI.

---

## Verification

### 1. Check that the agent server starts correctly

After adding the configuration to Zed's `settings.json`, look for the go-on entry in the agent
dropdown inside the AI panel (`Ctrl+Enter`). If it appears, the server was registered
successfully.

### 2. Send a test prompt

Select **"go-on"** from the dropdown, type a question, and press Enter. You should see a
streaming response.

### 3. Test the ACP protocol directly (without Zed)

You can verify that go-on responds to ACP stdio messages by running it from your terminal:

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' | go-on --protocol-mode acp_stdio
```

A successful response looks like:

```json
{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"...","capabilities":{}}}
```

### 4. Check the HTTP server (optional)

If you configured `-b`, visit `http://127.0.0.1:8090/health` in your browser or use `curl`:

```bash
curl http://127.0.0.1:8090/health
```

---

## Troubleshooting

| Problem | Likely Cause | Fix |
|---|---|---|
| Agent not appearing in dropdown | `go-on` not on `PATH` | Use an absolute path for `"command"` |
| Connection refused | Wrong `--protocol-mode` | Make sure `--protocol-mode acp_stdio` is set |
| Empty response / timeout | Missing API keys | Set `OPENAI_API_KEY` or `ANTHROPIC_API_KEY` in `env` |
| `config.toml` not found | Wrong path | Set `GO_ON_CONFIG` or pass `--config` |
| Verbose errors on stderr | Need debugging | Add `"--verbose"` to `args` |

### Enabling debug logging

Add `"--verbose"` to the `args` array and set `"GOON_LOG": "debug"` in `env`. Then check Zed's
agent server logs via **"Zed: Open Log"** → select the agent server log file.

---

## See Also

- [Zed Agent Servers Documentation](https://zed.dev/docs/assistant/agent-servers)
- [go-on Protocol Guide](./protocol-guide.md)
- [go-on Configuration Guide](./workflow-config.md)
