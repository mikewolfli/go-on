# 架构总览

`go-on` 是一个围绕 Rust 后端构建的三端运行时体系：

- **后端**：负责配置加载、Provider 选择、路由、setup、健康检查、协议协商、stdio 或 HTTP 传输层，以及包含 7 个特性门控子总线和认知模块的能力架构。
- **GUI**：EGUI（Rust 原生）桌面图形界面，负责后端发现、进程生命周期、集成探测、监控、对话和配置管理。
- **VS Code 插件**：负责拉起或探测运行时，暴露基于 RPC 的命令，并可在工作区级别覆盖协议模式。

## 版本

- 后端 Runtime：**1.6.0**
- GUI 桌面端：**1.6.0**
- VS Code 插件：**1.6.0**

## GUI 桌面应用

基于 EGUI 的桌面图形界面（`gui/`）提供监控、对话和配置管理：

```bash
cargo run --manifest-path gui/Cargo.toml
```

主要功能：
- **监控面板**：后端健康状态、AI 供应商状态、实时指标
- **对话界面**：多会话管理、阶段选择（来自后端配置，默认 think/act/check/done）、模式切换（Ask/Plan/Edit/Safeguard/Full Auto）、文件附件、动态发送按钮（依据 AI 状态变化）
- **技能管理**：创建和导入 AI 技能；内置 `skill-creator` 让 AI 自主定义新技能
- **设置**：功能开关、语言切换（en/zh-CN/zh-TW）、6 种视觉主题
- **后端连接**：ACP+HTTP JSON-RPC，自动健康轮询

## 构建配置文件

三种构建配置文件适用于不同的部署场景，外加 `full` 用于 CI：

| 配置文件 | 后端 | 使用场景 | 构建命令 |
|:--------|:------|:---------|:--------|
| `local` | SQLite + sqlite-vec | 单用户本地工具 | `cargo build`（默认） |
| `simple-server` | SQLite + sqlite-vec | 单服务器部署 | `cargo build --no-default-features -F simple-server` |
| `multi-users-server` | PostgreSQL + pgvector | 多用户生产 | `cargo build --no-default-features -F multi-users-server` |
| `full` | SQLite（全部特性） | CI / 开发 | `cargo build --no-default-features -F full` |

## 验证状态

| 配置文件 | `cargo clippy -D warnings` | 测试数 |
|:--------|:--------------------------:|:------:|
| **local** | ✅ **零警告** | **全部通过（~1.7K）** |
| **simple-server** | ✅ **零警告** | **全部通过** |
| **full** | ✅ **零警告** | **全部通过** |
| **multi-users-server** | ✅ **零警告** | **全部通过** |

最近一次完整 `cargo test --all-targets` 运行全部通过、零失败（最新计数见 `CHANGELOG.md` 最新一节）。E2E 测试不需要外部基础设施，也无需 `#[ignore]`（见 `tests/structural_tests.rs`）。

## 运行时协议模式

后端支持 5 种访问模式：

- `adaptive`（推荐默认）：双栈协议能力加按请求类型路由。
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

## 架构：多总线能力系统

go-on 实现了以 **CapabilityBus** 和 **HarnessBus** 为核心的**子总线架构**（7 个特性门控子总线，见 `Cargo.toml`）。

### 核心总线

| 总线 | 模块 | 说明 |
|:----|------|------|
| **CapabilityBus** | `src/intelligence/capability_bus/core.rs` | 中央智能总线，编排 sense/decide/evolve 生命周期 |
| **HarnessBus** | `src/governance/harness_bus.rs` | 治理入口，策略评估、漂移/弹性/安全检查 |

### 子总线（Phase 4）

| 总线 | 模块 | 说明 |
|:----|------|------|
| **ToolBus** | `capability_bus/tool_bus.rs` | 统一工具/Skill 调用，能力矩阵，Agent-工具匹配 |
| **ObservabilityBus** | `capability_bus/observability_bus.rs` | 统一可观测：延迟、错误率、Agent 健康 |
| **OptimizationBus** | `capability_bus/optimization_bus.rs` | 成本/速度/可靠性推荐，熔断器 |
| **MemoryBus** | `capability_bus/memory_bus.rs` | 级联缓存（L1→L2→L3），向量存储查找 |
| **ProtocolBus** | `capability_bus/protocol_bus.rs` | 协议感知路由，健康/延迟追踪 |
| **OrchestrationBus** | `capability_bus/orchestration_bus.rs` | 流程/模式/路由编排，模式推荐 |
| **DistributedMemoryBus** | `capability_bus/distributed_memory_bus.rs` | 跨节点记忆共享（特性门控） |

### 总线生命周期

```
sense()   →  聚合 Agent 健康、可用模式、优化推荐
decide()  →  结合模式推荐与工具-Agent 匹配
evolve()  →  更新 Q 表、记录共识投票、发送进化事件
execute_tool() → HarnessBus evaluate() → ToolBus execute() → ObservabilityBus record()
```

## 安全特性

| 功能 | 描述 |
|:----|:------|
| **mTLS** | ACP HTTP 监听器的双向 TLS，支持证书锁定和过期监控 |
| **请求签名** | 使用 Ed25519 或 HMAC-SHA256 对 JSON-RPC 请求进行签名认证 |
| **Vault 集成** | 集成 HashiCorp Vault 进行密钥生命周期管理 |
| **系统密钥环** | macOS Keychain、Linux Secret Service、Windows Credential Manager |
| **内容安全** | 运行时内容扫描，可配置安全策略（SafeGuard 模式） |
| **提示注入检测** | 运行时扫描注入模式，可配置检测阈值 |

## 可观测性

go-on 提供生产级的可观测能力：

| 能力 | 详情 |
|:-----|:-----|
| **Prometheus `/metrics` 端点** | 16+ 指标，包括延迟、吞吐量、缓存命中率 |
| **OpenTelemetry 追踪** | OTLP 导出（默认端点 `localhost:4317`），路由/执行/选择的跨度 |
| **治理状态端点** | 通过 `governance.status` JSON-RPC 获取实时 p95 延迟、DAG 指标、缓存统计 |
| **OTel stdout 导出器** | 当无 OTLP 收集器时可回退到标准输出导出跟踪 |

## 国际化（i18n）

go-on 在后端实现了约 **95%** 的全链路国际化覆盖：

| 语言 | 文件 | 键值数 |
|:-----|:-----|:------:|
| 英语（美国） | `config/languages/en-US.json` | 733 |
| 简体中文 | `config/languages/zh-CN.json` | 733 |
| 繁体中文 | `config/languages/zh-TW.json` | 733 |

覆盖层：ACP/MCP HTTP 错误（100%）、Agent 供应商模块（100%，37 家供应商）、配置验证（100%）、CLI 初始化（100%）、API 处理错误（100%）、编排层（100%）、GUI（约 98%）、VS Code 插件（70+ 键值）。

## 与架构对应的仓库目录

- `src/`：后端运行时、CLI、setup、ACP 与 MCP 实现。
  - `src/acp/`：ACP 服务、请求路由、workflow/task/chat/checkpoint
  - `src/agents/`：Provider 适配器（OpenAI、Anthropic、DeepSeek、Gemini、xAI Grok、SiliconFlow 等 37 家）
  - `src/core/`：配置、初始化、就绪性检查、错误模型
  - `src/governance/`：策略/规则治理、审计、安全治理器、漂移防护
  - `src/intelligence/`：选择器、强化学习、能力总线、发现、共识、演化
  - `src/orchestration/`：流程/模式/路由编排、脑回路、全能模式、制品层
  - `src/fault_tolerance.rs`：跨节点容错引擎
  - `src/resilience/`：超弹性引擎
  - `src/protocol/`：协议服务、JSON-RPC、多渠道消息传输
  - `src/i18n/`：语言运行时
- `gui/`：EGUI（Rust 原生）桌面图形界面
- `vscode-addon/`：VS Code 插件（支持 en-US、zh-CN、zh-TW 多语言）
- `config/`：配置文件
- `tests/`：集成测试与回放资产
- `scripts/`：质量/发布门禁脚本

## 推荐运维路径

新机器或新工作目录，最短路径通常是：

1. 构建或准备 `go-on` 后端可执行文件。
2. 运行 `go-on --setup --setup-level standard`。
3. 用 `go-on --status` 检查运行时就绪状态。
4. 如果前端要走 HTTP，使用 `--protocol-mode adaptive --acp-http-bind 127.0.0.1:8090` 启动后端。
5. 再接入 Zed、VS Code 插件或 GUI。

后续章节分别展开说明。