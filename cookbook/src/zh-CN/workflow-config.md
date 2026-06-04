# 工作流配置

go-on 支持多种工作流类型，适用于不同的使用场景。工作流在 `config.toml` 的 `[flow]` 部分进行配置。

> 相关文档：[GUI 控制台](gui.md) | [Prompts 系统](prompts.md)

---

## 1. 工作流类型

| 类型 | 值 | 用途 | 默认阶段 |
|------|-----|------|----------|
| Auto | `"auto"` | 根据上下文自动检测 | 因情况而异 |
| Dev | `"dev"` | 软件开发 | planning → coding → review → delivery |
| General | `"general"` | 问答、分析、研究 | gathering → thinking → executing → validating → closing |
| Free | `"free"` | 单轮对话，无阶段路由 | 无 |
| Custom | `"custom"` | 用户自定义阶段 | 用户自定义 |

---

## 2. 快速开始

在 `config/templates/` 中提供了两个即用的配置模板：

### 开发工作流
```bash
cp config/templates/config.dev.toml config.toml
# 编辑 config.toml 添加您的 API 密钥
go-on --config config.toml
```

### 通用工作流
```bash
cp config/templates/config.general.toml config.toml
# 编辑 config.toml 添加您的 API 密钥
go-on --config config.toml
```

---

## 3. 代理路由流程

**代理并不绑定到阶段。** 所有注册的 AI 提供者全局可用。**CapabilityBus** 根据以下条件动态为每个子任务选择最佳提供者：

1. **任务上下文** — 正在执行的工作类型（编码、分析等）
2. **信誉评分** — 类似任务的历史成功率
3. **能力标签** — 每个提供者擅长的领域

您可以在不列出代理的情况下定义阶段——总线会自动选择最佳代理：

```toml
[phases.coding]
description = "编码 — 实现功能"
agents = []          # ← 留空！能力总线会自动选择最佳代理。
fallback = true
```

或者通过列出首选代理来给总线提示：

```toml
[phases.coding]
agents = ["deepseek", "openai"]   # ← 提示：优先使用这些，但总线仍会自行决定
fallback = true
```

---

## 4. 自动检测（`workflow_type = "auto"`）

当未设置 `workflow_type` 或设置为 `"auto"` 时，系统会在启动时检测上下文：

- 如果检测到代码仓库 → 使用 **Dev** 工作流（4 个阶段）
- 否则 → 使用 **General** 工作流（5 个阶段）

通过在配置中显式设置 `workflow_type` 来覆盖默认行为。

---

## 5. 自由模式（`workflow_type = "free"`）

自由模式绕过阶段路由。每次请求都是单轮交互，不进行阶段切换：

```toml
[flow]
name = "自由聊天"
workflow_type = "free"
phases = []   # 无阶段 — 仅直接路由
```

---

## 6. 自定义工作流

定义您自己的工作流阶段和转换：

```toml
default_phase = "research"
model_selection_mode = "adaptive"

[flow]
name = "我的研究工作流"
workflow_type = "custom"
phases = ["research", "draft", "polish", "publish"]

[phases.research]
description = "研究 — 收集资料、分析数据"
agents = []
fallback = true

[phases.research.options]
request_timeout_seconds = 180
cache_enabled = true
vector_enabled = true
phase_max_inflight = 8
global_max_inflight = 128
```

每个阶段都支持可配置选项，包括超时、缓存、向量记忆、并发限制以及高风险多代理投票设置。

---

## 7. 高风险多代理投票

对于高风险阶段（医疗、法律、金融、安全关键），go-on 支持多代理联合处理：

1. **风险检测** — 分析用户消息中的领域和决策关键词
2. **多代理执行** — 多个代理并行独立生成回复
3. **投票** — 回复去重并排名；获得共识者胜出
4. **升级处理** — 如未达成共识，由其他模型参与打破平局

通过 `options` 在每个阶段进行配置：

```toml
[phases.review.options]
high_risk_vote_enabled = true
high_risk_vote_threshold = 1
high_risk_vote_min_agents = 2
high_risk_vote_max_agents = 4
high_risk_escalate_multi_model_enabled = true
```

---

> 完整的文档（包括阶段选项参考、功能配置文件和技能系统）可在项目 `docs/workflow-config.md` 中找到。
