# go-on Multi-Editor Quick Config

Use go-on as an AI agent in any editor. Copy the config snippet for your editor below.

## VS Code

```json
{
  "mcp": {
    "servers": {
      "go-on": {
        "command": "go-on",
        "args": ["--protocol-mode", "mcp_stdio"],
        "env": {
          "OPENAI_API_KEY": "sk-...",
          "GO_ON_CONFIG": "/home/user/.goon/config.toml"
        }
      }
    }
  }
}
```

> Requires VS Code with MCP-supporting extension (Continue.dev, Copilot Chat, or Cline).

## Cursor

File: `~/.cursor/mcp.json`

```json
{
  "mcpServers": {
    "go-on": {
      "command": "go-on",
      "args": ["--protocol-mode", "mcp_stdio"],
      "env": {
        "OPENAI_API_KEY": "sk-..."
      }
    }
  }
}
```

## Windsurf (Codeium)

File: `~/.codeium/windsurf/mcp_config.json`

```json
{
  "mcpServers": {
    "go-on": {
      "command": "go-on",
      "args": ["--protocol-mode", "mcp_stdio"],
      "env": {
        "OPENAI_API_KEY": "sk-..."
      }
    }
  }
}
```

## Claude Desktop

File: `~/Library/Application Support/Claude/claude_desktop_config.json`

```json
{
  "mcpServers": {
    "go-on": {
      "command": "go-on",
      "args": ["--protocol-mode", "mcp_stdio"],
      "env": {
        "OPENAI_API_KEY": "sk-..."
      }
    }
  }
}
```

## Zed Editor

File: `~/.config/zed/settings.json`

```json
{
  "agent_servers": {
    "go-on": {
      "command": {
        "command": "go-on",
        "args": ["--protocol-mode", "acp_stdio"],
        "env": {
          "OPENAI_API_KEY": "sk-..."
        }
      }
    }
  }
}
```

> See [docs/zed-integration.md](zed-integration.md) for detailed configuration.

## Claude Code CLI

File: `claude.json` (project root)

```json
{
  "mcpServers": {
    "go-on": {
      "command": "go-on",
      "args": ["--protocol-mode", "mcp_stdio"],
      "env": {}
    }
  }
}
```

## Continue.dev

File: `~/.continue/config.json`

```json
{
  "experimental": {
    "mcpServers": {
      "go-on": {
        "command": "go-on",
        "args": ["--protocol-mode", "mcp_stdio"]
      }
    }
  }
}
```

## Cline / Roo Code

File: `~/.config/cline/mcp_settings.json`

```json
{
  "mcpServers": {
    "go-on": {
      "command": "go-on",
      "args": ["--protocol-mode", "mcp_stdio"],
      "env": {}
    }
  }
}
```

## Remote Server (HTTP)

```bash
# Start go-on as HTTP MCP server
go-on --protocol-mode mcp_http -b 0.0.0.0:8090

# Connect any MCP client to http://host:8090/mcp
```

---

## Protocol Matrix

| Editor | Protocol | Config File | Session Mgmt | Tools | Streaming |
|--------|----------|-------------|-------------|-------|-----------|
| Zed | **ACP** (native) | `settings.json` → `agent_servers` | ✅ Full | ✅ 60+ | ✅ |
| VS Code | **MCP** (via ext) | `settings.json` → `mcp.servers` | ❌ | ✅ 60+ | ❌ |
| Cursor | **MCP** | `~/.cursor/mcp.json` | ❌ | ✅ 60+ | ❌ |
| Windsurf | **MCP** | `mcp_config.json` | ❌ | ✅ 60+ | ❌ |
| Claude Desktop | **MCP** | `claude_desktop_config.json` | ❌ | ✅ 60+ | ❌ |
| Claude Code CLI | **MCP** | `claude.json` | ❌ | ✅ 60+ | ❌ |
| Continue.dev | **MCP** | `~/.continue/config.json` | ❌ | ✅ 60+ | ❌ |
| Cline/Roo Code | **MCP** | `mcp_settings.json` | ❌ | ✅ 60+ | ❌ |

> **Note**: MCP does not support session management (new/prompt/close) or streaming chat natively. For the full experience (session lifecycle, streaming, thinking blocks), use Zed with ACP mode. For tool access only, any MCP-compatible editor works.
