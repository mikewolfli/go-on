# Zed Chat 指定 app 路径集成 go-on

Zed 的 Chat ACP 支持通过“App 路径”直接管理后端服务的启动与关闭，无需手动运行 go-on。

## 步骤

### 1. 配置 go-on App 路径
1. 打开 Zed 设置（`Cmd+,`）。
2. 搜索 `AI` 或 `Chat`，进入“AI Providers”配置。
3. 添加自定义 provider，类型选择 `OpenAI Compatible` 或 `Custom`。
4. 在“App 路径”或“App Path”字段填写 go-on 可执行文件路径，例如：
   - macOS/Linux: `/完整路径/到/go-on/target/release/go-on`
   - Windows: `C:\完整路径\到\go-on.exe`
5. 设置启动参数（如有）：
   - `--acp-http-bind 127.0.0.1:8080`
6. 其他参数同前述 API URL 配置。

### 2. 使用
- 启动 Chat 时，Zed 会自动启动 go-on 后端。
- 关闭 Chat 或 Zed 时，go-on 进程会自动关闭。

### 3. 优势
- 无需手动管理后端进程，体验更流畅。
- 可为不同 provider 配置不同 app 路径和参数。

---

> 通过“App 路径”集成，Zed 可自动拉起和关闭 go-on 服务，适合本地开发和多后端场景。
