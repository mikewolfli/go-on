# go-on

简体中文 | [English](README.md)

**go-on** 是一个基于 Rust 的 **ACP/MCP 智能体编排、治理与生产安全运行时** — 一站式 AI 智能体运行时。

- 🖥️ **桌面 GUI** — 监控、对话、技能与工具管理
- 🧠 **14 条总线智能核心** — CapabilityBus + HarnessBus 全链路闭环治理
- 🌐 **全链路国际化** — 简体中文、繁体中文、英文（各 448+ 键值）
- 🛡️ **安全审查模式 (Safeguard)** — AI 驱动的风险内容评估
- 🔌 **多协议支持** — ACP + MCP，stdio + HTTP，单用户到多用户集群
- 🤖 **35+ AI 供应商** — OpenAI、Anthropic、DeepSeek、Gemini、Groq、Ollama 等

---

## GUI 桌面应用

基于 EGUI 的桌面图形界面（`gui/`）提供实时监控、多会话对话、技能管理和可视化配置 — 无需终端。

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
- **对话界面**：多会话管理、阶段/模式选择、文件附件、多模型支持、自动消息修剪（每会话最多 1000 条）、动态发送按钮
- **技能管理**：创建和管理 AI 技能，内置 `skill-creator`
- **设置**：供应商管理（35 供应商）、配置编辑器、6 种主题、语言切换（en/zh-CN/zh-TW）
- **风险决策面板**：当后端检测到高风险话题（医疗、法律、金融、安全等）时，展示风险评分、应对策略（多模型投票、多智能体投票、升级处理）、审查要求及具体原因
- **密钥管理**：系统 keyring + 配置文件双重存储
- **自动重启**：后端崩溃后自动重启（指数退避 3→96 秒）

---

## 架构：多总线能力系统

go-on 实现了以 **CapabilityBus** 和 **HarnessBus** 为核心的 **14 条总线架构**：

### 核心总线
| 总线 | 说明 |
|:----|------|
| **CapabilityBus** | 中央智能总线，编排 sense → decide → act → feedback → evolve 生命周期 |
| **HarnessBus** | 治理入口，策略评估、漂移/弹性/安全检查 |

### 子总线
| 总线 | 说明 |
|:----|------|
| **ToolBus** | 统一工具/Skill 调用，能力矩阵，Agent-工具匹配 |
| **ObservabilityBus** | 延迟、错误率、Agent 健康 |
| **OptimizationBus** | 成本/速度/可靠性推荐，熔断器 |
| **MemoryBus** | 级联缓存（L1 内存 → L2 SQLite → L3 向量存储） |
| **ProtocolBus** | 协议感知路由，健康/延迟追踪 |
| **OrchestrationBus** | 流程/模式/路由编排，模式推荐 |
| **DistributedMemoryBus** | 跨节点记忆共享（多用户配置） |

### F-GAP 认知模块（21/21 全部完成 ✅）

| 模块 | 说明 |
|:-----|------|
| OmnipotentMode 全能模式 | 自愈任务执行 |
| BrainLoop 脑回路 | 计划→执行→反思→重计划 |
| ConsensusEngine 共识引擎 | 多智能体投票治理 |
| SelfModelCore 自模型核心 | 系统自我认知与能力追踪 |
| ConsciousnessMetrics 意识代理指标 | 智能体意识状态机 |
| MetacognitiveController 元认知控制器 | 观察驱动的反思与行动 |
| WorldModel 世界模型 | 实体/事件/关系流水线 |
| DiscoveryCenter 方案发现中心 | 跨会话模式挖掘 |
| EvolutionGraph 演化图谱 | 能力生命周期与趋势追踪 |
| FederatedRL 联邦强化学习 | 分布式强化学习 |
| DriftProtection 漂移防护 | 目标/能力/行为漂移检测 |
| HyperResilience 超弹性 | 熔断器、故障切换、自愈 |
| FaultTolerance 跨节点容错 | 故障隔离与自动恢复 |
| MultiChannelTransport 多渠道传输 | QoS、去重、消息探测 |

### 38 维度满星评级

```
治理与合规 (5/5):    ★★★★★ 溯源, 漂移防护, 策略评估, Token门控, 安全治理
弹性与容错 (2/2):    ★★★★★ 超弹性, 跨节点容错
编排与执行 (6/6):    ★★★★★ 编排总线, 调度器, 执行图, 全能模式, 制品层, 脑回路
路由与调度 (7/7):    ★★★★★ 能力图谱, 信誉, Q学习, 场景匹配, 发现中心, 工作流注册表, Agent工厂
协议与传输 (2/2):    ★★★★★ 协议总线, 多渠道传输
记忆与缓存 (2/2):    ★★★★★ 内存总线, 分布式内存总线
观测与优化 (3/3):    ★★★★★ 可观测总线, 优化总线, 工具总线
智能认知 (5/5):      ★★★★★ 知识萃取, 深度RL, 技能传承, AI进化, 自建Skills
自我认知 (5/5):      ★★★★★ 自模型, 意识指标, 元认知, 世界模型, 共识
───────────────────────────────────────────────────────────────────────────────────
总计 (38/38):        100% ★★★★★
```

---

## 协议模式

5 种模式适配任意集成场景：

| 模式 | 说明 |
|:-----|------|
| `adaptive`（默认） | 双栈协议，按请求类型路由 |
| `acp_stdio` / `acp_http` | ACP over stdio / HTTP |
| `mcp_stdio` / `mcp_http` | MCP over stdio / HTTP |

配置示例：
```toml
[protocol]
mode = "adaptive"
```

---

## 国际化（i18n）

后端全链路约 **95%** 国际化覆盖：

| 语言 | 文件 | 键值数 |
|:-----|:-----|:------:|
| 简体中文 | `languages/zh_CN.json` | 448+ |
| 繁体中文 | `languages/zh_TW.json` | 448+ |
| 英语（美国） | `languages/en_US.json` | 448+ |

覆盖：ACP/MCP HTTP 错误 ✅、Agent 供应商模块 ✅、配置验证 ✅、CLI 初始化 ✅、API 错误 ✅、编排层 ✅

---

## 快速开始

```bash
# 克隆并启动（自动创建配置）
git clone https://github.com/your-org/go-on
cd go-on
cargo run

# 或启动桌面 GUI
cargo run --manifest-path gui/Cargo.toml

# 终端聊天模式（类似 Claude Code / Codex）
go-on --chat
```

首次运行自动检测环境 — 若未配置 AI 供应商，初始化向导将交互式引导。

默认健康检查：`http://127.0.0.1:8090/health`

---

## 许可证

MIT 或 BSD（可任选其一）。
