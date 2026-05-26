<p align="center">
  <img src="snapshots/chat.png" alt="go-on" width="600">
</p>

<p align="center">
  <strong>go-on</strong> — 用 Rust 编写的 ACP/MCP 智能体编排运行时，提供桌面 GUI、VS Code 插件，支持 35+ AI 供应商。
</p>

<p align="center">
  <a href="README.md">English</a> | 简体中文
</p>

---

[![Rust](https://img.shields.io/badge/rust-1.1.0-orange?logo=rust)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Tests](https://img.shields.io/badge/tests-1400%2B-brightgreen)]()
[![Providers](https://img.shields.io/badge/providers-35%2B-9cf)]()
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey)]()

## go-on 是什么？

go-on 是一个**本地优先**、生产级的 AI 智能体运行时，使用 Rust 编写。它通过标准智能体协议（ACP / MCP）连接大语言模型与你的工具和工作流。你可以将它作为 CLI 工具、桌面应用或后端服务器运行 — 内置完整的自治循环、工具编排与治理能力。

**用 go-on 你可以：**
- 🖥️ 通过原生桌面 GUI 或终端与 AI 模型对话
- 🤖 运行能自主规划、执行、自我纠错的智能体
- 🔧 通过依赖感知的 DAG 执行引擎编排多工具工作流
- 🔌 将 AI 模型连接到 MCP 服务器，或作为 MCP 服务器运行
- 🛡️ 通过 RBAC、审计追踪和风险评估执行治理策略
- 🧩 通过 VS Code 插件或 Rust/Python SDK 扩展

## 快速开始

```bash
# 编译并运行（自动生成默认配置）
cargo run

# 启动桌面 GUI
cargo run --manifest-path gui/Cargo.toml

# 终端对话模式
cargo run -- --chat

# 作为 MCP 服务端启动
cargo run -- --protocol-mode mcp-stdio
```

首次运行时会检测 AI 供应商配置，若未配置则启动交互式初始化向导。  
默认健康检查：`http://127.0.0.1:8090/health`

---

## 截图预览

| 监控面板 | 对话界面 |
|:---:|:---:|
| ![监控面板](snapshots/monitor.png) | ![对话界面](snapshots/chat.png) |

| 供应商管理 | 设置 |
|:---:|:---:|
| ![供应商管理](snapshots/providers.png) | ![设置](snapshots/settings.png) |

| 技能管理 |
|:---:|
| ![技能管理](snapshots/skills.png) |

---

## 核心功能

### 智能体编排
- **自治循环** — 规划 → 执行 → 反思 → 重规划，迭代次数根据复杂度自适应调整
- **DAG 任务执行** — Kahn 拓扑排序、依赖边、并行组执行、循环检测
- **全自动流程** — 意图解析 → 技能发现 → 环境准备 → 执行 → 报告
- **快路径缓存** — SHA-256 指纹索引、TTL/LRU 淘汰、四层缓存（意图/技能/环境/路由）
- **预测式重路由** — 基于健康评分的主动智能体切换，非仅失败后回退

### AI 供应商支持（35+）
OpenAI · Anthropic · DeepSeek · Gemini · Groq · Ollama · Mistral · Qwen · Llama · Cohere · AI21 · Perplexity · Together · Fireworks · Replicate · MiniMax · Moonshot · 智谱 GLM · 百度千帆 · 字节豆包 · 腾讯混元 · StepFun · Skywork · 以及更多。

OpenAI、Anthropic、DeepSeek、Gemini 四家供应商已支持原生 Function Call。

### 协议与传输
- **ACP**（Agent Client Protocol）— stdio + HTTP，JSON-RPC 2.0
- **MCP**（Model Context Protocol）— stdio + HTTP，工具列表/调用、流式传输、取消、超时
- **5 种模式**：`adaptive`（双栈）、`acp-stdio`、`acp-http`、`mcp-stdio`、`mcp-http`
- **跨入口一致性** — 同一任务在 ACP/CLI/MCP 下产生一致的 stop_reason 与回合数

### 工具系统
- **16 个内置工具** — 读写文件、代码搜索、补丁应用、测试运行、Git Diff、Shell 执行、HTTP 请求、数据库查询等
- **工具流水线** — 串行/并行/条件执行，可配置的错误处理策略
- **工具事务** — 幂等键去重、WAL 持久化、补偿操作、两阶段提交（2PC）
- **动态工具推荐** — 基于模式+近因+共现的工具推荐引擎
- **原生函数调用** — OpenAI/Anthropic tool_choice、Gemini functionCall、DeepSeek tools 参数

### 治理与安全
- **HarnessBus** — 中央治理层，含策略评估、漂移检测、安全检查
- **PUA 规则引擎** — 实时策略评估与升级级别
- **RBAC 权限控制** — 基于角色的访问控制，支持多源租户注册
- **租户隔离** — 跨租户访问阻断；基于预算的并发限制
- **审计追踪** — 完整决策流水线记录，支持回放
- **Safeguard 安全审查** — AI 驱动的高风险操作评估

### 性能
- **FastPathCache** — 重复查询的亚毫秒级缓存命中
- **SSE 缓冲池** — 零分配的流式事件序列化
- **缓存预热** — 预测性预加载，自适应 TTL，多层管理
- **并发执行** — 按角色 BinaryHeap 出队（O(log n)），信号量背压
- **DAG 汇聚超时** — tokio::time::timeout 防止单工具拖尾延迟

### 韧性
- **恢复编排器** — 6 种策略：重试 → 重路由 → 重规划 → 修复 → 升级 → 降级
- **混沌测试** — 10 种故障注入（超时、分区、崩溃、数据损坏、限流等）
- **熔断器** — 基于状态机的快速失败与冷却
- **热故障切换** — 主备模型切换，含冷却黑名单

### 可观测性
- **governance.status 端点** — 真实 p95 延迟、DAG 宽/深度、缓存指标、幂等冲突率
- **OpenTelemetry 追踪** — 请求路由、工具执行、智能体选择的 Span
- **审计回放** — 完整的任务执行证据链，可重现、可筛选

### 会话管理
- **会话上下文管理** — 关键概念提取、消息重要性评分、连续性标记
- **会话压缩** — 超限消息的语义压缩，保留最近的 + 系统 + 指令类消息
- **上下文窗口预算** — 超令牌限制时的智能消息保留

### 配置与部署
- **配置热重载** — 基于文件监控，运行时原子替换配置
- **Schema 版本管理** — semver 语义化版本跟踪，前向/后向迁移
- **3 种构建配置** — local（SQLite）、simple-server（SQLite）、multi-users-server（PostgreSQL + pgvector）
- **系统密钥环集成** — macOS Keychain、Linux Secret Service、Windows Credential Manager

---

## 架构

go-on 采用 **14 条总线能力架构**，含 21 个认知（F-GAP）模块：

```
┌──────────────────────────────────────────────────────────┐
│                     HarnessBus (治理层)                  │
│  策略评估 · 漂移检测 · 韧性 · 安全 · 审计               │
├──────────────────────────────────────────────────────────┤
│                   CapabilityBus (智能层)                  │
│  感知 → 决策 → 行动 → 反馈 → 进化                       │
├──────────┬──────────┬──────────┬──────────┬─────────────┤
│ ToolBus  │ObservB.  │OptimizB. │MemoryBus │ProtocolBus  │
├──────────┼──────────┼──────────┼──────────┼─────────────┤
│OrchestB. │          │          │DistMemB. │             │
└──────────┴──────────┴──────────┴──────────┴─────────────┘
```

### 核心能力模块

| 模块 | 说明 |
|:-----|:-----|
| **Planner 规划器** | 任务自适应的 DAG 规划，含依赖推断 |
| **DAG Driver 执行引擎** | 拓扑排序执行，并行组调度 |
| **BrainLoop 脑回路** | 规划→执行→反思→重规划的认知循环 |
| **CapabilityBus 能力总线** | 多因子智能体选择（信誉+近因+任务匹配+近期结果） |
| **SelfModelCore 自模型** | 系统自感知与能力追踪 |
| **MetacognitiveController 元认知** | 观察驱动的反思与纠偏 |
| **WorldModel 世界模型** | 实体/事件/关系追踪流水线 |
| **FederatedRL 联邦强化学习** | 跨节点的分布式强化学习 |
| **DriftProtection 漂移防护** | 目标/能力/行为的漂移检测 |
| **HyperResilience 超弹性** | 熔断器、故障切换组、自愈 |
| **MultiChannelTransport 多渠道传输** | QoS 感知、去重、优先级消息传输 |

---

## 扩展

### VS Code 插件
`vscode-addon/` 目录包含 VS Code 扩展，可启动 go-on 运行时并在编辑器内提供 60+ 命令 — 对话、工作流执行、技能管理和配置。

```bash
cd vscode-addon
npm install
npm run compile
```

### SDK
- **Rust SDK**（`sdk/rust/`）— 强类型客户端，覆盖 go-on ACP/MCP 端点，8 个领域 40+ 方法
- **Python SDK**（`sdk/python/`）— 基于 HTTPX 的客户端，支持流式传输，含 `py.typed` 标记

---

## 构建配置

| 配置 | 后端 | 适用场景 | 构建命令 |
|:-----|:-----|:---------|:--------|
| `profile-local` | SQLite + sqlite-vec | 单用户本地工具 | `cargo build`（默认） |
| `profile-simple-server` | SQLite + sqlite-vec | 单服务器部署 | `cargo build --no-default-features -F profile-simple-server` |
| `profile-multi-users-server` | PostgreSQL + pgvector | 多用户生产环境 | `cargo build --no-default-features -F profile-multi-users-server` |

---

## 验证状态

| 配置 | cargo check | cargo clippy `-D warnings` | 测试 |
|:-----|:-----------:|:--------------------------:|:----:|
| `profile-local` | ✅ 0 errors | ✅ 0 warnings | 800+ |
| `profile-simple-server` | ✅ 0 errors | ✅ 0 warnings | 900+ |
| `profile-multi-users-server` | ✅ 0 errors | ✅ 0 warnings | 1,000+ |

---

## 许可证

MIT License — 详见 [LICENSE](LICENSE)。
