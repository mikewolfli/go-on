<p align="center">
  <img src="snapshots/head.png" alt="go-on" width="600">
</p>

<p align="center">
  <strong>go-on</strong> — 用 Rust 编写的 AI 智能体编排运行时，提供桌面 GUI、VS Code 插件、SSE 流式传输、MCP/ACP 协议、自治工作流与内置治理。v1.5.3
</p>

<p align="center">
  <a href="README.md">English</a> | 简体中文
</p>

---

[![Rust](https://img.shields.io/badge/rust-1.80+-orange?logo=rust)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Tests](https://img.shields.io/badge/tests-passing-brightgreen)]()
[![Clippy](https://img.shields.io/badge/clippy-zero%20warnings-success)]()
[![Providers](https://img.shields.io/badge/providers-37-9cf)]()
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey)]()
[![LOC](https://img.shields.io/badge/code-252K-blue)]()

## go-on 是什么？

go-on 是一个**本地优先**、生产级的 AI 智能体编排运行时，使用 Rust 编写。它通过 SSE 流式传输、标准智能体协议（ACP / MCP）和认知循环架构，连接大语言模型与你的工具、文件和工作流。你可以将它作为 CLI 工具、桌面 GUI 应用或后端服务器运行 — 内置自治循环、DAG 工具编排、子代理面板与治理能力。

**用 go-on 你可以：**
- 🖥️ 通过原生桌面 GUI（EGUI）或终端与 AI 模型对话
- 🤖 运行能自主规划、执行、反思、自我纠错的智能体
- 🧩 选择 **5 种对话模式**：Ask、Plan、Edit、SafeGuard、FullAuto
- 🔧 通过依赖感知的 DAG 执行引擎编排多工具工作流
- 🔌 将 AI 模型连接到 MCP 服务器，或作为 MCP 服务器运行
- 🛡️ 通过 RBAC、审计追踪和风险评估执行治理策略
- 📊 通过 SSE 面板实时监控子代理执行和命令输出
- 🧩 通过 VS Code 插件、技能市场（34 个技能）或 Rust/Python/TypeScript SDK 扩展

## 快速开始

```bash
# 编译并运行（自动生成默认配置）
cargo run

# 启动桌面 GUI
cargo run --manifest-path gui/Cargo.toml

# 终端对话模式
cargo run -- --chat

# 作为 MCP 服务端启动
cargo run -- --protocol-mode mcp_stdio
```

首次运行时会检测 AI 供应商配置，若未配置则启动交互式初始化向导。

**完整文档**：`cookbook/` 目录（mdBook 格式，支持三语） — `cd cookbook && mdbook serve --open`

默认健康检查端点：`http://127.0.0.1:8090/health`

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
- **5 种对话模式** — Ask（流式对话）、Plan（仅生成计划大纲）、Edit（迭代编辑+高风险审查）、SafeGuard（风险评分门控+人工确认）、FullAuto（全自动+记忆+验证）
- **自治循环** — 规划 → 执行 → 反思 → 重规划，迭代次数根据复杂度自适应调整
- **子代理监控** — GUI 中通过 SSE 面板实时显示子代理执行和命令输出
- **DAG 任务执行** — Kahn 拓扑排序、依赖边、并行组执行、循环检测
- **工作流/任务执行** — `workflow.execute` / `task.execute`，含需求门控、确定性评审与自主修复循环
- **多模型投票** — 高风险决策的并发智能体投票（多数/加权/一致/融合）

### AI 供应商支持（37 家）
OpenAI · Anthropic · DeepSeek · Gemini · xAI Grok · Groq · Mistral · Qwen · Llama · Copilot · SiliconFlow · Cohere · AI21 · Perplexity · Together · Fireworks · Replicate · MiniMax · Moonshot · 智谱 GLM · 百度千帆 · 字节豆包 · 腾讯混元 · StepFun · Skywork · Yi · Kimi · NIM · Aleph Alpha · DeepQuest · FaceWall · LoopAI · Langboat · Titan · 文心 · 西湖

OpenAI、Anthropic、DeepSeek、Gemini、Groq、xAI Grok 六家支持原生 Function Call。

### 协议与传输
- **ACP**（Agent Client Protocol）— stdio + HTTP，JSON-RPC 2.0
- **MCP**（Model Context Protocol）— stdio + HTTP，工具列表/调用、流式传输、取消、超时
- **5 种传输模式**：`adaptive`（双栈）、`acp_stdio`、`acp_http`、`mcp_stdio`、`mcp_http`
- **SSE 流式传输协议** — chunk、done、telemetry、error、state_sync、sub_agent、command + Responses API 事件
- **跨入口一致性** — 同一任务在 ACP/CLI/MCP 下产生一致的 stop_reason 与回合数

### 工具系统
- **108 个内置工具** — 读写文件/代码搜索/补丁应用/测试运行/Git Diff/Shell 执行/HTTP 请求/grep/search_files/git/cargo_check/cargo_test/目录列表/文件移动/文件删除/压缩解压/日期时间/DNS/Ping/端口扫描 + CAD/3D/GIS/条码/SVG/Office/图像处理 + 文档解析(PDF/DOCX/PPT/HTML/Markdown/Excel)
- **工具流水线** — 串行/并行/条件执行，可配置的错误处理策略
- **动态工具推荐** — 基于模式+近因+共现的工具推荐引擎
- **基于模式的工具限制** — 各模式强制执行 allowed_tools 与 max_tool_calls

### 治理与安全
- **HarnessBus** — 中央治理层，含策略评估、漂移检测、安全检查
- **PUA 规则引擎** — 实时策略评估与升级级别
- **RBAC 权限控制** — 基于角色的访问控制，支持租户注册
- **租户隔离** — 跨租户访问阻断；基于预算的并发限制
- **审计追踪** — 完整决策流水线记录，支持回放
- **审计完整性** — 每一条审计条目哈希链防篡改（`governance.audit.verify` + 可选 Ed25519 签名）
- **提示注入检测** — 运行时扫描注入模式，可配置阈值
- **内容安全审查** — AI 驱动的高风险操作评估

### 性能
- **SSE 缓冲池** — 零分配的流式事件序列化
- **并发执行** — 按角色 BinaryHeap 出队（O(log n)），信号量背压
- **DAG 汇聚超时** — 防止单工具拖慢整个流水线

### 韧性
- **容错恢复** — 5 个真实恢复动作（RestartNode、FailoverToBackup、ScaleUp、Rebalance、NotifyOperator），由 `FaultToleranceEngine` 分派
- **混沌测试** — 14 种故障注入（超时、分区、崩溃、数据损坏、限流、OOM、延迟尖峰、部分写入等）
- **超弹性** — 熔断器状态机、故障切换组、自愈
- **热故障切换** — 主备模型切换，含冷却黑名单

### 可观测性
- **Prometheus `/metrics` 端点** — 16+ 指标，含延迟、吞吐量、缓存命中率
- **OpenTelemetry 追踪** — OTLP/stdout 导出，路由/执行/选择 Span
- **治理状态端点** — 实时 p95 延迟、DAG 指标、缓存统计
- **审计回放** — 完整任务执行证据链，可筛选

### 会话管理
- **会话上下文** — 关键概念提取、消息重要性评分、连续性标记
- **会话压缩** — 超限消息的语义压缩
- **上下文窗口预算** — 令牌限制内的智能消息保留

### 安全
- **请求签名** — Ed25519 或 HMAC-SHA256 的 JSON-RPC 请求认证
- **mTLS** — ACP HTTP 监听器的双向 TLS，含证书固定与过期监控
- **密钥轮换** — HashiCorp Vault 集成，密钥生命周期管理
- **系统密钥环** — macOS Keychain、Linux Secret Service、Windows Credential Manager
- **内容安全** — 运行时内容扫描，可配置策略

### 配置与部署
- **配置热重载** — 基于文件监控，运行时原子替换
- **Schema 版本管理** — semver 语义化版本跟踪与迁移
- **4 种构建配置** — local（SQLite）、simple-server（SQLite）、multi-users-server（PostgreSQL + pgvector）、full（全部特性）
- **Docker** — 生产容器含 HEALTHCHECK，提供 k8s 清单
- **OTel** — 通过 OTLP collector 的分布式追踪（默认：`localhost:4317`）
- **三语国际化** — 英文、简体中文、繁体中文，覆盖后端/GUI/VS Code 约 95%

### 技能市场（34 个技能）
- **内置技能**：api-docs-generator、api-tester、architecture-diagrammer、changelog-generator、ci-pipeline-generator、classify-text、code-execution-sandbox、code-review、context-summarizer、data-pipeline-optimizer、data-transformer、dockerfile-generator、env-config-validator、error-recovery-planner、knowledge-retriever、log-analyzer、note-taking、performance-analyzer、progress-tracker、project-analyzer、prompt-optimizer、refactoring-advisor、regex-builder、security-auditor、self-reviewer、semantic-diff、skill-creator、sql-query-helper、summarize-text、task-planner、test-generator、translate-text、web-scraper、workflow-optimizer（与 `skills/` 目录一致；部分条目为合并后的展示名，如 analyze-text←classify-text、conventional-commits-toolkit←changelog-generator）
- **从 GitHub/URL/本地导入** — SkillImportStore 获取并验证 SKILL.md 清单
- **自动发现** — 启动时扫描 `~/.agents/skills/` 目录

---

## 架构

go-on 采用**子总线能力架构** —— 7 个特性门控子总线（tool、orchestration、observability、optimization、memory、protocol、distributed-memory）—— 含认知循环和统一的 **DispatchOutput** handler 模式：

> 子总线特性门控定义在 `Cargo.toml`：`sub-bus-tool`、
> `sub-bus-orchestration`、`sub-bus-observability`、`sub-bus-optimization`、
> `sub-bus-memory`、`sub-bus-protocol` 和 `sub-bus-distributed-memory`。
> `local` 配置启用六个子总线（tool、orchestration、observability、optimization、memory、protocol）；
> `simple-server` 与 `multi-users-server` 额外启用 distributed-memory（全部七项）。
> 下图将上述子总线归组为上层能力模块。

```
┌────────────────────────────────────────────────────────────┐
│                    HarnessBus (治理层)                      │
│  策略评估 · 漂移检测 · 韧性 · 安全 · 审计                  │
├────────────────────────────────────────────────────────────┤
│                   CapabilityBus (智能层)                    │
│  感知 → 决策 → 行动 → 反馈 → 进化                         │
├──────────┬──────────┬──────────┬──────────┬───────────────┤
│ ToolBus  │ ObservB. │ MemoryBus│ ProtocolB.│ OrchestB.    │
├──────────┼──────────┼──────────┼──────────┼───────────────┤
│ Unified  │ Reinforc.│ Learning │ Capab.   │ DistMemB.    │
│ Knowl.B. │ ementBus │ OptimB.  │ Graph    │              │
├────────────────────────────────────────────────────────────┤
│              CommunicationBus (智能体通信层)                 │
│  AgentPath · AgentMessenger · ContextForker                │
└────────────────────────────────────────────────────────────┘
```

### 请求处理分发

所有 154 个 JSON-RPC handler 返回统一的 `DispatchOutput` 枚举，dispatch 层自动序列化为对应的传输响应：

```
Handler → Result<DispatchOutput> → dispatch_to_client → JSON-RPC / SSE / text/plain
  ├─ Json(Value)          → 标准 JSON-RPC 成功响应
  ├─ Error { code, msg }  → 带精确错误码的 JSON-RPC 错误
  ├─ Stream { receiver }  → 基于 channel 的流式输出（chat）
  │    ├─ "chunk"     → JSON-RPC notification chat.stream.chunk
  │    ├─ "done"      → JSON-RPC notification chat.stream.done
  │    ├─ "telemetry" → JSON-RPC notification chat.stream.telemetry
  │    ├─ "result"    → JSON-RPC result（最终响应）
  │    └─ "error"     → JSON-RPC error
  ├─ Text(String)        → 含 __text_plain__ sentinel 的 JSON-RPC
  ├─ Checkpoint(...)     → 自动分解为 checkpoint 成功/错误
  └─ Silent              → 无响应（JSON-RPC notification）
```

### 对话执行流水线（SSE）

```
GUI/CLI → POST /chat/stream → 后端
  │ observe_phase → think_phase → act_phase → reflect_phase
  │   ├─ emit_stream_chunk()     → SSE event: chunk
  │   ├─ emit_stream_sub_agent() → SSE event: sub_agent
  │   ├─ emit_stream_command()   → SSE event: command
  │   ├─ emit_stream_token_economy() → SSE event: telemetry
  │   └─ emit_stream_done()      → SSE event: done
  ▼
客户端 SSE 解析器 → PendingResponse → UI 面板
  ├─ StreamChunk   → 消息内容更新
  ├─ SubAgentEvent → 子代理面板（可折叠）
  ├─ CommandOutput → 命令面板（可折叠）
  └─ TokenEconomy  → Token 计数显示
```

### 核心能力模块

| 模块 | 说明 |
|:-----|:-----|
| **HarnessBus 治理总线** | 中央策略引擎：评估/验证/审核、PUA 规则、RBAC、漂移检测、超弹性、审计追踪 |
| **CapabilityBus 能力总线** | 多因子智能体选择（信誉+任务匹配+因果贝叶斯路由）|
| **CommunicationBus** | 层次化 Agent 树形通信、消息路由、取消传播、上下文继承（BLUE70）|
| **UnifiedKnowledgeBus** | 合并知识库+声誉+经验管理的统一知识总线，EMA 评分（BLUE70）|
| **ReinforcementBus** | Q-Learning + 可选联邦强化学习的路由优化（BLUE70）|
| **LearningOptimizationBus** | 原子化学习-优化：执行事件→优化建议（BLUE70）|
| **Planner 规划器** | 任务自适应的 DAG 规划，含依赖推断 |
| **BrainLoop 脑回路** | 规划→执行→反思→重规划的认知循环 |
| **DAG Driver 执行引擎** | 拓扑排序执行，并行组调度 |
| **SelfModelCore 自模型** | 系统自感知与能力追踪 |
| **MetacognitiveController 元认知** | 观察驱动的反思与纠偏 |
| **WorldModel 世界模型** | 实体/事件/关系追踪，含因果洞察 |
| **HyperResilience 超弹性** | 熔断器、故障切换组、自愈 |
| **MultiChannelTransport 多渠道传输** | QoS 感知、优先级消息传输 |

---

## 扩展

### VS Code 插件
`vscode-addon/` 目录包含 VS Code 扩展，可在编辑器内启动 go-on 并提供 60+ 命令 — 对话、工作流执行、技能管理。

```bash
cd vscode-addon
npm install
npm run compile
```

### SDK
- **Rust SDK**（`sdk/rust/`）— 强类型客户端，多领域方法覆盖
- **Python SDK**（`sdk/python/`）— 基于 HTTPX 的异步客户端，支持流式传输
- **TypeScript SDK**（`sdk/typescript/`）— 面向浏览器和 Node.js 环境的完整 TypeScript 客户端（同时被 `vscode-addon` 消费）

### Zed 编辑器集成
`.zed/settings.json` 将 go-on 预注册为 Zed 智能体服务器（`agent_servers.go-on`），启用自动批准，并配置 `auto_approve_tools` 用于常见只读操作（文件读取、目录列出、搜索）。

---

## 代码库统计

| 指标 | 数值 |
|:-----|:-----|
| Rust 后端代码行数 | ~213K（450 个模块）|
| GUI（EGUI）代码行数 | ~24K |
| VS Code 插件（TypeScript）代码行数 | ~17K |
| SDK（Rust + Python + TypeScript）代码行数 | ~6K |
| 内置工具数量 | ToolRegistry 注册 108 个（含特性门控）|
| AI 供应商数量 | 37 |
| 技能市场数量 | 35 |
| 单元测试数量 | `cargo test --lib` 实测 1659 通过（+ 集成套件，见下方验证状态）|
| 三语国际化覆盖 | en / zh-CN / zh-TW（约 95%）|

## 构建配置

| 配置 | 后端 | 适用场景 |
|:-----|:-----|:---------|
| `local` | SQLite + sqlite-vec | 单用户本地工具（默认） |
| `simple-server` | SQLite + sqlite-vec | 单服务器部署 |
| `multi-users-server` | PostgreSQL + pgvector | 多用户生产环境 |
| `full` | SQLite（全部特性）| CI / 开发 |

```bash
# 构建命令
cargo build                                                    # local（默认）
cargo build --no-default-features --features simple-server
cargo build --no-default-features --features multi-users-server
cargo build --no-default-features --features full
```

## 验证状态

| 配置 | `cargo clippy -D warnings` | 测试状态 |
|:-----|:--------------------------:|:--------:|
| `local` | ✅ **零警告** | ✅ **全部通过** |
| `simple-server` | ✅ **零警告** | ✅ **全部通过** |
| `multi-users-server` | ✅ **零警告** | ✅ **全部通过** |
| `full` | ✅ **零警告** | ✅ **全部通过** |

所有 4 种构建配置零 clippy 警告通过。最近一次完整 `cargo test --all-targets` 运行全部通过、零失败（最新计数见 `CHANGELOG.md` 最新一节）。GUI 和 VS Code 插件同样零错误编译通过。

统计表中的 lib 数字为**实测执行数**：`cargo test --lib` → **1659 通过 / 0 失败**。作为参考，`src/` 下 `#[test]` / `#[tokio::test...]` 属性声明数为 1737；`tests/` 下为 160（不含 `chaos-testing` 特性门控的 `chaos_drill` 套件 6 个）。实测数低于声明数，因部分声明位于默认（local）profile 未启用的特性门控之后。

用 `scripts/stats.sh` 刷新这些数字（加 `--check` 作为 CI 门禁：README 漂移时非零退出）。

---

## 许可证

MIT License — 详见 [LICENSE](LICENSE)。
