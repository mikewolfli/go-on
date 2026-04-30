# Zed 接入

Zed 當前可以用兩大類方式接入 `go-on`：

- ACP over stdio，由 Zed 直接拉起後端子進程
- HTTP 接入，由 `go-on` 作為本地長駐服務運行，Zed 指向它的 HTTP 面

由於不同版本的 Zed 配置鍵名可能變化，下面的示例請按“傳輸方式、命令行、端點”來理解。即使將來具體字段名有調整，也應保持這些核心值不變。

## 方式一：ACP over stdio

適合由 Zed 自己拉起 `go-on`。

推薦命令：

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

Windows 示例：

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

適用場景：

- 希望由單個 Zed 窗口獨佔進程生命週期
- 不需要多個客戶端共享同一個後端
- 想避免本地監聽端口

## 方式二：ACP over HTTP

適合共享長駐後端。

先手動啟動後端：

```bash
go-on --config config.toml --protocol-mode acp_http --acp-http-bind 127.0.0.1:8090
```

如果想讓同一個後端同時兼顧 MCP 風格客戶端，建議改成：

```bash
go-on --config config.toml --protocol-mode adaptive --acp-http-bind 127.0.0.1:8090
```

然後把 Zed 的 external-agent 風格配置指向運行時根地址：

```json
{
  "agent_servers": {
    "go-on-http": {
      "url": "http://127.0.0.1:8090"
    }
  }
}
```

ACP HTTP 的健康檢查地址是：

```text
http://127.0.0.1:8090/health
```

## 方式三：通過 `/v1` 走 MCP 或模型提供方風格 HTTP

後端還暴露了 OpenAI 兼容端點：

- `/v1/models`
- `/v1/model`
- `/v1/chat/completions`
- `/v1/responses`

如果你的 Zed 版本對應的是 OpenAI 兼容 provider 或 MCP 風格 LLM provider，那麼應使用 `/v1` 基址。

示例：

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

適用場景：

- Zed 這一能力要求的是 OpenAI 兼容接口，而不是 ACP 傳輸
- 希望同一個後端同時提供編輯器對話與 `/v1` 模型探測能力

## 如何選模式

- 希望由 Zed 拉起進程，用 `acp_stdio`。
- 希望 Zed 連接共享 ACP 服務，用 `acp_http`。
- 希望一個後端同時服務多前端，優先用帶 `--acp-http-bind` 的 `adaptive`。
- 如果 Zed 這個入口本質上是 provider 配置，而不是 ACP agent，就走 `/v1` 模型提供方面。

## 接入前檢查

不要先怪 Zed 配置，先驗證後端：

```bash
go-on --config config.toml --status
go-on --config config.toml --validate-config
```

HTTP 模式下，至少手工確認：

```text
GET http://127.0.0.1:8090/health
GET http://127.0.0.1:8090/v1/models
```

## 常見失敗模式

- `/health` 能通，但 Zed 仍拒絕 ACP，多半是當前運行時模式偏 MCP-only。
- `/v1/models` 能通，但實際聊天失敗，多半是 Provider 沒就緒，先看 `go-on --status`。
- stdio 模式一啟動就失敗，先檢查可執行文件路徑和 `config.toml` 路徑。