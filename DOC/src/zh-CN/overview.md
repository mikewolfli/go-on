# 架构总览

`go-on` 当前是一个围绕 Rust 后端构建的三端运行时体系：

- 后端：负责配置加载、Provider 选择、路由、setup、健康检查、协议协商，以及 stdio 或 HTTP 传输层。
- GUI：Tauri 桌面控制台，负责后端发现、进程生命周期、集成探测和本地运维。
- VS Code 插件：负责拉起或探测运行时，暴露基于 RPC 的命令，并可在工作区级别覆盖协议模式。

## 当前运行时模型

后端支持 5 种访问模式：

- `adaptive`：尽量自动协商 ACP 与 MCP 兼容路径。
- `acp_stdio`：通过 stdio 提供 ACP，适合编辑器拉起子进程。
- `acp_http`：通过 HTTP 暴露 ACP 风格接口，适合共享长驻后端。
- `mcp_stdio`：通过 stdio 提供 MCP。
- `mcp_http`：通过 HTTP 暴露 MCP 与 OpenAI 兼容接口。

当以后端 `--acp-http-bind` 启动时，默认会围绕 `http://127.0.0.1:8090` 暴露实际可用的 HTTP 面：

- `/health`
- `/chat`
- `/chat/stream`
- `/v1/models`
- `/v1/model`
- `/v1/chat/completions`
- `/v1/responses`

这也是三端分工的关键：

- Zed 既可以走 ACP stdio，也可以走 ACP HTTP。
- Zed 也可以把后端当成 OpenAI 兼容的 `/v1` 模型提供方。
- VS Code 插件既可以走拉起式 stdio RPC，也可以探测 HTTP 运行时。
- GUI 依赖本地后端可执行文件，并要求工作目录中存在 `config.toml`。

## 与架构对应的仓库目录

- `src/`：后端运行时、CLI、setup、ACP 与 MCP 实现。
- `GUI/`：Tauri 桌面控制台。
- `vscode-addon/`：VS Code 插件。
- `config.toml` 与 `config.toml.autopilot-adaptive`：运行时配置基线。
- `requests/`：请求和基准样例。
- `scripts/`：辅助脚本。

## 推荐运维路径

新机器或新工作目录，最短路径通常是：

1. 构建或准备 `go-on` 后端可执行文件。
2. 运行 `go-on --setup --setup-level standard`。
3. 用 `go-on --status` 检查运行时就绪状态。
4. 如果前端要走 HTTP，使用 `--protocol-mode adaptive --acp-http-bind 127.0.0.1:8090` 启动后端。
5. 再接入 Zed、VS Code 插件或 GUI。

后续章节分别展开说明。