# BLUE70 — go-on 多 Agent 通信系统设计蓝图

> **设计日期**: 2026-07-23
> **设计依据**: `docs/blueprints/principle.md` + Codex AgentControl + CodeWhale SubAgent + go-on 14-bus 架构
> **现状分析**: `docs/log/log-20260723-3.md` §4.1 (Agent Control 架构差距)
> **上蓝图**: BLUE69 (独立审计)

---

## 0. 设计目标

为 go-on 设计一个**完整的、与现有 14-bus 架构融合的**多 Agent 通信系统，使 AI agent 能以树形层次结构协作，支持以下能力：

1. **Agent 树** — 层次化 agent 关系，路径寻址（如 `root/research/coder`）
2. **消息传递** — agent 间直接消息、广播、父子通信
3. **上下文传播** — 父代理的 KV 缓存、原则、记忆可继承给子代理
4. **执行控制** — 深度限制、并发上限、token 预算、超时
5. **结构化输出** — 不只是纯文本 token，包含 SUMMARY/CHANGES/EVIDENCE/RISKS/BLOCKERS
6. **可观测性** — 每个通信链路有 tracing span 和 metrics
7. **取消传播** — 父代理取消时级联取消所有子代理
8. **与现有系统兼容** — Agent trait、ForkRegistry、SpawnAgentTool、Bus 架构

---

## 1. 现有架构差距分析

### 1.1 go-on 当前状态 vs Codex AgentControl

| 维度 | Codex AgentControl | go-on 现状 |
|------|-------------------|-----------|
| **Agent 树** | `AgentTree` 层次结构 + `AgentPath` 路径解析 | ❌ 纯 `HashMap<String, Agent>` 平面注册表 |
| **消息传递** | `AgentCommunicationContext` + spawn/message/followup/result | ❌ 仅 `chat()` 单向流 + `ToolOutput` 返回 |
| **上下文传播** | `fork_context: true` 保留父级 KV 前缀缓存 | ❌ 每次 spawn 从头创建上下文 |
| **执行限制** | `AgentExecutionLimiter` + `SpawnReservation` | ⚠️ 仅全局 128 semaphore |
| **结构化输出** | 枚举类型 run receipt | ⚠️ 正则解析自由文本 |
| **可观测性** | `codex.multi_agent.*` metrics + span context | ❌ 仅单行 info! 日志 |
| **取消传播** | `completion_watcher` + 级联取消 | ❌ 无取消机制 |

### 1.2 可复用的现有组件

| 组件 | 位置 | 可复用程度 |
|------|------|-----------|
| `Agent` trait + `chat()` | `src/agents/agent.rs` | ✅ 保留，扩展 `send_message()` |
| `AgentRegistry` | `src/agents/mod.rs` | ✅ 保留，增加树形索引 |
| `ForkRegistry` + `ForkEntry` | `src/orchestration/fork_registry.rs` | ✅ 增强：增加 agent 链路追踪 |
| `SpawnAgentTool` | `src/orchestration/tool/extended/spawn_agent.rs` | ✅ 改造：使用 AgentCommunicationBus |
| `StreamingSender` | `src/agents/agent.rs` | ✅ 保留，增加结构化帧支持 |
| `ToolHookRegistry` | `src/orchestration/tool/types.rs` | ✅ Hook 系统可挂接通信事件 |
| `MultiChannelTransport` | `src/protocol/transport.rs` | ✅ 可复用作 agent 间消息通道 |

---

## 2. 架构设计

### 2.1 整体架构

```
                           ┌──────────────────────┐
                           │   CommunicationBus    │  ← 新增 Bus (15th Bus)
                           │  (消息路由 + 分发)      │
                           └──────┬───────────────┘
                                  │
            ┌─────────────────────┼─────────────────────┐
            │                     │                     │
     ┌──────▼──────┐    ┌────────▼───────┐    ┌────────▼──────┐
     │  AgentTree  │    │ AgentMessenger │    │ ContextForker │
     │  (层次索引)  │    │ (消息收发)      │    │ (上下文继承)   │
     └──────┬──────┘    └────────┬───────┘    └────────┬──────┘
            │                    │                     │
     ┌──────▼──────┐    ┌────────▼───────┐    ┌────────▼──────┐
     │ AgentPath   │    │ AgentMessage   │    │ ForkContext   │
     │ 路径解析器   │    │ 消息类型定义    │    │ 上下文快照     │
     └─────────────┘    └────────────────┘    └───────────────┘
```

### 2.2 CommunicationBus — 新增第 15 个 Bus

作为独立 Bus 接入现有 14-bus 架构，遵循 Bus 模式的 Builder + Profile + health 端点规范。

```
CommunicationBus
  ├── AgentTree          — 层次化 agent 索引
  ├── AgentMessenger     — 消息路由和投递
  ├── ContextForker      — 上下文继承与快照
  ├── ExecutionGovernor  — 执行限制器
  └── CommunicationHealth— health 报告
```

### 2.3 与现有 Bus 的交互

```
ToolBus ──→ CommunicationBus: PreToolUse hook → spawn_agent 路由到 CommunicationBus
MemoryBus ←→ CommunicationBus: 子代理可继承父代理的记忆上下文
ObservBus ←─ CommunicationBus: 所有通信事件生成 tracing span + metrics
ProtocolBus → CommunicationBus: ACP 协议扩展支持 agent 间消息通道
```

---

## 3. 核心类型定义

### 3.1 AgentPath — 层次化寻址

```rust
/// Agent 路径：root/research/coder
///
/// 支持以下格式：
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

    /// 通配符匹配 (root/*/coder 匹配 root/research/coder)
    pub fn matches(&self, pattern: &AgentPathPattern) -> bool;
}
```

### 3.2 AgentNode — 树节点

```rust
/// Agent 树的节点
#[derive(Debug, Clone)]
pub struct AgentNode {
    /// 此节点在树中的路径
    pub path: AgentPath,
    /// agent 名称（对应 AgentRegistry 中的 key）
    pub agent_name: String,
    /// 子节点索引
    pub children: HashMap<String, AgentNode>,
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

### 3.3 AgentMessage — 结构化消息

```rust
/// Agent 间消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    /// 消息 ID (UUID v4)
    pub id: String,
    /// 发送者路径
    pub from: AgentPath,
    /// 接收者路径 (支持通配符)
    pub to: AgentPathPattern,
    /// 消息时间戳
    pub timestamp_ms: u64,
    /// 消息类型
    pub kind: AgentMessageKind,
    /// 消息负载
    pub payload: Value,
    /// 父消息 ID (用于回复链)
    pub in_reply_to: Option<String>,
    /// 优先级 (0=low, 5=normal, 10=high)
    pub priority: u8,
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
    Progress {
        tokens: String,
        partial: bool,
    },
    /// 取消请求：父 → 子
    Cancel {
        reason: String,
    },
    /// 状态查询：任意 → 任意
    StatusQuery,
    /// 状态响应
    StatusResponse {
        phase: String,
        elapsed_ms: u64,
        tokens_used: u64,
        memory_used_bytes: u64,
    },
    /// 自定义消息
    Custom {
        event: String,
    },
}
```

### 3.4 ForkContext — 上下文继承

```rust
/// 上下文快照：子代理可继承父代理的运行时状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkContext {
    /// 父代理的路径
    pub parent_path: AgentPath,
    /// 对话历史摘要
    pub conversation_summary: Option<String>,
    /// 活跃的 principles (PUA 规则)
    pub principles: Vec<String>,
    /// 受限的文件路径
    pub allowed_base_dir: Option<PathBuf>,
    /// 可继承的记忆
    pub inherited_memories: Vec<String>,
    /// KV 缓存指纹 (用于 DeepSeek 类模型的前缀缓存)
    pub kv_cache_fingerprint: Option<String>,
}
```

### 3.5 ExecutionGovernor — 执行控制

```rust
/// 执行控制状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentExecutionBudget {
    /// 树总 token 上限 (所有子代理共享)
    pub aggregate_token_ceiling: Option<u64>,
    /// 树当前已用 token
    pub aggregate_tokens_used: u64,
    /// 最大深度
    pub max_depth: u32,
    /// 最大并发子代理数
    pub max_concurrency: usize,
    /// 当前活跃子代理数
    pub active_children: usize,
    /// 总 wall clock 时间上限 (ms)
    pub max_wall_clock_ms: Option<u64>,
    /// 开始时间
    pub started_at_ms: u64,
}
```

---

## 4. AgentTree — 层次化索引

### 4.1 数据结构

```rust
pub struct AgentTree {
    /// 注册表：path → AgentNode (O(1) 查找)
    nodes: HashMap<AgentPath, AgentNode>,
    /// 根节点缓存
    root: AgentNode,
}
```

### 4.2 操作

```rust
impl AgentTree {
    /// 注册 agent 到树中
    pub fn register(&mut self, path: &AgentPath, agent_name: &str, metadata: AgentNodeMetadata) -> Result<()>;

    /// 按路径查找
    pub fn resolve(&self, path: &AgentPath) -> Option<&AgentNode>;

    /// 按通配符查找
    pub fn resolve_pattern(&self, pattern: &AgentPathPattern) -> Vec<&AgentNode>;

    /// 获取所有祖先
    pub fn ancestors(&self, path: &AgentPath) -> Vec<&AgentNode>;

    /// 获取所有子孙 (递归)
    pub fn descendants(&self, path: &AgentPath) -> Vec<&AgentNode>;

    /// 从 AgentRegistry 导入扁平注册表
    pub fn import_from_registry(&mut self, registry: &AgentRegistry) -> Result<()>;

    /// 删除子树
    pub fn remove_subtree(&mut self, path: &AgentPath) -> Vec<AgentPath>;
}
```

### 4.3 路径解析规则

```
root/research/coder = AgentPath(["root", "research", "coder"])
     ↓
AgentTree.resolve("root/research/coder")
     ↓
1. 检查 "root" 是否存在 → 是
2. 检查 root 下 "research" 是否存在 → 是
3. 检查 research 下 "coder" 是否存在 → 返回 AgentNode
```

---

## 5. AgentMessenger — 消息路由

### 5.1 核心流程

```
发送消息:
  AgentMessenger::send(msg)
    │
    ├─ 1. 验证 msg.from 在 AgentTree 中存在
    ├─ 2. 按 msg.to 匹配接收者 (支持通配符)
    ├─ 3. 对每个匹配的接收者:
    │     ├─ 如果同进程 → 直接投递到接收者的 inbox
    │     └─ 如果跨进程 → 通过 ProtocolBus 转发
    ├─ 4. 记录到 ObservBus (tracing + metrics)
    └─ 5. 返回发送结果

接收消息:
  AgentMessenger::recv(path) → Vec<AgentMessage>
    │
    ├─ 从 path 对应的 inbox 中取出所有待处理消息
    └─ 按优先级排序返回
```

### 5.2 通道类型

```rust
pub enum AgentChannel {
    /// 直接消息: A → B
    Direct(AgentPath),
    /// 广播: A → 所有子孙
    Broadcast,
    /// 父子: A → A 的父节点
    ToParent,
    /// 通配符: A → 匹配 pattern 的所有节点
    Pattern(AgentPathPattern),
}
```

### 5.3 消息投递保障

```rust
pub enum DeliveryGuarantee {
    /// 最多一次 (fire-and-forget)
    AtMostOnce,
    /// 至少一次 (确认重试)
    AtLeastOnce,
    /// 恰好一次 (去重 + 确认)
    ExactlyOnce,
}
```

---

## 6. ContextForker — 上下文继承

### 6.1 流程

```
父代理 spawn 子代理时:
  ContextForker::fork(parent_path, child_path, options)
    │
    ├─ 1. 收集父代理的当前上下文:
    │     ├─ conversation_summary (最近 N 轮对话摘要)
    │     ├─ principles (PUA 规则)
    │     ├─ allowed_base_dir
    │     └─ kv_cache_fingerprint (如果支持)
    │
    ├─ 2. 创建 ForkContext 快照
    │
    ├─ 3. 将快照传递给子代理作为系统提示的一部分
    │     (子代理收到 "Parent context: ..." 前缀)
    │
    └─ 4. 在 ForkRegistry 中注册 fork 记录
```

### 6.2 KV 缓存优化 (DeepSeek 专用)

```
对于支持前缀缓存的模型 (DeepSeek):
1. 父代理的 KV 缓存前缀被保留在 shared memory 中
2. 子代理 fork 时，可直接引用父代理的 KV 缓存前缀
3. 只需计算子代理新增的 prompt 部分
4. 节省 30-60% 的首 token 延迟
```

---

## 7. ExecutionGovernor — 执行限制器

### 7.1 限制规则

| 限制项 | 检查时机 | 行为 |
|--------|---------|------|
| `max_depth` | spawn 前 | 超限则拒绝 spawn |
| `aggregate_token_ceiling` | 每次 token 产出后 | 超限则停止子代理 |
| `max_concurrency` | spawn 前 | 超限则排队等待 |
| `max_wall_clock_ms` | 每次消息轮询时 | 超限则强制取消 |
| `max_children_per_parent` | spawn 前 | 超限则报错 |

### 7.2 预留模式 (SpawnReservation)

```
父代理: 先 reservation → 确认资源 → 再 spawn
          │                    │
          ▼                    ▼
   Semaphore::acquire()   实际执行 spawn
   (预占一个插槽)         (释放 reserve)
```

---

## 8. 与现有系统的集成

### 8.1 对 Agent trait 的扩展

```rust
#[async_trait]
pub trait Agent: Send + Sync {
    // 现有方法
    async fn chat(&self, ...);
    fn available_models(&self) -> Vec<ModelInfo>;

    // 新增方法 (默认实现基于 CommunicationBus)
    /// 接收一条 agent 间消息
    async fn on_message(&self, msg: AgentMessage) -> Result<Option<AgentMessage>> {
        // 默认实现：记录日志，不做处理
        tracing::info!(from = %msg.from, kind = ?msg.kind, "agent received message");
        Ok(None)
    }

    /// 发送一条消息给另一个 agent
    async fn send_message(&self, messenger: &AgentMessenger, to: AgentPathPattern, kind: AgentMessageKind, payload: Value) -> Result<()>;
}
```

### 8.2 SpawnAgentTool 改造

```rust
// 当前: SpawnAgentTool 直接调用 agent.chat()
// 改造后: SpawnAgentTool 通过 CommunicationBus 的 channel 通信

fn execute_spawn(...) {
    // 1. 通过 CommunicationBus 的 AgentTree 注册子节点
    communication_bus.tree().register(&child_path, &agent_name, metadata)?;

    // 2. 通过 ContextForker 创建上下文快照
    let fork_ctx = communication_bus.forker().fork(&parent_path, &child_path)?;

    // 3. 通过 ExecutionGovernor 获取执行许可
    let permit = communication_bus.governor().reserve(&child_path, &budget)?;

    // 4. 通过 AgentMessenger 发送 Delegate 消息
    let msg = AgentMessage::delegate(&parent_path, &child_path, task, role, budget);
    communication_bus.messenger().send(msg).await?;

    // 5. 通过 AgentMessenger 的 inbox 接收 Result 消息
    let result = communication_bus.messenger()
        .wait_for(&parent_path, |msg| msg.kind.is_result(), timeout)
        .await?;
}
```

### 8.3 ForkRegistry 增强

```rust
// 当前 ForkEntry:
pub struct ForkEntry {
    pub id: String,
    pub parent_task_id: String,
    pub status: ForkStatus,
    pub snapshot: Option<ForkSnapshot>,
}

// 增强后:
pub struct ForkEntry {
    pub id: String,
    pub agent_path: AgentPath,           // 新增: agent 路径
    pub parent_agent_path: AgentPath,    // 新增: 父 agent 路径
    pub parent_task_id: String,
    pub status: ForkStatus,
    pub snapshot: Option<ForkSnapshot>,
    pub budget: AgentExecutionBudget,    // 新增: 执行预算
    pub context: Option<ForkContext>,    // 新增: 上下文快照
    pub started_at_ms: u64,
    pub completed_at_ms: Option<u64>,
}
```

### 8.4 ToolHook 集成

```rust
// 通过 ToolHook 系统挂接 spawn 事件
pub struct AgentCommunicationHook {
    bus: Arc<CommunicationBus>,
}

impl ToolHook for AgentCommunicationHook {
    fn pre_execute(&self, tool_name: &str, input: &ToolInput) -> Result<()> {
        if tool_name == "spawn_agent" {
            // 在工具执行前注册 agent 树节点
            self.bus.tree().register(...)?;
        }
        Ok(())
    }

    fn post_execute(&self, tool_name: &str, input: &ToolInput, output: &ToolOutput, duration_ms: u64) -> Result<()> {
        if tool_name == "spawn_agent" {
            // 在工具执行后记录通信指标
            self.bus.record_metrics(tool_name, duration_ms, output.success)?;
        }
        Ok(())
    }
}
```

---

## 9. 可观测性

### 9.1 Tracing Spans

```rust
// 每个通信操作产生 tracing span
let span = info_span!(
    "agent_communication",
    agent.path = %msg.from,
    agent.to = %msg.to,
    agent.kind = ?msg.kind,
    agent.msg_id = %msg.id,
    agent.priority = msg.priority,
);

// span 包含:
// - agent_communication.send
// - agent_communication.deliver
// - agent_communication.receive
// - agent_communication.fork
// - agent_communication.cancel
```

### 9.2 Metrics

```rust
// Prometheus/OTLP 指标
metrics::counter!("agent_communication.messages_sent_total", "type" => kind);
metrics::counter!("agent_communication.messages_received_total", "type" => kind);
metrics::histogram!("agent_communication.delivery_latency_ms", "from" => from, "to" => to);
metrics::gauge!("agent_communication.active_agents", "path" => path);
metrics::counter!("agent_communication.forks_total");
metrics::counter!("agent_communication.cancellations_total");
metrics::histogram!("agent_communication.fork_context_size_bytes");
```

---

## 10. 实现计划

### Phase 1: 核心类型 (2-3 天)

| 任务 | 文件 | 产出 |
|------|------|------|
| `AgentPath` + 解析器 | `src/agents/communication/path.rs` | ✅ 路径解析与匹配 |
| `AgentMessage` 类型 | `src/agents/communication/message.rs` | ✅ 消息类型枚举 |
| `ForkContext` 类型 | `src/agents/communication/context.rs` | ✅ 上下文快照 |
| `AgentExecutionBudget` | `src/agents/communication/budget.rs` | ✅ 预算类型 |

### Phase 2: AgentTree (2-3 天)

| 任务 | 文件 | 产出 |
|------|------|------|
| `AgentNode` + `AgentTree` | `src/agents/communication/tree.rs` | ✅ 层次化索引 |
| `import_from_registry()` | `tree.rs` | ✅ 从现有注册表导入 |
| 通配符模式匹配 | `tree.rs` | ✅ pattern 解析 |

### Phase 3: CommunicationBus (3-4 天)

| 任务 | 文件 | 产出 |
|------|------|------|
| `CommunicationBus` Builder + Profile | `src/agents/communication/bus.rs` | ✅ Bus 骨架 |
| `AgentMessenger` 路由 | `src/agents/communication/messenger.rs` | ✅ 消息收发 |
| `ContextForker` 实现 | `src/agents/communication/forker.rs` | ✅ 上下文继承 |
| `ExecutionGovernor` 实现 | `src/agents/communication/governor.rs` | ✅ 执行控制 |
| `CommunicationHealth` | `bus.rs` | ✅ health 端点 |
| metrics + tracing | `bus.rs` | ✅ 可观测性 |

### Phase 4: 集成 (2-3 天)

| 任务 | 文件 | 产出 |
|------|------|------|
| `SpawnAgentTool` 改造 | `spawn_agent.rs` | ✅ 使用 CommunicationBus |
| `Agent` trait 扩展 | `agent.rs` | ✅ on_message/send_message |
| `AgentCommunicationHook` | `types.rs` | ✅ ToolHook 集成 |
| `ForkRegistry` 增强 | `fork_registry.rs` | ✅ agent 路径字段 |
| `server_builder.rs` 接线 | `server_builder.rs` | ✅ 初始化 |
| 全量测试 | `tests/` | ✅ 2068 测试通过 |

---

## 11. 兼容性保证

1. **不破坏现有 Agent trait** — `chat()` 方法签名不变，新增方法有默认实现
2. **不破坏 SpawnAgentTool 的 Tool trait** — 现有 `run()`/`run_async()` 路径保留
3. **不破坏 ForkRegistry 的现有 API** — 新增字段可选，向后兼容反序列化
4. **不破坏 14-bus 架构** — CommunicationBus 作为第 15 个 Bus 独立接入
5. **不破坏 principle.md** — 所有方法使用 `try_current()` + fallback，零 block_on

---

## 12. 最终验证标准

```
cargo check                      ✅ 零错误
cargo clippy --all-targets -D warnings  ✅ 零警告
cargo test --lib                 ✅ 2068 passed, 0 failed, 0 ignored

# 新增通信测试
cargo test --lib agent_communication  ✅ 全部通过
cargo test --lib agent_tree          ✅ 全部通过
cargo test --lib agent_messenger     ✅ 全部通过

# Profile 全链路
cargo build --no-default-features --features simple-server  ✅
cargo build --no-default-features --features multi-users-server  ✅
cargo build --no-default-features --features full  ✅
```
