# BLUE70 — go-on 多 Agent 通信系统设计蓝图（革新版）

> **设计日期**: 2026-07-23（初版）→ 2026-07-23（革新版）
> **设计依据**: `docs/blueprints/principle.md` + Codex AgentControl + CodeWhale SubAgent + go-on 14-bus 架构 + 多轮深度分析
> **现状分析**: `docs/log/log-20260723-3.md` §4.1 (Agent Control 架构差距)
> **上蓝图**: BLUE69 (独立审计) | BLUE46 (14-Bus 评估)
> **革新动因**: 对 14-Bus 架构的多角度深度分析发现存在 4-5 条可合并的冗余总线，直接新增第 15 条总线会加剧复杂度。本版提出**架构精简 + 通信增强**的双重优化方案。

---

## 0. 执行摘要

### 0.1 核心发现：14-Bus 的冗余与缺口

| 维度 | 评估 |
|------|------|
| **Agent间通信** | ❌ 严重缺失 — 平面注册表、无树形层次、无取消传播 |
| **总线冗余度** | ⚠️ 4-5 条总线功能重叠可合并 — 学术优雅但工程冗余 |
| **认知负载** | 14 条总线对新贡献者门槛过高 |
| **治理完备性** | ✅ 优秀 — HarnessBus 是核心资产 |
| **协议支持** | ✅ 优秀 — 5 种协议全链路闭合 |

### 0.2 革新方案概览

```
┌──────────────────────────────────────────────────────────┐
│               BLUE70 革新版：精简 + 增强                  │
├──────────────────────────────────────────────────────────┤
│                                                          │
│  ① 总线精简（14 → 11）                                    │
│     ┌────────────────────────────────────────────────┐   │
│     │ KnowledgeBus + ReputationStore                 │   │
│     │   + ExperienceKnowledgeBase                    │   │
│     │   → UnifiedKnowledgeBus                        │   │
│     ├────────────────────────────────────────────────┤   │
│     │ QLearningAgent + FederatedRL                   │   │
│     │   → ReinforcementBus                           │   │
│     ├────────────────────────────────────────────────┤   │
│     │ WorkflowLearningBus + OptimizationBus          │   │
│     │   → LearningOptimizationBus                    │   │
│     └────────────────────────────────────────────────┘   │
│                                                          │
│  ② CommunicationBus 新增（精简设计）                       │
│     轻量级 Agent 通信 — 仅为 Tree + Messenger，           │
│     不做过度工程化的 ExactlyOnce 投递和完整通配符引擎       │
│                                                          │
│  ③ 最终架构：11 条核心总线 + 1 条通信总线 = 12 条          │
│                                                          │
└──────────────────────────────────────────────────────────┘
```

### 0.3 与原始方案的差异

| 项目 | 原始 BLUE70 | 革新版 BLUE70 |
|------|------------|--------------|
| **总线总数** | 14 + 1 = **15** | **11 + 1 = 12** |
| **消息投递保障** | 3 级（AtMost/AtLeast/ExactlyOnce） | **2 级**（去掉了 ExactlyOnce） |
| **通配符引擎** | 完整 `root/*/coder` 语法 | **简化版**（仅 `*` 单层通配） |
| **SpawnReservation** | 完整预留模式 | **直接 acquire 模式** |
| **已有总线处理** | 不动，直接叠加第 15 条 | **先精简再叠加** |
| **Agent 树遍历** | 递归 descendants/ancestors | **迭代 + 惰性求值** |
| **KV 缓存优化** | 仅 DeepSeek | **通用接口**（DeepSeek + CacheBlend） |
| **重构风险** | 低（仅叠加） | **中**（需合并总线但收益明确） |

---

## 1. 现有架构深度分析

### 1.1 14-Bus 全景与冗余图谱

```
当前 CapabilityBus 内部结构：

                     CapabilityBus
                           │
        ┌──────────────────┼──────────────────┐
        │  核心认知层      │  治理层           │  通信层
        │                  │                  │    ❌ 缺失
   ════════════════════════════════════════════════════
    WorkflowLearningBus    HarnessBus          (空白)
    KnowledgeBus ═══╗
    ReputationStore ║═══ 可合并 → UnifiedKnowledgeBus
    ExperienceKB    ╝
    CapabilityGraph
    QLearningAgent ═══╗
    FederatedRL    ═══╝ 可合并 → ReinforcementBus
    ToolBus
    ObservabilityBus
    OptimizationBus ═══╗
    WorkflowLearning ──╝ 可合并 → LearningOptimizationBus
    MemoryBus
    ProtocolBus
    OrchestrationBus
    DistributedMemoryBus
```

### 1.2 总线合并分析

| 合并组 | 当前总线 | 合并理由 | 合并为 |
|--------|---------|---------|--------|
| **知识管理组** | `KnowledgeBus` — 知识存储与检索 | 三者都管理"知识"的不同侧面，API 接口重复度 >60% | `UnifiedKnowledgeBus` |
| | `ReputationStore` — agent 声誉评分 | 声誉本质是 agent 知识的一个维度 | |
| | `ExperienceKnowledgeBase` — 经验案例库 | 经验 = 时间维度上的知识 | |
| **强化学习组** | `QLearningAgent` — Q 表路由学习 | 两者都是强化学习方法，仅算法不同 | `ReinforcementBus` |
| | `FederatedRL` — 分布式强化学习 | Q-Learning 可视为 FederatedRL 的单机特例 | |
| **学习优化组** | `WorkflowLearningBus` — 工作流模式学习 | 学习与优化天然一体：学什么 + 怎么优化 | `LearningOptimizationBus` |
| | `OptimizationBus` — 故障预防与优化 | WorkflowLearning 的输出 → Optimization 的输入 | |

### 1.3 合并收益预估

| 指标 | 精简前 | 精简后 | 改善 |
|------|--------|--------|------|
| CapabilityBus 子总线数 | 13 | **10** | **-23%** |
| 总总线数 | 14 | **11** | **-21%** |
| 模块间调用深度 | 4-5 层 | **3-4 层** | 减少间接跳转 |
| 初始化代码行数 | ~200 行 | **~150 行** | -25% |
| 新贡献者上手时间 | 2-3 天 | **1-2 天** | 认知负载降低 |

### 1.4 go-on 差距分析

| 维度 | Codex AgentControl | go-on 现状 | 差距等级 |
|------|-------------------|-----------|---------|
| **Agent 树** | `AgentTree` 层次结构 + `AgentPath` 路径解析 | ❌ 纯 `HashMap<String, Agent>` 平面注册表 | **P0** |
| **消息传递** | `AgentCommunicationContext` + spawn/message/followup/result | ❌ 仅 `chat()` 单向流 + `ToolOutput` 返回 | **P0** |
| **上下文传播** | `fork_context: true` 保留父级 KV 前缀缓存 | ❌ 每次 spawn 从头创建上下文 | **P0** |
| **执行限制** | `AgentExecutionLimiter` + `SpawnReservation` | ⚠️ 仅全局 128 semaphore | **P1** |
| **结构化输出** | 枚举类型 run receipt | ⚠️ 正则解析自由文本 | **P1** |
| **可观测性** | `codex.multi_agent.*` metrics + span context | ❌ 仅单行 info! 日志 | **P1** |
| **取消传播** | `completion_watcher` + 级联取消 | ❌ 无取消机制 | **P0** |

### 1.5 可复用的现有组件

| 组件 | 位置 | 复用策略 |
|------|------|---------|
| `Agent` trait + `chat()` | `src/agents/agent.rs` | ✅ 保留，扩展 `send_message()` 和 `on_message()` |
| `AgentRegistry` | `src/agents/mod.rs` | ✅ 保留，增加树形索引层 |
| `ForkRegistry` + `ForkEntry` | `src/orchestration/fork_registry.rs` | ✅ 增强：增加 `agent_path` 和 `budget` 字段 |
| `SpawnAgentTool` | `src/orchestration/tool/extended/spawn_agent.rs` | ✅ 改造：接入 CommunicationBus |
| `StreamingSender` | `src/agents/agent.rs` | ✅ 保留，增加结构化帧标记 |
| `ToolHookRegistry` | `src/orchestration/tool/types.rs` | ✅ 挂接 spawn 生命周期事件 |
| `MultiChannelTransport` | `src/protocol/transport.rs` | ✅ 复用为跨进程 agent 消息通道 |

---

## 2. 革新架构设计

### 2.1 最终架构全景

```
                        go-on 总线架构 (11 + 1 = 12 条)
                        ════════════════════════════════

┌── 治理层 ──────────────────────────────────────────────────────┐
│  HarnessBus — 策略评估 / 执行 / 审计 / 反馈                      │
│    ├─ PolicyEvaluator                                           │
│    ├─ AuditTrail                                                │
│    └─ FeedbackLoop                                              │
└─────────────────────────────────────────────────────────────────┘

┌── 能力层 (CapabilityBus) — sense → decide → act → feedback → evolve ┐
│                                                                      │
│  ┌─────────────────────┐  ┌─────────────────────┐                    │
│  │ UnifiedKnowledgeBus │  │  ReinforcementBus   │                    │
│  │  知识 + 声誉 + 经验  │  │  Q-Learning + FedRL │                    │
│  └─────────┬───────────┘  └──────────┬──────────┘                    │
│            │                         │                               │
│  ┌─────────▼───────────┐  ┌──────────▼──────────┐                    │
│  │ LearningOptimization│  │   CapabilityGraph   │                    │
│  │ Bus (学习+优化)      │  │   (能力图谱)         │                    │
│  └─────────────────────┘  └─────────────────────┘                    │
│                                                                      │
│  ┌─────────────────────┐  ┌─────────────────────┐                    │
│  │     ToolBus         │  │  ObservabilityBus   │                    │
│  │   (工具执行)         │  │  (可观测性)          │                    │
│  └─────────────────────┘  └─────────────────────┘                    │
│                                                                      │
│  ┌─────────────────────┐  ┌─────────────────────┐                    │
│  │    MemoryBus        │  │   ProtocolBus       │                    │
│  │  (L1/L2/L3 缓存)    │  │  (协议适配)          │                    │
│  └─────────────────────┘  └─────────────────────┘                    │
│                                                                      │
│  ┌─────────────────────┐  ┌─────────────────────┐                    │
│  │ OrchestrationBus    │  │ DistributedMemoryBus│                    │
│  │  (编排调度)          │  │  (分布式记忆)        │                    │
│  └─────────────────────┘  └─────────────────────┘                    │
│                                                                      │
└──────────────────────────────────────────────────────────────────────┘

┌── 通信层 (新增) ─────────────────────────────────────────────────┐
│  CommunicationBus — Agent 树形通信系统                             │
│    ├─ AgentTree (轻量层次索引)                                     │
│    └─ AgentMessenger (消息路由)                                    │
└─────────────────────────────────────────────────────────────────┘
```

### 2.2 合并细节：3 组总线 -> 3 条合并总线

#### 2.2.1 UnifiedKnowledgeBus（合并 KnowledgeBus + ReputationStore + ExperienceKnowledgeBase）

```rust
/// 统一知识总线 — 管理知识、声誉、经验三个维度的统一知识面
pub struct UnifiedKnowledgeBus {
    /// 可复用解决方案知识库
    knowledge_store: Vec<KnowledgeEntry>,
    /// agent 声誉评分（EMA 平滑）
    reputation_scores: HashMap<String, ReputationScore>,
    /// 成功/失败案例库
    experience_cases: Vec<ExperienceCase>,
    /// 统一查询接口：按 agent + 任务类型 检索相关知识
    relevant_knowledge: HashMap<String, Vec<KnowledgeRef>>,
}

impl UnifiedKnowledgeBus {
    /// 统一查询 — 同时检索知识 + 声誉 + 经验
    pub fn query(&self, agent: &str, task_type: &str) -> UnifiedKnowledgeResult {
        UnifiedKnowledgeResult {
            reputation: self.reputation_scores.get(agent).cloned(),
            relevant_experiences: self.experience_cases.iter()
                .filter(|e| e.agent == agent || e.task_type == task_type)
                .take(5).cloned().collect(),
            applicable_knowledge: self.relevant_knowledge.get(task_type).cloned(),
        }
    }

    /// 记录执行结果 — 同时更新知识、声誉和经验
    pub fn record_outcome(&mut self, agent: &str, task_type: &str, success: bool, summary: String) {
        self.update_reputation(agent, success);
        self.experience_cases.push(ExperienceCase {
            agent: agent.to_string(),
            task_type: task_type.to_string(),
            success,
            summary,
            timestamp_ms: now_ms(),
        });
        if success && self.is_novel_pattern(task_type, &summary) {
            self.knowledge_store.push(KnowledgeEntry {
                task_type: task_type.to_string(),
                pattern: summary,
                confidence: 0.5,
            });
        }
    }
}
```

**收益**: 减少 1 次 `Arc<RwLock<>>` 获取、1 次模块间调用跳转。知识/声誉/经验三者事务性一致。

#### 2.2.2 ReinforcementBus（合并 QLearningAgent + FederatedRL）

```rust
/// 统一强化学习总线 — Q-Learning 作为 FederatedRL 的单机后端
pub struct ReinforcementBus {
    /// Q 表 — 状态-动作价值函数
    q_table: HashMap<(String, String), f64>,
    /// FederatedRL 协调器（分布式模式启用时）
    federated_coordinator: Option<FederatedCoordinator>,
    /// 学习率
    learning_rate: f64,
    /// 折扣因子
    discount_factor: f64,
}

impl ReinforcementBus {
    pub fn select_action(&self, state: &str, available_actions: &[String]) -> Option<String> {
        available_actions.iter()
            .map(|a| (a, self.q_table.get(&(state.to_string(), a.clone())).copied().unwrap_or(0.0)))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .map(|(a, _)| a.clone())
    }

    pub fn record_reward(&mut self, state: &str, action: &str, reward: f64, next_state: &str) {
        let key = (state.to_string(), action.to_string());
        let old_q = self.q_table.get(&key).copied().unwrap_or(0.0);
        let max_next_q = self.q_table.iter()
            .filter(|((s, _), _)| s == next_state)
            .map(|(_, v)| *v)
            .fold(0.0_f64, f64::max);
        let new_q = old_q + self.learning_rate * (reward + self.discount_factor * max_next_q - old_q);
        self.q_table.insert(key, new_q);
        if let Some(ref coordinator) = self.federated_coordinator {
            coordinator.schedule_sync(state, action, new_q);
        }
    }
}
```

**收益**: 消除两条 RL 总线的概念二义性。单机部署零额外开销，分布式按需启用联邦组件。

#### 2.2.3 LearningOptimizationBus（合并 WorkflowLearningBus + OptimizationBus）

```rust
/// 学习优化总线 — 从历史模式中提取优化策略
pub struct LearningOptimizationBus {
    /// 历史执行事件（原 WorkflowLearningBus）
    events: VecDeque<WorkflowLearningEvent>,
    /// 故障预防规则（原 OptimizationBus）
    prevention_rules: Vec<PreventionRule>,
    /// 优化建议缓存
    optimization_cache: HashMap<String, OptimizationSuggestion>,
}

impl LearningOptimizationBus {
    /// 记录执行事件 + 触发优化分析（原子操作）
    pub fn record_and_optimize(&mut self, event: WorkflowLearningEvent) {
        self.events.push_back(event.clone());
        if let Some(suggestion) = self.analyze_for_optimization(&event) {
            self.optimization_cache.insert(event.task_type.clone(), suggestion);
        }
        if let Some(rule) = self.analyze_for_prevention(&event) {
            self.prevention_rules.push(rule);
        }
    }

    pub fn suggestion_for(&self, task_type: &str) -> Option<&OptimizationSuggestion> {
        self.optimization_cache.get(task_type)
    }

    pub fn agent_success_rate(&self, agent: &str) -> Option<f64> {
        let (total, successes) = self.events.iter()
            .filter(|e| e.agent == agent)
            .fold((0usize, 0usize), |(t, s), e| (t + 1, s + e.success as usize));
        if total == 0 { None } else { Some(successes as f64 / total as f64) }
    }
}
```

**收益**: WorkflowLearning 的历史数据直接喂给 Optimization 分析管线，无需中间 event 传递。

---

### 2.3 CommunicationBus — 精简设计

#### 2.3.1 设计原则

1. **够用就好** — 只解决 Agent 树形通信的核心痛点，不做过度工程
2. **两级投递** — 只实现 `AtMostOnce`（fire-and-forget）和 `AtLeastOnce`（确认重试），去掉 `ExactlyOnce`
3. **简化通配** — 只支持 `*` 单层通配（`root/*/coder`），不支持 `**` 和复杂 pattern 语法
4. **直接 acquire** — 去掉 SpawnReservation 预留模式，直接用 Semaphore::acquire
5. **迭代遍历** — AgentTree 遍历用 BFS 迭代器 + 惰性求值，不用递归

#### 2.3.2 整体架构

```
                          ┌──────────────────────┐
                          │  CommunicationBus     │  ← 新增 (第12条总线)
                          │  (轻量消息路由)        │
                          └──────┬───────────────┘
                                 │
                          ┌──────▼───────┐
                          │  AgentTree   │
                          │  (层次索引)    │
                          └──────┬───────┘
                                 │
                    ┌────────────┴────────────┐
                    │                         │
             ┌──────▼──────┐          ┌───────▼──────┐
             │ AgentPath   │          │ AgentMessage │
             │ 路径解析器   │          │ 消息类型定义  │
             └─────────────┘          └──────────────┘
```

```
CommunicationBus
  ├── AgentTree          — 层次化 agent 索引（HashMap + 父指针）
  ├── AgentMessenger     — 消息路由和投递（inbox 模式）
  └── CommunicationHealth— health 报告（集成到现有 health 端点）
```

#### 2.3.3 与合并后总线的交互

```
ToolBus ──→ CommunicationBus: PreToolUse hook → spawn_agent 路由到 CommunicationBus
MemoryBus ←→ CommunicationBus: 子代理继承父代理的记忆上下文摘要
ObservabilityBus ←─ CommunicationBus: 通信事件输出 tracing span + metrics
ProtocolBus → CommunicationBus: ACP 协议扩展支持跨进程 agent 通道
```

---

## 3. 核心类型定义（精简版）

### 3.1 AgentPath — 层次化寻址

```rust
/// Agent 路径：root/research/coder
///
/// 支持的路径格式：
/// - "root"                    → 根代理
/// - "root/research"           → 根下的 research 子代理
/// - "root/research/coder"     → 三层嵌套
/// - "."                       → 当前代理
/// - ".."                      → 父代理
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct AgentPath {
    segments: Vec<String>,
}

impl AgentPath {
    /// 从字符串解析 "/" 分隔的路径
    pub fn parse(path: &str) -> Result<Self>;
    /// 返回父路径
    pub fn parent(&self) -> Option<AgentPath>;
    /// 追加子路径
    pub fn child(&self, name: &str) -> AgentPath;
    /// 路径深度
    pub fn depth(&self) -> usize;
    /// 是否是根路径
    pub fn is_root(&self) -> bool;
    /// 简化通配匹配：仅支持 * 单层匹配
    pub fn matches_simple(&self, pattern: &AgentPathPattern) -> bool;
}
```

### 3.2 AgentNode — 树节点（轻量）

```rust
/// Agent 树的节点 — 不含递归 children HashMap，改为父指针 + 子路径列表
#[derive(Debug, Clone)]
pub struct AgentNode {
    /// 此节点在树中的路径
    pub path: AgentPath,
    /// agent 名称（对应 AgentRegistry 中的 key）
    pub agent_name: String,
    /// 父节点路径（None = 根节点）
    pub parent_path: Option<AgentPath>,
    /// 子节点路径列表（仅在需要时遍历树）
    pub children: Vec<AgentPath>,
    /// 节点元数据
    pub metadata: AgentNodeMetadata,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentNodeMetadata {
    pub created_at_ms: u64,
    pub role: Option<String>,
    pub model: Option<String>,
    pub token_budget: Option<u64>,
    pub depth_limit: Option<u32>,
    /// 是否自动 fork 上下文
    pub fork_context: bool,
}
```

**设计变更说明**：去掉递归的 `HashMap<String, AgentNode>` 子节点结构，改为扁平 `HashMap<AgentPath, AgentNode>` + 父指针 + 子路径列表。因 `HashMap<AgentPath, AgentNode>` 已是树的所有节点的全集，`children` 仅为遍历索引，避免了递归 clone 的性能问题和栈溢出风险。

### 3.3 AgentMessage — 结构化消息（精简）

```rust
/// Agent 间消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    /// 消息 ID (UUID v4)
    pub id: String,
    /// 发送者路径
    pub from: AgentPath,
    /// 接收者路径
    pub to: AgentTarget,
    /// 消息时间戳
    pub timestamp_ms: u64,
    /// 消息类型
    pub kind: AgentMessageKind,
    /// 消息负载
    pub payload: Value,
    /// 父消息 ID (用于回复链)
    pub in_reply_to: Option<String>,
}

/// 消息目标 — 替代原始方案的 AgentPathPattern + Channel 枚举
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentTarget {
    /// 直接消息到指定路径
    Direct(AgentPath),
    /// 广播到所有子孙节点
    Broadcast,
    /// 发送到父节点
    ToParent,
    /// 简化通配模式：root/*/coder（仅支持 * 单层）
    Pattern { prefix: Vec<String>, suffix: Vec<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentMessageKind {
    /// 任务委托：父 → 子
    Delegate {
        task: String,
        role: Option<String>,
        token_budget: Option<u64>,
        timeout_secs: u64,
    },
    /// 任务结果：子 → 父
    Result {
        success: bool,
        summary: Option<String>,
        changes: Option<String>,
        evidence: Option<String>,
        risks: Option<String>,
        blockers: Option<String>,
        response: String,
        actual_tokens: u64,
    },
    /// 进度更新：子 → 父（流式中间结果）
    Progress { tokens: String, partial: bool },
    /// 取消请求：父 → 子
    Cancel { reason: String },
    /// 状态查询 / 响应
    StatusQuery,
    StatusResponse { phase: String, elapsed_ms: u64, tokens_used: u64 },
    /// 自定义消息
    Custom { event: String },
}
```

**去掉的内容**：`priority: u8`（Messenger 层内部排序即可）、`DeliveryGuarantee` 枚举（仅内部实现）、`AgentChannel` 枚举（合并到 `AgentTarget`）、完整通配引擎（简化为 `Pattern { prefix, suffix }`）。

### 3.4 ForkContext — 上下文继承

```rust
/// 上下文快照：子代理可继承父代理的运行时状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkContext {
    pub parent_path: AgentPath,
    pub conversation_summary: Option<String>,
    pub principles: Vec<String>,
    pub allowed_base_dir: Option<PathBuf>,
    pub inherited_memories: Vec<String>,
    pub kv_cache_fingerprint: Option<String>,
}
```

### 3.5 ExecutionGovernor — 执行控制（精简）

```rust
/// 执行控制状态 — 去掉 SpawnReservation，使用直接 Semaphore
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentExecutionBudget {
    pub aggregate_token_ceiling: Option<u64>,
    pub aggregate_tokens_used: u64,
    pub max_depth: u32,
    pub max_concurrency: usize,
    pub active_children: usize,
    pub max_wall_clock_ms: Option<u64>,
}
```

**去掉的内容**：`started_at_ms`（由调用方管理）、`SpawnReservation` 预留模式。

---

## 4. AgentTree — 轻量层次化索引

### 4.1 数据结构

```rust
pub struct AgentTree {
    /// 注册表：path → AgentNode（扁平）
    nodes: HashMap<AgentPath, AgentNode>,
    /// 根节点路径缓存
    root_path: Option<AgentPath>,
}
```

### 4.2 操作

```rust
impl AgentTree {
    pub fn register(&mut self, path: &AgentPath, agent_name: &str, metadata: AgentNodeMetadata) -> Result<()>;
    pub fn resolve(&self, path: &AgentPath) -> Option<&AgentNode>;
    pub fn resolve_pattern(&self, pattern: &AgentTarget::Pattern) -> Vec<&AgentNode>;
    pub fn ancestors(&self, path: &AgentPath) -> Vec<&AgentNode>;      // 向上迭代
    pub fn descendants(&self, path: &AgentPath) -> Vec<&AgentNode>;    // BFS 迭代
    pub fn import_from_registry(&mut self, registry: &AgentRegistry) -> Result<()>;
    pub fn remove_subtree(&mut self, path: &AgentPath) -> Vec<AgentPath>;  // BFS 收集 + 批量移除
}
```

### 4.3 关键设计差异

| 设计点 | 原始版本 | 精简版本 |
|--------|---------|---------|
| 子节点存储 | `HashMap<String, AgentNode>` 递归 | `Vec<AgentPath>` 索引 |
| 内存模型 | 树中有树（nested HashMap） | 扁平 map + 父指针 |
| `descendants()` | 递归遍历 | **BFS 迭代**（避免栈溢出） |
| clone 成本 | 递归 clone 子树 | O(1) clone AgentPath |
| 子树删除 | 递归删除 | BFS 收集 + HashMap 批量移除 |

### 4.4 BFS 遍历算法

```
AgentTree.descendants("root/research")
     ↓
1. 初始化队列 = ["root/research"]
2. 初始化结果 = []
3. 循环：队列非空时
   a. 弹出队首路径
   b. 从 nodes 中取出该路径的节点
   c. 将该节点的所有 children 加入队列
   d. 将节点加入结果
4. 返回结果
```

---

## 5. AgentMessenger — 消息路由（精简）

### 5.1 核心流程

```
发送消息:
  AgentMessenger::send(msg)
    ├─ 1. 验证 msg.from 在 AgentTree 中存在
    ├─ 2. 匹配接收者:
    │     ├─ Direct(path)    → 单个接收者
    │     ├─ Broadcast       → 所有子孙（tree.descendants）
    │     ├─ ToParent        → from 的父节点
    │     └─ Pattern(p,s)    → 通配匹配
    ├─ 3. 对每个匹配的接收者:
    │     ├─ 同进程 → 投递到接收者 inbox
    │     └─ 跨进程 → 通过 MultiChannelTransport 转发
    ├─ 4. 记录 ObservabilityBus (span + metric)
    └─ 5. 返回发送结果

接收消息:
  AgentMessenger::recv(path) → Vec<AgentMessage>
    ├─ 从 path 对应的 inbox 取出所有待处理消息
    └─ 可选按消息类型排序返回
```

### 5.2 投递保障实现

```rust
impl AgentMessenger {
    /// 最多一次投递 (fire-and-forget)
    pub async fn send_at_most_once(&self, msg: AgentMessage) -> Result<()>;
    /// 至少一次投递（带确认重试，最多 3 次）
    pub async fn send_at_least_once(&self, msg: AgentMessage) -> Result<()>;
}
```

Delegate 和 Cancel 等关键消息使用 `send_at_least_once`，进度更新等非关键消息使用 `send_at_most_once`。

### 5.3 取消传播

```
父代理取消时：
  AgentMessenger::cancel_subtree(path, reason)
    ├─ 1. 获取 path 的所有子孙（BFS）
    ├─ 2. 向每个子孙发送 Cancel 消息（AtLeastOnce）
    ├─ 3. 每个收到 Cancel 的子代理：
    │     ├─ 停止当前执行
    │     ├─ 向自己的子代级联 Cancel
    │     └─ 向父代理发送 Result { success: false, blockers: "cancelled" }
    └─ 4. 从 AgentTree 中移除子树（异步 cleanup）
```

取消传播是异步的，父代理不会等待所有子代理确认。

---

## 6. ContextForker — 上下文继承

### 6.1 流程

```
ContextForker::fork(parent_path, child_path, options)
    ├─ 1. 收集父代理上下文:
    │     ├─ conversation_summary（最近 N 轮对话摘要）
    │     ├─ principles（PUA 规则）
    │     ├─ allowed_base_dir
    │     └─ kv_cache_fingerprint（通用接口）
    ├─ 2. 创建 ForkContext 快照
    ├─ 3. 注入子代理系统提示前缀
    └─ 4. 在 ForkRegistry 中注册 fork 记录
```

### 6.2 KV 缓存优化（通用接口）

```
对于支持 KV 缓存的模型（抽象接口，不限 DeepSeek）：
  pub trait KvCacheProvider: Send + Sync {
      fn cache_fingerprint(&self) -> Option<String>;
      fn try_attach_cache(&self, fingerprint: &str) -> bool;
  }

各 Provider 按需实现：
  - DeepSeekProvider    → 原生前缀缓存
  - AnthropicProvider   → Prompt Caching API
  - CacheBlendProvider  → CacheBlend 技术
```

---

## 7. ExecutionGovernor — 执行控制

### 7.1 限制规则

| 限制项 | 检查时机 | 行为 |
|--------|---------|------|
| `max_depth` | spawn 前 | 超限则拒绝 spawn |
| `aggregate_token_ceiling` | 每次 token 产出后 | 超限则停止子代理 |
| `max_concurrency` | spawn 前 | 超限则排队（Semaphore） |
| `max_wall_clock_ms` | 定时检查 | 超限则取消子树 |
| `max_children_per_parent` | spawn 前 | 超限则报错 |

### 7.2 直接模式

```
父代理 spawn 子代理：
  1. ExecutionGovernor::check_limits(path, budget) → 检查所有限制
  2. 通过 → Semaphore::acquire() 获取并发插槽
  3. 执行子代理
  4. 完成 → Semaphore::release()
```

去掉了原版的 SpawnReservation 预留模式。在 AI agent 编排场景中，check 和 acquire 之间的窗口期极短（微秒级），竞态概率可忽略。

---

## 8. 与现有系统的集成

### 8.1 Agent trait 扩展

```rust
#[async_trait]
pub trait Agent: Send + Sync {
    // 现有方法（不变）
    async fn chat(&self, ...);
    fn available_models(&self) -> Vec<ModelInfo>;

    // 新增方法
    async fn on_message(&self, msg: AgentMessage) -> Result<Option<AgentMessage>> {
        tracing::info!(from = %msg.from, kind = ?msg.kind, "agent received message");
        Ok(None)
    }
    async fn send_message(&self, messenger: &AgentMessenger, to: AgentTarget, kind: AgentMessageKind, payload: Value) -> Result<()>;
}
```

### 8.2 SpawnAgentTool 改造

```rust
fn execute_spawn(...) {
    // 1. 注册子节点
    communication_bus.tree().register(&child_path, &agent_name, metadata)?;
    // 2. 创建上下文快照
    let fork_ctx = communication_bus.forker().fork(&parent_path, &child_path)?;
    // 3. 检查限制 + 获取并发插槽
    communication_bus.governor().check_limits(&child_path, &budget)?;
    let _permit = communication_bus.governor().acquire().await?;
    // 4. 发送 Delegate 消息
    let msg = AgentMessage::delegate(&parent_path, &child_path, task, role, budget);
    communication_bus.messenger().send_at_least_once(msg).await?;
    // 5. 接收 Result
    let result = communication_bus.messenger()
        .wait_for(&parent_path, |msg| msg.kind.is_result(), timeout).await?;
}
```

### 8.3 ForkRegistry 增强

```rust
// 增强后 ForkEntry：
pub struct ForkEntry {
    pub id: String,
    pub agent_path: AgentPath,           // 新增
    pub parent_agent_path: AgentPath,    // 新增
    pub parent_task_id: String,
    pub status: ForkStatus,
    pub snapshot: Option<ForkSnapshot>,
    pub budget: AgentExecutionBudget,    // 新增
    pub context: Option<ForkContext>,    // 新增
    pub started_at_ms: u64,
    pub completed_at_ms: Option<u64>,
}
```

### 8.4 ToolHook 集成

```rust
pub struct AgentCommunicationHook {
    bus: Arc<CommunicationBus>,
}

impl ToolHook for AgentCommunicationHook {
    fn pre_execute(&self, tool_name: &str, input: &ToolInput) -> Result<()> {
        if tool_name == "spawn_agent" {
            self.bus.tree().register(...)?;
        }
        Ok(())
    }
    fn post_execute(&self, tool_name: &str, input: &ToolInput, output: &ToolOutput, duration_ms: u64) -> Result<()> {
        if tool_name == "spawn_agent" {
            self.bus.record_metrics(tool_name, duration_ms, output.success)?;
        }
        Ok(())
    }
}
```

### 8.5 CapabilityBus 总线合并迁移路径

```
Step 1: 创建新模块（不删旧代码）
  src/intelligence/capability_bus/
    unified_knowledge_bus.rs
    reinforcement_bus.rs
    learning_optimization_bus.rs

Step 2: 逐个实现新总线，保留旧总线作为 delegate
  struct UnifiedKnowledgeBus {
      inner: Arc<RwLock<UnifiedKnowledgeBusInner>>,
      #[cfg(feature = "migration-legacy-knowledge")]
      legacy_knowledge: Arc<RwLock<KnowledgeBus>>,
      #[cfg(feature = "migration-legacy-reputation")]
      legacy_reputation: Arc<Mutex<ReputationStore>>,
  }

Step 3: 逐个迁移调用点
  Phase 1: 新代码使用新总线
  Phase 2: 旧调用点逐个切换
  Phase 3: 删除旧代码和迁移 feature flag

Step 4: 验证（每步后运行全量测试）
  cargo check + cargo clippy + cargo test → 2068+ 不减少
```

---

## 9. 可观测性

### 9.1 Tracing Spans（精简）

```rust
let span = info_span!("agent_communication",
    from = %msg.from, to = ?msg.to, kind = ?msg.kind,
);
// 事件: send / deliver / receive / cancel
```

### 9.2 Metrics（核心指标，控制基数）

```rust
metrics::counter!("agent_comm.messages_sent", "kind" => kind);
metrics::counter!("agent_comm.messages_received", "kind" => kind);
metrics::histogram!("agent_comm.delivery_latency_ms");
metrics::gauge!("agent_comm.active_agents");
metrics::counter!("agent_comm.forks");
metrics::counter!("agent_comm.cancellations");
```

**去掉的指标**：`"from" => from` 标签（高基数爆炸风险）、`fork_context_size_bytes`（收益 < 成本）。

---

## 10. 实现计划

### 总体时间线：10-14 天

```
Week 1: 总线精简（5-7 天）
  ├── Day 1-2: UnifiedKnowledgeBus 实现 + 旧 KnowledgeBus 迁移
  ├── Day 3:   ReinforcementBus 实现 + 旧 QLearning/FederatedRL 迁移
  ├── Day 4-5: LearningOptimizationBus 实现 + 旧 WorkflowLearning/Optimization 迁移
  └── Day 6-7: 删除旧总线代码、全量回归测试

Week 2: CommunicationBus 实现（5-7 天）
  ├── Day 1:   核心类型（AgentPath / AgentMessage / ForkContext / Budget）
  ├── Day 2-3: AgentTree + AgentMessenger 实现
  ├── Day 4:   ContextForker + ExecutionGovernor 实现
  ├── Day 5:   SpawnAgentTool 改造 + ToolHook 集成
  ├── Day 6:   ForkRegistry 增强 + server_builder 接线
  └── Day 7:   全量测试 + 性能基准对比
```

### Phase 1: 总线精简（5-7 天）

| 任务 | 新文件 | 说明 |
|------|--------|------|
| `UnifiedKnowledgeBus` | `src/intelligence/capability_bus/unified_knowledge_bus.rs` | 合并 KnowledgeBus + ReputationStore + ExperienceKB |
| `ReinforcementBus` | `src/intelligence/capability_bus/reinforcement_bus.rs` | 合并 QLearningAgent + FederatedRL |
| `LearningOptimizationBus` | `src/intelligence/capability_bus/learning_optimization_bus.rs` | 合并 WorkflowLearningBus + OptimizationBus |
| CapabilityBus 更新 | `core.rs` | 替换 3 组旧总线为 3 条新总线 |
| 删除旧代码 | 删除 6 个旧文件 | 确认无引用后删除 |
| 全量回归 | `cargo test --lib` | 确认 2068+ 测试全部通过 |

### Phase 2: CommunicationBus 核心类型（1 天）

| 任务 | 文件 | 产出 |
|------|------|------|
| `AgentPath` + 解析器 | `src/agents/communication/path.rs` | ✅ 路径解析与简化通配匹配 |
| `AgentMessage` 类型 | `src/agents/communication/message.rs` | ✅ 消息类型枚举 + AgentTarget |
| `ForkContext` 类型 | `src/agents/communication/context.rs` | ✅ 上下文快照 |
| `AgentExecutionBudget` | `src/agents/communication/budget.rs` | ✅ 预算类型 |

### Phase 3: AgentTree + AgentMessenger（2-3 天）

| 任务 | 文件 | 产出 |
|------|------|------|
| `AgentNode` + `AgentTree` | `src/agents/communication/tree.rs` | ✅ 轻量层次化索引 |
| `AgentMessenger` 路由 | `src/agents/communication/messenger.rs` | ✅ 消息收发 |
| `ContextForker` | `src/agents/communication/forker.rs` | ✅ 上下文继承 |
| `ExecutionGovernor` | `src/agents/communication/governor.rs` | ✅ 执行控制 |

### Phase 4: CommunicationBus 集成（2-3 天）

| 任务 | 文件 | 产出 |
|------|------|------|
| `CommunicationBus` Builder | `src/agents/communication/bus.rs` | ✅ Bus 骨架 + health |
| `SpawnAgentTool` 改造 | `spawn_agent.rs` | ✅ 使用 CommunicationBus |
| `Agent` trait 扩展 | `agent.rs` | ✅ on_message/send_message |
| `AgentCommunicationHook` | `types.rs` | ✅ ToolHook 集成 |
| `ForkRegistry` 增强 | `fork_registry.rs` | ✅ agent 路径字段 |
| `server_builder.rs` 接线 | `server_builder.rs` | ✅ 初始化 |

---

## 11. 兼容性保证

| 保证项 | 说明 |
|--------|------|
| **Agent trait 向后兼容** | `chat()` 方法签名不变，新增方法有默认实现 |
| **SpawnAgentTool 向后兼容** | 保留现有 `run()`/`run_async()` 路径 |
| **ForkRegistry 向后兼容** | 新增字段 Optional，向后兼容反序列化 |
| **CapabilityBus API 兼容** | 合并后保留旧方法的 delegate 层 |
| **Profile 兼容** | local/simple-server/multi-users-server/full 全部支持 |
| **principle.md 合规** | 零 block_on，全 try_current() + fallback |
| **i18n 兼容** | 所有新增字符串通过 i18n 键转译 |

---

## 12. 最终验证标准

```
# 编译验证
cargo check                      ✅ 零错误
cargo clippy --all-targets -D warnings  ✅ 零警告

# 存量测试
cargo test --lib                 ✅ 2068 passed, 0 failed, 0 ignored

# 新增通信测试
cargo test --lib agent_communication  ✅ 全部通过
cargo test --lib agent_tree          ✅ 全部通过
cargo test --lib agent_messenger     ✅ 全部通过

# 精简总线测试
cargo test --lib unified_knowledge_bus  ✅ 全部通过
cargo test --lib reinforcement_bus      ✅ 全部通过
cargo test --lib learning_optimization  ✅ 全部通过

# Profile 全链路
cargo build --no-default-features --features simple-server  ✅
cargo build --no-default-features --features multi-users-server  ✅
cargo build --no-default-features --features full  ✅
```

---

## 13. 与原始方案对比总结

| 维度 | 原始 BLUE70 | 革新版 BLUE70 | 改善 |
|------|-----------|-------------|------|
| **总线总数** | 14+1 = **15** | 11+1 = **12** | **-20%** |
| **CommunicationBus 复杂度** | 5 组件 | **4 组件** | 更轻量 |
| **消息投递级别** | 3 级 | **2 级** | 去掉过度设计 |
| **通配符引擎** | 完整语法 | **简化 `*` 单层** | 去掉 60% 通配代码 |
| **SpawnReservation** | 完整预留模式 | **直接 acquire** | 节省一次锁操作 |
| **树遍历方式** | 递归 | **BFS 迭代** | 避免栈溢出 |
| **子节点存储** | 递归 HashMap | **扁平 Vec 索引** | O(1) clone |
| **KV 缓存** | 仅 DeepSeek | **通用接口** | 多模型受益 |
| **总线合并** | ❌ 不合并 | ✅ **合并 3 组** | 认知负载降低 |
| **迁移策略** | 新增 | **先精简再新增 + 分步迁移** | 更安全 |
| **指标基数控制** | ❌ 高基数标签 | ✅ **限制 per-path 标签** | 避免 Prometheus OOM |
| **实现周期** | 10-14 天 | **10-14 天**（含合并） | 同等时间更多收益 |

---

## 14. 风险与缓解

| 风险 | 等级 | 缓解措施 |
|------|------|---------|
| 总线合并破坏现存调用路径 | **中** | 分步迁移 + feature flag + legacy delegate 层 |
| CommunicationBus 与现有 SpawnAgentTool 竞争 | **低** | 新增路径不删除旧路径，逐步切换 |
| AgentTree 大规模树性能 | **低** | BFS 迭代 + 惰性求值，benchmark 阈值 1000 节点 |
| KV 缓存通用接口适配成本 | **中** | 仅 DeepSeek 必须实现，其余 Optional |
| 合并后测试覆盖率下降 | **低** | 每步全量测试，确保 2068+ 不减少 |
