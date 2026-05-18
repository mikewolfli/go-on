# go-on

简体中文 | [English](README.md)

go-on 是一个基于 Rust 的 **ACP/MCP 智能体编排、治理与生产安全运行时**，
支持全链路多语言国际化，采用模块化多总线架构，涵盖 14 条能力子总线和 21+ 个 F-GAP 模块。

## 版本

- 后端 Runtime：**0.9.5**
- GUI 桌面端：**0.9.5**
- VS Code 插件：**0.9.5**
- 默认特性：`profile-local`
- 可选特性：`profile-simple-server`、`profile-multi-users-server`

## GUI 桌面应用

基于 EGUI 的桌面图形界面（`gui/`）提供监控、对话和配置管理：

```bash
cargo run --manifest-path gui/Cargo.toml
```

### 截图预览

| 监控面板 | 对话界面 |
|:---:|:---:|
| ![监控面板](snapshots/monitor.png) | ![对话界面](snapshots/chat.png) |

| 供应商管理 | 设置 |
|:---:|:---:|
| ![供应商管理](snapshots/providers.png) | ![设置](snapshots/settings.png) |

| 技能管理 |
|:---:|
| ![技能管理](snapshots/skills.png) |

### 主要功能
- **监控面板**：后端健康状态、AI 供应商状态、实时指标
- **对话界面**：多会话管理、阶段选择（coding/review/debug/test/deploy）、模式切换（Ask/Plan/Edit/Safeguard/Full Auto）、文件附件、动态发送按钮（依据 AI 状态变化）；自动消息修剪（每会话最多 1000 条）
- **技能管理**：创建和导入 AI 技能；内置 `skill-creator` 让 AI 自主定义新技能
- **设置**：功能开关、语言切换（en/zh-CN/zh-TW）、5 种视觉主题；供应商管理（35 个供应商）
- **风险决策与安全审查模式 (Safeguard)**：AI 驱动的风险内容评估。当后端检测到高风险话题（医疗、法律、金融、安全等）时，在对话界面中显示**风险决策面板**，展示风险评分、应对策略（多模型投票、多智能体投票、升级处理）、审查要求及具体原因，实现对敏感 AI 交互的人工知情监督。
- **后端连接**：ACP+HTTP JSON-RPC，自动健康轮询

## 构建配置文件

三种构建配置文件适配不同的部署场景：

| 配置文件 | 后端 | 目标 | 构建命令 |
|---------|------|------|----------|
| `profile-local` | SQLite + sqlite-vec | 单用户本地工具 | `cargo build`（默认） |
| `profile-simple-server` | SQLite + sqlite-vec | 单服务部署 | `cargo build --no-default-features -F profile-simple-server` |
| `profile-multi-users-server` | PostgreSQL + pgvector | 多用户生产环境 | `cargo build --no-default-features -F profile-multi-users-server` |

### 多用户服务器特性

`profile-multi-users-server` 在 ACP+HTTP 和 MCP+HTTP 双传输路径上增加了完整的多用户安全功能：

| 特性 | 说明 |
|------|------|
| **CORS 支持** | 可配置允许的来源、预检请求 (OPTIONS) 处理、所有 HTTP/SSE 响应均含 CORS 头部 |
| **入口认证（网关）** | 通过 `Authorization: Bearer`、`X-Api-Key` 或 `X-Go-On-Key` 头部验证共享 API 密钥 |
| **用户认证（HMAC 令牌）** | 基于 HMAC-SHA256 签名的 JWT 风格令牌，支持自动配置、可配置 TTL |
| **RBAC 授权** | 基于角色的访问控制（admin/user/viewer/monitor），支持按端点权限检查 |
| **租户预算控制** | 每个租户的每日令牌/并发任务/API 调用配额，支持自动配置 |
| **对话隔离** | 使用租户前缀命名对话 ID，防止跨用户数据泄漏 |
| **关闭排空** | 优雅关闭期间可配置的排空时间（秒），等待正在处理的连接完成 |
| **信号处理** | 全平台支持 SIGINT（Ctrl+C）和 SIGTERM 信号实现干净关闭 |
| **热重载配置** | 通过 `config.reload` RPC 在运行时重新加载配置（agent/cache/vector 变更需重启） |

**密钥管理（线程安全）：**
- `SECRET_OVERRIDE_MAP` — 内存 `HashMap` 替代 `std::env::set_var()`（多线程环境下为未定义行为）
- `KEYRING_CACHE` — 30 秒 TTL 密钥环缓存，避免异步热路径中的阻塞 I/O
- `crate::shared::secret_override::get_secret(key)` — 先检查内存覆写映射，再回退到 `std::env::var()`

**安全合规：**
- ✅ 生产代码中 0 个 `std::env::set_var()` 调用
- ✅ 0 个 `.expect("lock poisoned")` 恐慌 — 所有锁中毒通过 `unwrap_or_else(|e| e.into_inner())` 恢复
- ✅ 生产代码中 0 个 `unsafe` 块
- ✅ 所有 HTTP 响应（JSON、SSE、错误）均包含 CORS 头部
- ✅ MCP HTTP 服务器与 ACP HTTP 服务器共享相同的认证流水线

## 验证状态（Phase 4+ — 50+ 轮深度扫描完成）

| 配置文件 | `cargo check` | `cargo clippy -D warnings` | `cargo test` |
|---------|:-----------:|:------------------------:|:----------:|
| **profile-local** | ✅ 0 errors, 0 warnings | ✅ 0 errors | ✅ **991 通过** |
| **profile-simple-server** | ✅ 0 errors, 0 warnings | ✅ 0 errors | ✅ **991 通过** |
| **profile-multi-users-server** | ✅ 0 errors, 0 warnings | ✅ 0 errors | ✅ **991 通过** |

### CI 门禁状态（GitHub Actions）
| 步骤 | 结果 |
|------|------|
| `cargo check --all-targets` | ✅ 0 errors, 0 warnings |
| `cargo clippy -D warnings`（3 个配置文件） | ✅ 全部 0 errors |
| 单元测试（816） | ✅ 全部通过 |
| 集成测试（175） | ✅ 全部通过 |
| GUI EGUI 编译 + 测试 | ✅ 0 errors |
| VS Code 插件编译 + 代码检查 + 合约测试 | ✅ 0 errors |

跨平台（Windows、Linux、macOS）：
- 所有平台特定代码均有对应的回退实现（Unix `AsRawFd` ✅ Windows stub ✅）
- 内存读取：Linux `/proc/self/status` ✅ Windows `windows-sys` ✅ Fallback 返回 0 ✅
- ANSI 颜色代码在 Windows 上禁用（`#[cfg(not(target_os = "windows"))]`）✅
- 信号处理：Unix 上支持 SIGTERM，全平台支持 SIGINT（Ctrl+C）✅
- GUI 路径使用 `directories` 库获取平台合适的数据目录 ✅
- vscode-addon：已设置 `activationEvents`，`.exe`/`.bat` 平台感知默认路径 ✅

### 线程安全与生产加固
- 所有 `Mutex::lock().unwrap()` 已替换为中毒恢复模式 ✅
- 所有 `std::env::set_var()` 已替换为线程安全的 `set_secret_override()` ✅
- 生产代码中 0 个 `unsafe` 块 ✅
- 生产代码中 0 个 `panic!()` / `todo!()` / `unimplemented!()` ✅
- 生产代码中 0 个 `unwrap()` / `.expect()` ✅
- 37 个 `.expect("lock poisoned")` 调用已替换为中毒恢复 ✅
- 所有内部通道已绑定（`mpsc::sync_channel`）— 无内存无限制增长 ✅
- 对话会话上限 1000 条消息 — 自动淘汰最旧消息 ✅
- 后端：崩溃后自动重启（指数退避 3→96 秒）✅
- 可选规则文件降级为 DEBUG 级别 — 无日志噪音 ✅
- 3 个构建配置文件（local/simple-server/multi-users-server）均为 0 warnings ✅

## 仓库结构

### 核心目录
- `src/` — 后端 Runtime 主实现（Rust）
  - `src/acp/` — ACP 服务、请求路由、workflow/task/chat/checkpoint 主链
    - `src/acp/impl/cors.rs` — CORS 支持模块（可配置来源、预检请求、响应头部）
    - `src/acp/impl/session.rs` — 用户会话管理（HMAC-SHA256 令牌、RBAC 集成、租户隔离）
  - `src/agents/` — 模型供应商适配器（OpenAI、Anthropic、DeepSeek、Ollama 等）与统一合约
    - `src/agents/factory/` — AgentFactory，特性门控的配置文件选择
  - `src/core/` — 配置、初始化、就绪性检查、错误模型
  - `src/governance/` — 策略/规则治理、审计、安全治理器、漂移防护
    - `src/governance/drift/` — 漂移防护引擎（F-GAP-26）
    - `src/governance/harness_bus.rs` — HarnessBus 治理入口
  - `src/intelligence/` — 选择器、强化学习、质量模型、能力总线、发现、共识
    - `src/intelligence/capability_bus/` — **14 条子总线**（核心 + 工具 + 可观测 + 优化 + 内存 + 协议 + 编排 + 分布式内存）
    - `src/intelligence/discovery.rs` — DiscoveryCenter 方案发现中心（F-GAP-11）
    - `src/intelligence/matcher.rs` — ScenarioMatcher 场景匹配器（F-GAP-12）
    - `src/intelligence/evolution_graph.rs` — EvolutionGraph 演化图谱（F-GAP-18）
    - `src/intelligence/metacognitive.rs` — MetacognitiveController 元认知控制器（F-GAP-22）
    - `src/intelligence/self_model.rs` — SelfModelCore 自模型核心（F-GAP-21）
    - `src/intelligence/consensus.rs` — ConsensusEngine 共识引擎（F-GAP-16）
    - `src/intelligence/consciousness.rs` — ConsciousnessMetrics 意识代理指标（F-GAP-25）
    - `src/intelligence/world_model.rs` — WorldModel 世界模型流水线（F-GAP-23）
    - `src/intelligence/continuous_learning.rs` — ContinuousLearningCenter 持续学习中心（F-GAP-24）
  - `src/orchestration/` — 流程、模式、路由、编排，脑回路，全能模式
    - `src/orchestration/loop/brain_loop.rs` — Brain Loop 脑回路引擎（F-GAP-17）
    - `src/orchestration/council/` — OrchestrationCouncil 编排委员会（F-GAP-15）
    - `src/orchestration/omnipotent.rs` — OmnipotentMode 全能模式（F-GAP-09）
    - `src/orchestration/artifact.rs` — ArtifactLayer 制品合约层（F-GAP-10）
    - `src/orchestration/skill_import.rs` — RemoteSkill 远程技能（F-GAP-10 配套）
    - `src/orchestration/scheduler.rs` — TaskScheduler 任务调度器（ARCH-02）
    - `src/orchestration/task_graph_store.rs` — TaskGraphStore 任务图持久化（F-GAP-03）
  - `src/fault_tolerance.rs` — FaultToleranceEngine 跨节点容错引擎（F-GAP-28），故障隔离与自动恢复
  - `src/resilience/` — HyperResilienceEngine 超弹性引擎（F-GAP-27）
    - `src/resilience/hyper_resilience.rs` — 熔断器、故障切换、自愈
  - `src/i18n/` — 多语言运行时（后端模块 ~95% i18n 覆盖）
  - `src/mcp/` — MCP 适配辅助
  - `src/memory/` — 缓存与向量存储抽象
  - `src/observability/` — 指标/追踪/性能观测
  - `src/optimization/` — 成本/速度/可靠性优化
  - `src/protocol/` — 协议服务、JSON-RPC 支撑、多渠道消息传输
    - `src/protocol/mcp_server.rs` — MCP stdio + HTTP 服务器，完整 CORS/认证/RBAC 集成
  - `src/shared/` — 共享类型、协议模式、工具描述符、线程安全密钥管理
    - `src/shared/secret_override.rs` — `SECRET_OVERRIDE_MAP` + `KEYRING_CACHE`（线程安全 `set_var` 替代方案）
- `gui/` — EGUI（Rust 原生）桌面图形界面
- `vscode-addon/` — VS Code 扩展（支持 en_US、zh_CN、zh_TW 多语言）

### 配置与脚本
- `config/` — 配置文件（`config.toml`、`config.production.toml`）
  Provider 规格说明硬编码在 `src/core/config.rs` 和 `src/core/setup.rs` 的 `built_in_provider_specs()` 函数中。
- `scripts/` — 质量门禁、发布门禁脚本与部署工具
  - `scripts/deploy/nginx/` — 入口网关与 TLS 反向代理模板

### 文档
- `docs/` — 完整项目文档
  - `docs/blueprints/` — 蓝图文档（blue1.md 到 blue38.md、FAULT1）
  - `docs/design/` — 设计文档（FUTURE1-6、future-last）
  - `docs/guides/` — 实施指南（MCP、PUA、迁移、GUI、模型选择）
  - `docs/reports/` — 项目评估与代码审查报告
- `DOC/` — 书籍格式的项目文档

### 测试与开发
- `tests/` — 集成测试与测试产物
  - `tests/artifacts/` — 测试产物与基准测试结果
  - `tests/requests/` — NDJSON 场景基准与回放输入
  - 集成测试：ACP RPC、协议一致性、传输对等性、OpenAI 兼容矩阵、PUA 合约冒烟
- `test_i18n/` — 国际化测试套件

### 资源
- `languages/` — 运行时多语言资源（en_US、zh_CN、zh_TW — 各 448+ 键值）
  - `languages/rules/` — PUA 编码规则
- `RULES/` — 治理与编码规则集
- `contracts/` — 编辑器能力矩阵与契约

## 协议模式

`[protocol].mode` 支持 5 种：

- `adaptive`（推荐默认）— 双栈协议，按请求类型路由
- `acp_stdio` — ACP 基于 stdio
- `acp_http` — ACP 基于 HTTP
- `mcp_stdio` — MCP 基于 stdio
- `mcp_http` — MCP 基于 HTTP

示例：

```toml
[protocol]
mode = "adaptive"
```

## 架构：多总线能力系统

go-on 实现了以 **CapabilityBus** 和 **HarnessBus** 为核心的 **14 条总线架构**：

### 核心总线（Phase 0-3）
| 总线 | 模块 | 说明 |
|:----|------|------|
| **CapabilityBus** | `capability_bus/core.rs` | 中央智能总线，编排 sense/decide/evolve 生命周期 |
| **HarnessBus** | `governance/harness_bus.rs` | 治理入口，策略评估、漂移/弹性/安全检查 |

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

### F-GAP 模块（Phase 4 — 21/21 全部完成 ✅）

| F-GAP | 模块 | 位置 | 状态 |
|:-----:|------|------|:----:|
| 09 | OmnipotentMode 全能模式 | `orchestration/omnipotent.rs` | ✅ 20 测试 |
| 10 | ArtifactLayer 制品层 + RemoteSkill | `orchestration/artifact.rs`, `orchestration/skill_import.rs` | ✅ 13 测试 |
| 11 | DiscoveryCenter 方案发现中心 | `intelligence/discovery.rs` | ✅ 11 测试 |
| 12 | ScenarioMatcher 场景匹配器 | `intelligence/matcher.rs` | ✅ 9 测试 |
| 13 | AgentFactory 智能体工厂 | `agents/factory/` | ✅ 特性门控 |
| 14 | SecurityGovernor 安全治理器 | `governance/security_governor.rs` | ✅ |
| 15 | OrchestrationCouncil 编排委员会 | `orchestration/council/` | ✅ 22 测试 |
| 16 | ConsensusEngine 共识引擎 | `intelligence/consensus.rs` | ✅ 20 测试 |
| 17 | BrainLoop 脑回路 | `orchestration/loop/brain_loop.rs` | ✅ 32 测试 |
| 18 | EvolutionGraph 演化图谱 | `intelligence/evolution_graph.rs` | ✅ 12 测试 |
| 19 | FederatedRL 联邦强化学习 | `intelligence/reinforcement/federated.rs` | ✅ 27 测试 |
| 20 | DistributedMemory 分布式记忆（网络层） | `capability_bus/distributed_memory_bus.rs` | ✅ 增强 |
| 21 | SelfModelCore 自模型核心 | `intelligence/self_model.rs` | ✅ 12 测试 |
| 22 | MetacognitiveController 元认知控制器 | `intelligence/metacognitive.rs` | ✅ 12 测试 |
| 23 | WorldModel 世界模型 | `intelligence/world_model.rs` | ✅ |
| 24 | ContinuousLearning 持续学习中心 | `intelligence/continuous_learning.rs` | ✅ |
| 25 | ConsciousnessMetrics 意识代理指标 | `intelligence/consciousness.rs` | ✅ 12 测试 |
| 26 | DriftProtection 漂移防护 | `governance/drift/drift_protection.rs` | ✅ 12 测试 |
| 27 | HyperResilience 超弹性 | `resilience/hyper_resilience.rs` | ✅ |
| 28 | FaultTolerance 跨节点容错 | `fault_tolerance.rs` | ✅ 20 测试（含 E2E、500 节点压力） |
| 29 | MultiChannelTransport 多渠道消息传输 | `protocol/transport.rs` | ✅ 37 测试（QoS、去重、Peek） |

### 38 维度满星评级

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
───────────────────────────────────────────────────────────────────────────────────
总计 (38/38):        100% ★★★★★
```

### 整体完成率

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
| 英语（美国） | `languages/en_US.json` | 448+ |
| 简体中文 | `languages/zh_CN.json` | 448+ |
| 繁体中文 | `languages/zh_TW.json` | 448+ |

覆盖层：
- **ACP/MCP HTTP 错误响应** — 100%
- **Agent 供应商模块**（OpenAI、Anthropic、DeepSeek、Ollama 等）— 100%
- **配置验证**（约 50 条字符串）— 100%
- **CLI 初始化消息** — 100%
- **API 处理错误** — 100%
- **编排层**（工具、技能、脑回路）— 100%
- **GUI（Vue/TypeScript）** — 约 98%
- **VS Code 插件** — 3 种语言 70+ MessageKeys

## 快速开始

### 1）构建

```bash
cargo build
```

### 2）首次初始化（自动检测）

直接运行 `go-on`——若未找到配置或 AI 供应商，将交互式提示：

```bash
# 使用默认配置路径运行 (~/.config/go-on/config.toml)
cargo run
```

对于非交互式环境（GUI、CI），后端会自动创建引导配置并启动。

手动初始化：

```bash
cargo run -- --init
cargo run -- --init --setup-level quick
cargo run -- --init --setup-level custom
```

### 3）验证配置

```bash
cargo run -- --check
```

### 4）启动 Runtime

- Linux/macOS：`./scripts/start-go-on.sh`
- Windows：`scripts/start-go-on.bat`

或手动指定协议模式：

```bash
# ACP over HTTP（默认健康检查 http://127.0.0.1:8090）
cargo run -- --mode acp_http --bind 127.0.0.1:8090

# MCP over stdio（用于 Claude Code / Codex 集成）
cargo run -- --mode mcp_stdio
```

### 5）终端聊天模式（类似 Claude Code / Codex）

```bash
# 启动交互式终端聊天（使用当前目录的 config.toml）
go-on -a
# 或
go-on --chat
```

如果配置文件在其他路径：

```bash
go-on -c /path/to/config.toml -a
```

AI 智能体自动从系统 keyring 读取 API 密钥。如果未配置任何供应商，将提示先运行初始化向导。

默认健康检查地址：

- `http://127.0.0.1:8090/health`

## 生产基线

生产模板：

- `config/config.production.toml`

当前默认包含：

- 默认环回地址监听
- 入口鉴权与入口限流配置
- 严格模式 fail-fast（`runtime.production_strict = true`）
- OTEL 相关运行时配置

### API 密钥配置（生产模式）

使用 `config/config.production.toml` 启动时，入口鉴权默认已开启。
启动前必须设置以下环境变量：

```bash
# Linux / macOS
export GO_ON_ENTRY_API_KEY="your-secret-key-here"
./scripts/start-go-on.sh
```

```powershell
# Windows PowerShell
$env:GO_ON_ENTRY_API_KEY = "your-secret-key-here"
scripts\start-go-on.bat
```

所有 RPC 请求需在 `Authorization` 头中携带密钥：

```
Authorization: Bearer your-secret-key-here
```

若该变量缺失或为空，服务将以错误码 `-32003`（`AuthRequired`）拒绝所有请求。

> **安全提示**：绝不要将密钥写入任何配置文件或提交到版本控制。请使用环境变量、密钥管理器或 keyring 注入方式。

入口网关与 TLS 模板：

- `scripts/deploy/nginx/go-on.conf`
- `scripts/deploy/nginx/README.md`

发布就绪清单：

- `docs/RELEASE_READINESS.md`

## 场景与门禁工具

`tests/requests/` 已包含 runtime health、governance、cost、harness、security、release drill 等主链场景。

门禁脚本（位于 `scripts/`）：

- `scripts/run-quality-gate.sh`
- `scripts/run-quality-gate.ps1`
- `scripts/run-release-readiness-gate.sh`
- `scripts/run-release-readiness-gate.ps1`
- `scripts/test_ci.sh`

## 三端协同组件

- GUI 桌面控制台文档：`gui/README.md`
- VS Code 插件文档：`vscode-addon/README.md`

二者已对齐后端 RPC 能力、治理状态与健康探针语义。

## 通用 RPC 能力分组

当前主链代表方法：

- **核心运行**：`initialize`、`shutdown`、`runtime.health`、`runtime.stability`、`config.reload`
- **安全治理**：`governance.status`、`governance.plan.get`、`governance.plan.update`、`governance.audit.recent`、`security.baseline`
- **可观测**：`metrics.get`、`metrics.prometheus`、`trace.get`、`trace.metrics`、`observability.alerts`、`health.probes`
- **稳定性**：`breaker.status`、`breaker.reset`、`breaker.recovery`、`maintenance.gc`
- **工作流任务**：`workflow.execute`、`task.plan`、`task.execute`
- **学习与智能**：`learning.summary`、`learning.replay`、`learning.guardrail`、`selector.status`、`knowledge.distill`、`rl.alignment.offline_eval`、`hardness.status`
- **优化与治理**：`cost.status`、`config.baseline`、`error.contract`、`build.repro`、`data.lifecycle`、`harness.status`、`optimization.peak`、`quality.baseline`

## 相关文档

完整文档已组织在 `docs/` 目录中：

### 蓝图文档（`docs/blueprints/`）
- `blue1.md` 到 `blue38.md` — 实施蓝图与进展总账
- `FAULT1.MD` — 容错蓝图
- `server-blue1.md` — 服务器架构蓝图

### 设计文档（`docs/design/`）
- `design.md` — 系统设计概述
- `FUTURE.md` 到 `FUTURE6.md` — 未来规划文档
- `future-last.md` — 完整的未来改进计划

### 指南文档（`docs/guides/`）
- `README-PUA-UNIVERSAL.md` — PUA 通用实施指南
- `MCP_LAYER.md` — MCP 层实施详情
- `GO-ON_PUA_IMPLEMENTATION.md` — PUA 实施细节
- `IMPLEMENTATION_STATUS.md` — 当前实施状态
- `MIGRATION_STATUS.md` — 迁移状态与计划
- `PHASE_10_COMPLETE_IMPLEMENTATION.md` — 第 10 阶段实施详情
- `ENHANCEMENT_OPPORTUNITIES.md` — 改进机会（中/英）
- `MODEL_SELECTION.md` — 模型选择指南
- `GUI_FIRST_RUN.md` — GUI 首次运行指南

### 报告文档（`docs/reports/`）
- `PROJECT_EVALUATION_REPORT.md` — 完整的项目评估报告
- `CODE_REVIEW_FINAL_REPORT.md` — 代码审查发现
- `PHASE_10_DELIVERY_REPORT.md` — 第 10 阶段交付报告
- `MIGRATION_FINAL_SUMMARY.md` — 迁移总结

### 其他关键文档
- `docs/RELEASE_READINESS.md` — 发布就绪清单
- `docs/RULES.md` — 项目规则与指南
- `docs/DEVELOPMENT_RULES.md` — 开发规则与标准
- `docs/CLAUDE.md` — Claude.ai 集成指南
- `docs/SAFEGUARD_MODE.md` — 安全防护模式文档

## 许可证

本项目按 MIT 或 BSD 许可（可任选其一）。
