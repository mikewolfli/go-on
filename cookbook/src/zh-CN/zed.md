# Zed 接入

Zed 当前可以用两大类方式接入 `go-on`：

- ACP over stdio，由 Zed 直接拉起后端子进程
- HTTP 接入，由 `go-on` 作为本地长驻服务运行，Zed 指向它的 HTTP 面

由于不同版本的 Zed 配置键名可能变化，下面的示例请按“传输方式、命令行、端点”来理解。即使将来具体字段名有调整，也应保持这些核心值不变。

## 方式一：ACP over stdio

适合由 Zed 自己拉起 `go-on`。

推荐命令：

```json
{
  "agent_servers": {
    "go-on-acp": {
      "type": "custom",
      "command": "go-on",
      "args": [
        "--config",
        "/absolute/path/to/zed-config.toml",
        "--protocol-mode",
        "acp_stdio"
      ]
    }
  }
}
```

Windows 示例：

```json
{
  "agent_servers": {
    "go-on-acp": {
      "type": "custom",
      "command": "D:/Workspace/RustWorkspace/go-on/target/debug/go-on.exe",
      "args": [
        "--config",
        "D:/Workspace/RustWorkspace/go-on/zed-config.toml",
        "--protocol-mode",
        "acp_stdio"
      ]
    }
  }
}
```

适用场景：

- 希望由单个 Zed 窗口独占进程生命周期
- 不需要多个客户端共享同一个后端
- 想避免本地监听端口

## 方式二：ACP over HTTP

适合共享长驻后端。

先手动启动后端：

```bash
go-on --config zed-config.toml --protocol-mode acp_http --acp-http-bind 127.0.0.1:8090
```

如果想让同一个后端同时兼顾 MCP 风格客户端，建议改成：

```bash
go-on --config zed-config.toml --protocol-mode adaptive --acp-http-bind 127.0.0.1:8090
```

然后把 Zed 的 external-agent 风格配置指向运行时根地址：

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

ACP HTTP 的健康检查地址是：

```text
http://127.0.0.1:8090/health
```

## 方式三：通过 `/v1` 走 MCP 或模型提供方风格 HTTP

后端还暴露了 OpenAI 兼容端点：

- `/v1/models`
- `/v1/model`
- `/v1/chat/completions`
- `/v1/responses`

如果你的 Zed 版本对应的是 OpenAI 兼容 provider 或 MCP 风格 LLM provider，那么应使用 `/v1` 基址。

示例（按最新 Zed `openai_compatible` 结构）：

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

如果某个模型只支持 Responses API，请将该模型的 `capabilities.chat_completions` 设为 `false`。

适用场景：

- Zed 这一能力要求的是 OpenAI 兼容接口，而不是 ACP 传输
- 希望同一个后端同时提供编辑器对话与 `/v1` 模型探测能力

## 如何选模式

- 希望由 Zed 拉起进程，用 `acp_stdio`。
- 希望 Zed 连接共享 ACP 服务，用 `acp_http`。
- 希望一个后端同时服务多前端，优先用带 `--acp-http-bind` 的 `adaptive`。
- 如果 Zed 这个入口本质上是 provider 配置，而不是 ACP agent，就走 `/v1` 模型提供方面。

## 接入前检查

不要先怪 Zed 配置，先验证后端：

```bash
go-on --config zed-config.toml --status
go-on --config zed-config.toml --validate-config
```

HTTP 模式下，至少手工确认：

```text
GET http://127.0.0.1:8090/health
GET http://127.0.0.1:8090/v1/models
```

## Zed 外部 Agent 新变化

- 从 Zed `v0.221.x+` 开始，ACP Registry 是更推荐的外部 Agent 安装方式。
- 常见内置外部 Agent 名称包括：`claude-acp`、`codex-acp`、`gemini`。
- 建议使用 `zed: acp registry` 进行安装，使用 `dev: open acp logs` 进行联调诊断。

## 三平台路径统一

- Linux 配置文件：`~/.config/zed/settings.json`
- macOS 配置文件：`~/Library/Application Support/Zed/settings.json`
- Windows 配置文件：`%APPDATA%/Zed/settings.json`

## 模型更新策略

- 修改 `available_models` 前，先对齐最新官方文档中的模型 ID 与上下文窗口。
- OpenAI 兼容模型需同时核对 endpoint 兼容性（`chat/completions` 与 `responses`）。
- 使用 Zed 托管模型时，上线前先检查 Zed `AI > Models` 中的 retired/replaced 列表。

## 常见失败模式

- `/health` 能通，但 Zed 仍拒绝 ACP，多半是当前运行时模式偏 MCP-only。
- `/v1/models` 能通，但实际聊天失败，多半是 Provider 没就绪，先看 `go-on --status`。
- stdio 模式一启动就失败，先检查可执行文件路径和 `zed-config.toml` 路径。