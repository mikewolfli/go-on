# VS Code 插件

VS Code 插件是本仓库里功能最完整的编辑器接入面。它暴露了基于运行时的命令，可以探测运行时健康状态，也允许在设置中覆盖后端协议模式。

## 插件依赖什么

插件需要：

- 可访问的 `go-on` 可执行文件
- 有效的 `config.toml`
- 与当前工作流匹配的协议模式

插件清单当前暴露的协议覆盖值为：

- `from_config`
- `adaptive`
- `acp_stdio`
- `acp_http`
- `mcp_stdio`
- `mcp_http`

其中 `from_config` 表示跟随后端配置，其余值表示显式强制覆盖。

## 首次接入建议

1. 先构建后端或准备好可执行文件。
2. 运行 `go-on --setup --setup-level standard`。
3. 如果自动发现不够稳定，再在 VS Code 设置中显式填写可执行文件路径和配置路径。
4. 除非在排查特定传输问题，否则协议模式保持 `from_config`。

## 各协议模式什么时候用

- `from_config`：日常默认。
- `adaptive`：希望一个运行时同时兼容多类探测时优先使用。
- `acp_stdio`：插件应驱动拉起 stdio 运行时时使用。
- `acp_http`：后端已作为共享本地 HTTP 服务运行时使用。
- `mcp_stdio`：只有明确需要 MCP stdio 才用。
- `mcp_http`：明确需要 `/v1` HTTP 语义时使用。

## 运行时健康面

插件契约中的健康检查路径是：

```text
/health
```

OpenAI 兼容探测路径是：

```text
/v1/models
```

插件同时也知道这些路径：

- `/v1/model`
- `/v1/chat/completions`
- `/v1/responses`

## 实用工作区设置示例

```json
{
  "go-on.runtime.protocolMode": "from_config",
  "go-on.runtime.executablePath": "D:/Workspace/RustWorkspace/go-on/target/debug/go-on.exe",
  "go-on.runtime.configPath": "D:/Workspace/RustWorkspace/go-on/config.toml"
}
```

如果要强制共享 HTTP 运行时：

```json
{
  "go-on.runtime.protocolMode": "acp_http"
}
```

## 实际排查顺序

对插件来说，推荐按下面顺序排查：

1. `go-on --validate-config`
2. `go-on --status`
3. 检查 VS Code 设置里的 executable path
4. 检查 VS Code 设置里的 config path
5. 最后再看是否需要协议模式覆盖

## 什么时候选 HTTP，什么时候选 stdio

优先选 HTTP：

- 希望 GUI 与 VS Code 共享同一个后端
- 希望手工探测 `/health` 与 `/v1/models`
- 希望后端作为长驻本地服务存在

优先选 stdio：

- 希望 VS Code 自己管理进程启停
- 希望不同工作区完全隔离

## 常见失败模式

- 插件能拉起可执行文件，但提示 provider not ready，问题多半在配置或凭证，不在传输层。
- 选了 HTTP 模式但 `/health` 不通，说明后端并未用 `--acp-http-bind` 启动。
- 强制 `mcp_http` 时，要确认当前消费该能力的插件路径确实需要 `/v1` 语义，而不是 ACP 语义。