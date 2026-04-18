# go-on GUI

Desktop console for go-on built with Tauri 2 + Vue + Vite.

## Scope

- Independent project under GUI/
- Does not modify existing go-on source code
- Controls go-on process externally (start/stop/restart)
- Visualizes health, monitoring, logs, and AI usage snapshots
- Monitors Zed/VS Code integration interfaces (process, endpoint, addon)
- Supports zh-CN and en-US UI language switching
- Supports tray resident mode and mini floating console

## Commands

- `npm install` (or `pnpm install`)
- `npm run dev`
- `npm run build`
- `npm run tauri dev`
- `npm run tauri build`

## Notes

- Default polling intervals:
  - Monitoring/Status: 2s
  - Logs: 1s
- Main window closes to tray by default.
- Tray menu supports Show/Start/Stop/Restart/Mini Console/Quit.
- Built executable output:
  - `GUI/src-tauri/target/release/go-on-gui.exe`

## 快速联调
<!-- BLUE14-P2-2-GUI-QUICKSTART -->

1. 先启动 backend：`../start-go-on.sh` 或 `..\\start-go-on.bat`
2. 确认健康端点：`http://127.0.0.1:8090/health`
3. 再启动 GUI：`npm run tauri dev`

推荐协议模式：

- backend `config.toml` 使用：`[protocol].mode = "adaptive"`
- `adaptive` 表示 runtime 保留双栈能力，并按客户端类型选择路径；若配置了 `acp_http_bind_addr`，GUI 会优先走 HTTP 探测
- 若只走 GUI HTTP 链路，可用：`acp_http` 或 `mcp_http`

错误溯源对齐（BLUE14 AGENT4）：

- GUI 运行时 RPC 错误会保留统一格式：`rpc_error:<code>:<kind>:<message> (context=<...>)`
- `kind` 与 backend 对齐：`PuaViolation` / `BudgetExceeded` / `SandboxBlocked`
