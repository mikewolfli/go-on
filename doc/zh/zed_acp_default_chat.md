# Zed 默认 Chat ACP 集成 go-on 指南

Zed 编辑器自带的 Chat ACP（AI Chat Provider）支持自定义后端，可以直接集成 go-on，无需开发插件。

## 步骤

### 1. 启动 go-on 服务
确保 go-on 已运行并监听本地端口，例如：
```bash
./target/release/go-on --acp-http-bind 127.0.0.1:8080
```

### 2. 配置 Zed 的 Chat Provider
1. 打开 Zed 设置（`Cmd+,`）。
2. 搜索 `AI` 或 `Chat`，找到“AI Providers”配置项。
3. 添加自定义 provider，类型选择 `OpenAI Compatible` 或 `Custom`。
4. 填写如下信息：
   - **API URL**：`http://127.0.0.1:8080/chat`
   - **模型名称**：可自定义，如 `go-on`
   - **API Key**：留空或随意填写（go-on 默认不校验）
   - **请求格式**：选择 `OpenAI` 兼容（go-on 支持 OpenAI 风格 JSON）

### 3. 使用
- 在 Zed 侧边栏点击 Chat，选择你刚添加的 go-on provider。
- 输入内容即可与 go-on 后端对话。

### 4. 进阶
- 如需流式对话，将 API URL 设置为 `/chat/stream`。
- 可在 go-on 配置中自定义 phase、agent、路由等。
- 若需身份校验，可在 go-on 增加 API Key 校验逻辑。

---

> Zed 的 Chat ACP 机制允许直接对接本地或远程 LLM 服务，go-on 默认兼容 OpenAI API 协议，集成体验流畅。
