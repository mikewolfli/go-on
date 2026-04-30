# 架构总览

`go-on` 是一个围绕 Rust 后端构建的三端运行时体系：

- **后端**：负责配置加载、Provider 选择、路由、setup、健康检查、协议协商、stdio 或 HTTP 传输层，以及包含 14 条总线和 21 个 F-GAP 模块的能力架构。
- **GUI**：Tauri 桌面控制台，负责后端发现、进程生命周期、集成探测和本地运维。
- **VS Code 插件**：负责拉起或探测运行时，暴露基于 RPC 的命令，并可在工作区级别覆盖协议模式。

## 版本

- 后端 Runtime：**0.8.3**
- GUI 桌面端：**0.8.3**
- VS Code 插件：**0.8.3**

## 构建配置文件

三种构建配置文件适配不同的部署场景：

| 配置文件 | 后端 | 目标 | 构建命令 |
|---------|------|------|----------|
| `profile-local` | SQLite + sqlite-vec | 单用户本地工具 | `cargo build`（默认） |
| `profile-simple-server` | SQLite + sqlite-vec | 单服务部署 | `cargo build --no-default-features -F profile-simple-server` |
| `profile-multi-users-server` | PostgreSQL + pgvector | 多用户生产环境 | `cargo build --no-default-features -F profile-multi-users-server` |

## 验证状态（Phase 4 完成）

| 配置文件 | `cargo check` | `cargo clippy -D warnings` | `cargo test` |
|---------|:-----------:|:------------------------:|:----------:|
| **profile-local** | ✅ 0 errors, 0 warnings | ✅ 0 errors | ✅ **866 通过**（766 单元 + 86 RPC + 14 transport） |
| **profile-simple-server** | ✅ 0 errors, 0 warnings | ✅ 0 errors | ✅ **905 通过** |
| **profile-multi-users-server** | ✅ 0 errors, 0 warnings | ✅ 0 errors | ✅ **898 通过** |

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

go-on 实现了以 **CapabilityBus** 和 **HarnessBus** 为核心的 **14 条总线架构**。

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

## F-GAP 模块（Phase 4 — 21/21 全部完成 ✅）

go-on 实现了 21 个 FutureDesign 模块，分布在六个能力领域：

### 编排与执行（F-GAP-09, 10, 15, 17）
- **OmnipotentMode 全能模式**（F-GAP-09）：EscalationToken 颁发/验证/吊销、RAII 会话守卫、审计日志
- **ArtifactLayer 制品层**（F-GAP-10）：制品模式注册、存储、TTL 裁剪
- **RemoteSkill 远程技能**（F-GAP-10）：远程 MCP 端点包装为 Skill trait
- **OrchestrationCouncil 编排委员会**（F-GAP-15）：多 Agent 协调委员会
- **BrainLoop 脑回路**（F-GAP-17）：Plan→Execute→Reflect→Replan 全循环

### 智能与学习（F-GAP-11, 12, 16, 18, 19, 21, 22, 23, 24, 25）
- **DiscoveryCenter 方案发现中心**（F-GAP-11）：解决方案模式注册与搜索
- **ScenarioMatcher 场景匹配器**（F-GAP-12）：多维度场景匹配
- **ConsensusEngine 共识引擎**（F-GAP-16）：分布式投票与共识
- **EvolutionGraph 演化图谱**（F-GAP-18）：6 阶段能力演化生命周期
- **FederatedRL 联邦强化学习**（F-GAP-19）：FedAvg/FedWeighted/FedMedian 聚合
- **SelfModelCore 自模型核心**（F-GAP-21）：自我能力评估与置信度
- **MetacognitiveController 元认知控制器**（F-GAP-22）：6 阶段思维链、卡顿检测
- **WorldModel 世界模型**（F-GAP-23）：世界模型流水线
- **ContinuousLearningCenter 持续学习中心**（F-GAP-24）：持续学习编排
- **ConsciousnessMetrics 意识代理指标**（F-GAP-25）：5 维度意识度量

### 治理与安全（F-GAP-14, 26）
- **SecurityGovernor 安全治理器**（F-GAP-14）：安全策略治理
- **DriftProtection 漂移防护**（F-GAP-26）：5 种漂移类型、4 级严重度、趋势检测

### 弹性与容错（F-GAP-27, 28）
- **HyperResilienceEngine 超弹性引擎**（F-GAP-27）：熔断器、故障切换、自愈
- **FaultToleranceEngine 跨节点容错引擎**（F-GAP-28）：节点心跳、隔离、自动恢复、集群健康评分

### 协议与传输（F-GAP-29）
- **MultiChannelTransport 多渠道消息传输**（F-GAP-29）：6 通道、4 级优先级、QoS、去重、Peek

### Agent 基础设施（F-GAP-13）
- **AgentFactory Agent 工厂**（F-GAP-13）：特性门控的 Agent 实例化

## 38 维度满星评级

```
治理与合规 (5/5):    ★★★★★ 溯源账本, 漂移防护, 策略评估器, Token 门控链, 安全治理器
弹性与容错 (2/2):    ★★★★★ 超弹性引擎, 跨节点容错引擎
编排与执行 (6/6):    ★★★★★ 编排总线, 任务调度器, 执行图, 全能模式, 制品层, 脑回路
路由与调度 (7/7):    ★★★★★ 能力图谱, 信誉存储, Q学习Agent, 场景匹配器, 发现中心, 工作流注册表, Agent工厂
协议与传输 (2/2):    ★★★★★ 协议总线, 多渠道消息传输
记忆与缓存 (2/2):    ★★★★★ 内存总线, 分布式内存总线
观测与优化 (3/3):    ★★★★★ 可观测总线, 优化总线, 工具总线
智能认知 (5/5):      ★★★★★ 深度知识萃取, 强化深度学习, 技能保持传承, AI进化, 自建Skills
自我认知 (5/5):      ★★★★★ 自模型核心, 意识代理指标, 元认知控制器, 世界模型, 共识引擎
总计 (38/38):        100% ★★★★★
```

## 整体完成率

```
Phase 0: 核心双总线           ████████████████████ 100%
Phase 1: 子总线接入            ████████████████████ 100%
Phase 2: 剩余修复              ████████████████████ 100%
Phase 3: ARCH 扩展点           ████████████████████ 100%
Phase 4: FutureDesign (F-GAP)  ████████████████████ 100% (21/21)
Phase 5: 生产硬化              ████████████████████ 100%
────────────────────────────────────────────────────────
总体:                         ████████████████████ 100%
```

## 国际化（i18n）

go-on 在后端实现了约 **95%** 的全链路国际化覆盖：

| 语言 | 文件 | 键值数 |
|:-----|:-----|:------:|
| 英语（美国） | `languages/en_US.json` | 372+ |
| 简体中文 | `languages/zh_CN.json` | 372+ |
| 繁体中文 | `languages/zh_TW.json` | 372+ |

覆盖层：ACP/MCP HTTP 错误（100%）、Agent 供应商模块（100%）、配置验证（100%）、CLI 初始化（100%）、API 处理错误（100%）、编排层（100%）、GUI（约 98%）、VS Code 插件（70+ 键值）。

## 与架构对应的仓库目录

- `src/`：后端运行时、CLI、setup、ACP 与 MCP 实现。
  - `src/acp/`：ACP 服务、请求路由、workflow/task/chat/checkpoint
  - `src/agents/`：Provider 适配器（OpenAI、Anthropic、DeepSeek、Ollama），AgentFactory
  - `src/core/`：配置、初始化、就绪性检查、错误模型
  - `src/governance/`：策略/规则治理、审计、安全治理器、漂移防护
  - `src/intelligence/`：选择器、强化学习、能力总线、发现、共识、演化
  - `src/orchestration/`：流程/模式/路由编排、脑回路、全能模式、制品层
  - `src/fault_tolerance.rs`：跨节点容错引擎
  - `src/resilience/`：超弹性引擎
  - `src/protocol/`：协议服务、JSON-RPC、多渠道消息传输
  - `src/i18n/`：语言运行时
- `GUI/`：Tauri 桌面控制台
- `vscode-addon/`：VS Code 插件（支持 en_US、zh_CN、zh_TW 多语言）
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