# GUI 桌面控制台

GUI 是基于 EGUI（Rust 原生）的桌面图形界面，位于 `gui/` 目录下。
它提供后端监控、多会话对话、技能管理和设置编辑等功能，
让运维和集成调试不必一直停留在终端里。

## 架构概述

GUI 是一个 Rust 原生桌面应用，使用 EGUI 框架（基于 `eframe`/`egui`）构建。
它通过 ACP+HTTP JSON-RPC 与后端通信，自动管理后端进程的生命周期。

GUI 持有并使用三个核心值：

- 后端可执行文件路径
- 工作目录
- 工作目录中的运行时配置文件

GUI 拉起后端进程时会以工作目录作为当前目录，因此要求 `config.toml` 就放在该目录下。

## 功能面板

### 监控面板 (Monitor)
- 后端健康状态：通过 `/health` 端点自动轮询
- AI 供应商状态：实时显示 Provider 连接状况
- 实时指标：请求数、延迟、错误率

### 对话界面 (Chat)
- 多会话管理：创建、切换、删除会话
- 多模型支持：每个会话可选择不同 AI 模型
- 阶段选择：coding / review / debug / test / deploy
- 模式切换：Ask / Plan / Edit / Safeguard / Full Auto
- 文件附件：支持上传文件作为对话上下文
- 动态发送按钮：依据 AI 状态变化（loading / ready / error）

### 技能管理 (Skills)
- 创建和导入 AI 技能
- 内置 `skill-creator`：让 AI 自主定义新技能
- 技能列表管理：启用、禁用、删除

### 设置面板 (Settings)
- **Provider 管理**：动态环境变量注入（支持全部 34+ 供应商），不再硬编码为 8 个
- **配置文件编辑器**：管理 `gui_config.json`，含 JSON 语法验证
- **主题选择**：6 种视觉主题（简约 / 国风 / 武侠 / 山水 / Hello Kitty / 暗黑）
- **语言切换**：简体中文、繁體中文、English
- **功能开关**：启用/禁用各项 GUI 功能

## 开发与构建命令

在 `gui/` 目录下：

```bash
# 开发运行
cargo run

# 构建（release）
cargo build --release

# 从项目根目录运行
cargo run --manifest-path gui/Cargo.toml
```

## 绑定后端

GUI 可以自动发现后端可执行文件。自动绑定成功后，会把可执行文件所在目录作为工作目录，并把日志落到该目录下的 `go-on.log`。

如果自动发现失败，就手工配置：

1. 填写后端可执行文件路径
2. 填写工作目录
3. 确保该目录下存在 `config.toml`

## 密钥管理

GUI 使用双重存储机制：

- **系统密钥环 (keyring)**：优先使用操作系统级别的密钥存储（如 Linux 的 Secret Service、macOS 的 Keychain、Windows 的 Credential Manager）
- **配置文件 (config file)**：作为备份和便携方案

API Key 无需写入 `.env.goon`，全部通过 GUI 的 Provider 管理界面动态注入。

## 运行时进程行为

GUI 启动后端时，会从当前配置的工作目录拉起该可执行文件，并把 stdout 与 stderr 都写到 `go-on.log`。

**自动重启**：如果后端崩溃，GUI 会在 3 秒冷却后自动重启后端进程。

因此最常见的操作错误是：二进制路径正确，但工作目录错误，导致配置文件找不到或加载了错误配置。

## 健康检查与集成探测

GUI 当前会探测：

- `/health` 上的 ACP 或运行时健康状态
- `/v1/models` 上的 OpenAI 兼容模型列表

这些探测会被解释成以下前端状态：

- Zed 的 ACP 或 A2A over HTTP
- Zed 的 MCP 或 `/v1` 模型提供方风格接入
- VS Code 插件运行时健康状态

## GUI 场景下推荐的后端模式

- `adaptive`：最推荐，适合 GUI 与 Zed、VS Code 共用一个后端。
- `acp_http`：适合只关心 ACP over HTTP。
- `mcp_http`：适合主要关注 `/v1` provider 兼容面。

GUI 本身无论哪种模式都可以继续管理后端可执行文件，模式差异主要体现在 GUI 启动之后外部客户端还能做什么。

## 推荐操作顺序

1. 构建后端：`cargo build`
2. 初始化后端（首次）：`cargo run -- --init`
3. 构建 GUI：`cargo build --manifest-path gui/Cargo.toml`
4. 启动 GUI：`cargo run --manifest-path gui/Cargo.toml`
5. 使用自动绑定，或手工填写可执行文件路径
6. 确认工作目录中存在 `config.toml`
7. 在 Provider 管理中配置 API Key（自动存储到系统密钥环）
8. 启动后端
9. 查看健康状态和集成探测结果

## 故障排查

- 如果启动时报文件错误，先重新检查可执行文件路径。
- 如果启动成功但探测失败，优先检查协议模式和 Provider 就绪状态。
- 如果 GUI 显示健康正常，但编辑器仍失败，就对照编辑器所需传输契约与当前运行时模式是否一致。
- GUI 自身问题：检查 `gui_config.json` 是否损坏，必要时删除重置。
