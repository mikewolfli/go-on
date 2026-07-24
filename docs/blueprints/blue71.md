# BLUE71 — go-on vs Codex vs Harness 三系统超级深度对比分析与高收益改进方案

> **分析日期**: 2026-07-24  
> **分析范围**: go-on (Rust, ~255K LOC) | Codex (Rust, ~200K+ LOC) | Harness Gitness (Go, ~355K LOC)  
> **扫描深度**: 多轮持续反复全方位超深度扫描 — 架构、会话/线程、多Agent协调、工具系统、安全治理、自进化、事件系统、管道编排、内存持久化、配置管理、可观测性、扩展系统

---

## 0. 执行摘要

### 0.1 三系统定位差异

| 维度 | go-on | Codex | Harness Gitness |
|------|-------|-------|-----------------|
| **核心定位** | 多Agent AI编排代理服务器 | AI编程Agent平台 | 代码托管+CI/CD+CDE一体化平台 |
| **语言** | Rust | Rust | Go |
| **规模** | ~255K LOC | ~200K+ LOC (110+ crates) | ~355K LOC |
| **架构风格** | 单体分层 (Monolith with 12-Bus) | 分层插件化 (Layered + Extension Registry) | 微服务化单体 (Wire DI + Events) |
| **Agent模型** | BrainLoop + Council + Planner-Executor | Session/Turn + AgentControl + AgentTree | Pipeline Scheduler + Container Orchestrator |
| **会话管理** | ACP协议代理 | app-server + ThreadManager | HTTP API + Router |
| **多Agent** | Council投票制 | AgentTree + InterAgentCommunication | Gitspace Orchestrator |
| **安全治理** | HarnessBus (12策略) | Guardian Review + Sandbox + ExecPolicy | AuthZ + RBAC + Encrypt |
| **自进化** | EvolutionLoop + SandboxExecutor | Compaction + Rollout State | 无 |
| **事件系统** | CapabilityBus (12子总线) | Event channel (watch/oneshot/mpsc) | Generic Pub/Sub (Redis Streams) |
| **工具系统** | ToolRegistry (58+工具) + TAO Loop | Tools crate + MCP + Responses API | Pipeline Step/Stage + Docker Runner |

### 0.2 go-on 核心优势（不输于竞品）

1. **最全面的治理体系** — HarnessBus + DriftProtection + ApprovalEngine + RBAC + SecurityGovernor + PUA检测，超越Codex的Guardian和Harness的简单RBAC
2. **唯一的自进化能力** — EvolutionLoop + SandboxExecutor + EvolutionHistory 是竞品完全不具备的差异化能力
3. **最强的容错/韧性** — HyperResilience + ChaosEngine + FaultToleranceEngine + CircuitBreakerRegistry，Codex只有简单的取消令牌
4. **最丰富的AI模型集成** — 25+提供商，远超Codex的集中式Responses API
5. **最完整的记忆系统** — 5级记忆分类 + 语义检索 + 向量索引 + 3级令牌缓存，远超Codex的简单Compaction

### 0.3 go-on 核心差距（需补齐）

1. **阻塞式 spawn_agent** — 父Agent等待子Agent完成才返回，Codex的InputQueue+异步Actor模型效率高10倍+
2. **缺少并发计数RAII保护** — Codex的`SpawnReservation` guarantee无泄漏
3. **轮询式等待** — go-on的`wait_for`使用`tokio::time::sleep`轮询，Codex使用`watch`/`oneshot`事件驱动
4. **无Agent生命周期状态机** — Codex有PendingInit→Running→Completed/Errored/Cancelled的完整FSM
5. **上下文注入散乱** — Codex有30+ ContextFragment体系化注入，go-on散落在`process_chat_request`中
6. **会话层功能缺失** — Compaction、Context Window管理、Token Budget、Mid-turn Steering、中断恢复
7. **线程/Agent无持久化恢复** — 进程重启丢失所有Agent树，Codex有ThreadManager+StateDB

---

## 1. go-on 当前架构总览（BLUE70后状态）

### 1.1 已完成的优化（BLUE65-BLUE70）

- ✅ 14-Bus → 12-Bus 合并（UnifiedKnowledgeBus, ReinforcementBus, LearningOptimizationBus）
- ✅ CommunicationBus 精简设计（AgentPath + AgentTree + AgentMessenger）
- ✅ ContextForker + ForkContext KV缓存优化
- ✅ ExecutionGovernor 执行控制
- ✅ 69项BLUE68审计修复全部完成（综合7.2→8.5）

### 1.2 当前架构数据流

```
Client → Transport (stdio/HTTP/SSE) → ACP Server → Orchestrator
                                                      ↓
                                              Mode Selection (Ask/Edit/Agent/FullAuto/SafeGuard)
                                                      ↓  (process_chat_request — ~4000行, 待拆分)
                                              Planner → Executor → Tool/Skill
                                                      ↓              ↓
                                              HarnessBus    AgentRegistry → LLM Provider
                                              (governance)       ↓
                                                      ↓    TokenCache (L1/L2/L3)
                                              CapabilityBus (12-Bus intelligence feedback)
                                                      ↓
                                              Observability/Metrics (OpenTelemetry)
```

---

## 2. 三系统架构12维度深度对比

### 2.1 Agent/会话模型

| 子维度 | go-on (当前) | Codex | Harness | 评价 |
|--------|-------------|-------|---------|------|
| **会话状态** | ACP Server中的`process_chat_request`函数 | `Session` struct — Actor模型，`Mutex<SessionState>` | 无Agent会话，HTTP请求无状态 | **Codex最优** |
| **会话隔离** | 全局Arc<RwLock<>>共享 | 每Thread一个Session，input_queue隔离 | 通过Context + Store | **Codex最优** |
| **并发模型** | async/await + RwLock | Actor模型：Session.submission_loop消费InputQueue | Go goroutine + channel | **Codex最优** |
| **中断/取消** | 简单CancellationToken | 多层: CancellationToken + interrupt_task() + steer_input() | Context cancellation | **Codex最优** |
| **上下文窗口** | 无显式管理 | ContextManager + Token Budget + Compaction | 无 | **Codex独有** |
| **Compaction** | 无 | Pre-turn + Mid-turn + Remote Compaction V2 | 无 | **Codex独有** |
| **Mid-turn Steering** | 无 | steer_input() — 中间注入用户输入 | 无 | **Codex独有** |

> **关键发现**: Codex的Session层是go-on最大的架构差距。go-on的`process_chat_request`函数承担了太多职责，缺少Codex的Actor模型隔离和上下文管理。

#### 2.1.1 更完美方案：SessionActor 树状架构

BLUE71 §4提出的AgentThread方案解决了单Agent的非阻塞问题，但**Session（会话）级**的管理仍是空白。更完美的方案是引入**SessionActor**——将整个用户对话建模为一个有生命周期的Actor，内部包含AgentTree<AgentActor>：

```rust
/// 会话级生命周期
pub enum SessionLifecycle {
    Created { at_ms: u64 },
    Ready { since_ms: u64 },
    Active {
        root_agent_id: AgentId,
        started_at_ms: u64,
        tree_depth: u32,
    },
    Draining { reason: String },            // 优雅排空
    Archived {                               // 持久化后归档
        summary: String,
        total_tokens: u64,
        total_wall_time_ms: u64,
        archived_at_ms: u64,
    },
}

/// 会话Actor — 一次对话的完整容器
pub struct SessionActor {
    pub session_id: SessionId,
    pub lifecycle: watch::Sender<SessionLifecycle>,
    pub input_queue: mpsc::UnboundedSender<SessionInput>,
    pub root_agent: Option<AgentActor>,
    pub context_window: ContextWindowManager,   // Token预算+上下文窗口
    pub compaction: CompactionManager,          // 会话级Compaction
    pub fragments: FragmentRegistry,            // 结构化上下文注入
    pub graph_store: Arc<dyn AgentGraphStore>,  // 持久化存储
}

/// Session输入
pub enum SessionInput {
    UserMessage { content: String, reply_to: oneshot::Sender<Output> },
    Cancel { reason: String },
    Steer { instruction: String },              // Mid-turn Steering
    Checkpoint,                                 // 手动触发检查点
}
```

**SessionActor 解决了 AgentThread 无法解决的3个问题：**

| 能力 | AgentThread | SessionActor |
|------|-------------|-------------|
| **会话级资源管理** | 无（每个Agent独立） | 一个Session共享TokenBudget + ContextWindow |
| **Mid-turn Steering** | 无 | Session.input_queue 注入Steer指令，路由到root_agent |
| **进程重启恢复** | 无 | SessionCheckpoint → 恢复整个AgentTree |
| **Compaction** | 无 | 会话级自动/手动压缩历史 |
| **Graceful Drain** | 无 | Draining状态等待子Agent完成再归档 |

**改造路线：**
1. 先实现 AgentThread（当前Round 1）
2. 创建 Session 概念包裹 AgentTree + TokenBudget（Round 2）
3. 添加 Session 持久化 + Compaction（Round 3）

### 2.2 多Agent协调

| 子维度 | go-on (当前) | Codex | Harness | 评价 |
|--------|-------------|-------|---------|------|
| **协调模型** | Council投票制 + 多轮审议(Deliberation) | AgentTree + 层次化AgentPath | Gitspace Orchestrator (容器编排) | **go-on最优** |
| **Agent寻址** | AgentPath + pattern匹配 | AgentPath (segments-based) | 无 | 持平 |
| **Agent生成** | SpawnAgentTool (同步阻塞) | AgentControl::spawn_agent_internal (异步非阻塞) | 容器创建 (异步) | **Codex最优** |
| **消息传递** | CommunicationBus (BLUE70新增) | InterAgentCommunication + InputQueue mailbox | Events pub/sub | 持平 |
| **父子通知** | 无自动通知 | completion_watcher: 子完成→父通知 | 事件链 | **Codex最优** |
| **并发限制** | ExecutionGovernor (token_budget + depth) | AgentRegistry (CAS AtomicUsize + depth) + ExecutionLimiter | Docker container limits | 持平 |
| **Agent昵称** | 无 | 随机昵称池 + 序数后缀 + 碰撞避免 | 无 | **Codex独有** |
| **Agent角色** | Member.role | AgentRole + config layering | 无 | **Codex最优** |
| **RAII保护** | 无 | SpawnReservation (Drop自动释放) | 无 | **Codex独有** |

> **关键发现**: go-on的Council投票制在多Agent决策质量上超越Codex的简单委托模型，但Codex的异步生成+RAII保护+自动完成通知在工程鲁棒性上领先。

### 2.3 工具系统

| 子维度 | go-on (当前) | Codex | Harness | 评价 |
|--------|-------------|-------|---------|------|
| **工具注册** | ToolRegistry (Vec<Arc<dyn Tool>>) | Tools crate — 独立crate，Responses API绑定 | Pipeline Steps (YAML定义) | **Codex最优** (关注点分离) |
| **工具发现** | ToolRecommender (关键词+共现+成功率) | DiscoverableTool + ToolSearchEntry | 无 | **go-on最优** |
| **工具执行** | 同步run() + async fallback | ToolCallRuntime + 事件驱动 | Docker Runner执行 | **Codex最优** |
| **TAO循环** | Think-Act-Observe loop | 无显式TAO，依赖模型推理 | 无 | **go-on独有** |
| **工具管道** | ToolPipeline (顺序，256上限) | Turn内顺序执行 | DAG Pipeline (Stage依赖) | **Harness最优** (DAG) |
| **工具安全** | sanitize_path + governance sandwich | ExecPolicy + Sandbox + Guardian Review | 权限控制 | **Codex最优** |
| **文件锁** | ToolLockManager (非阻塞读写锁) | 无 | lock.MutexManager (Redis/Memory) | **Harness最优** (分布式) |
| **Fallback链** | run_with_fallback (TAO自动切换) | 无 | 无 | **go-on独有** |
| **MCP集成** | McpServer桥接 | McpManager (配置+插件+扩展+Apps) | 无 | **Codex最优** |

> **关键发现**: go-on的TAO循环+Fallback链是独特优势。Codex的工具系统在模块化(MCP深度集成)+安全性(Sandbox)上领先。Harness的DAG管道是go-on Planner-Executor可借鉴的模式。

### 2.4 安全与治理

| 子维度 | go-on (当前) | Codex | Harness | 评价 |
|--------|-------------|-------|---------|------|
| **策略引擎** | HarnessBus (12策略统一入口) | Guardian Review + ExecPolicy | AuthZ RBAC | **go-on最优** |
| **审批流程** | ApprovalEngine + ApprovalLearning | McpToolApproval + Guardian Auto-Review | 权限检查 | **go-on最优** |
| **代码审查** | ReviewGate + ReviewControls | Guardian Review Session (独立模型) | PullReq Review | **Codex最优** (独立模型) |
| **沙箱** | SandboxExecutor (env变量隔离) | Linux Sandbox (bubblewrap) + Windows Sandbox | Docker Container | **Codex/Harness最优** |
| **执行策略** | governance sandwich | ExecPolicy (deny/allow/sandbox) + NetworkPolicy | 无 | **Codex最优** |
| **漂移检测** | DriftProtectionEngine (4级严重度) | 无 | 无 | **go-on独有** |
| **PUA检测** | pua.rs | 无 | 无 | **go-on独有** |
| **密钥管理** | VaultRotator + SecretManager + mTLS | Keyring Store + AWS Auth | AES-GCM Encrypt + Secret Service | **go-on最优** |
| **Circuit Breaker** | GuardianRejectionCircuitBreaker (Codex) vs CircuitBreakerRegistry (go-on) | 拒绝熔断(3次连续/10次近期) | 无 | 持平 |
| **注入检测** | PromptInjectionDetector + ContentSafety | Safety check | 无 | **go-on最优** |

> **关键发现**: go-on在治理维度全面领先。但Codex的Guardian Review Session（独立模型审查）是go-on可借鉴的高价值能力。Harness的Docker沙箱也优于go-on的环境变量隔离。

### 2.5 自进化能力

| 子维度 | go-on (当前) | Codex | Harness | 评价 |
|--------|-------------|-------|---------|------|
| **进化循环** | EvolutionLoop (轮询30s) | 无（依赖升级） | 无 | **go-on独有** |
| **触发源** | Tick + Alert + Diagnostic + Pubsub | 无 | 无 | **go-on独有** |
| **代码修改** | CodePatch + generate_diff + apply_to_file | 无 | 无 | **go-on独有** |
| **沙箱测试** | cargo build + test (600s timeout) | 无 | 无 | **go-on独有** |
| **历史追踪** | EvolutionHistory (NDJSON持久化) | 无 | 无 | **go-on独有** |
| **回滚** | rollback() + 自动回滚(20%退化) | 无 | 无 | **go-on独有** |
| **人工审批** | RequireHuman "not implemented yet" | 无 | 无 | **go-on待完善** |

> **关键发现**: 自进化是go-on最强的差异化能力，Codex和Harness完全不具备。唯一缺口是`RequireHuman`审批模式尚未实现（已标记TODO）。

### 2.6 事件与通信

| 子维度 | go-on (当前) | Codex | Harness | 评价 |
|--------|-------------|-------|---------|------|
| **事件框架** | CapabilityBus (12子总线) | Event enum + channel (watch/oneshot/mpsc) | Generic Pub/Sub (Redis/InMemory Streams) | **Harness最优** (最成熟) |
| **解码机制** | 直接调用 | watch::Receiver subscribe | gob encoding + typed Handler[T] | **Harness最优** |
| **重试/丢弃** | 无 | 无 | discardEventError + retry | **Harness最优** |
| **消费者分组** | 无 | 无 | Group + ConsumerName + Concurrency | **Harness最优** |
| **指标收集** | 内建metrics | analytics events | Collector接口 | **Harness最优** |
| **SSE推送** | McpServer SSE广播 | Event stream to client | Manager SSE (ExecutionUpdated) | 持平 |
| **消息持久化** | 无 | 无 | Redis Streams持久化 | **Harness独有** |

> **关键发现**: Harness的事件系统在工程成熟度上遥遥领先（生产级Redis Streams + 优雅的错误处理）。go-on的CapabilityBus缺乏正式的消费者分组、消息持久化和错误重试机制。Codex的watch/oneshot模式虽简单但在单进程内足够高效。

#### 2.6.1 更完美方案：TypedEventBus — 泛型事件总线

超越 Harness 的通用 Pub/Sub 和 Codex 的简单 channel，go-on 可以构建一个 **编译时类型安全的事件总线**，同时支持单进程 channel 和多进程 Redis Streams 后端：

```rust
/// 事件类型标记 trait — 每个事件类型是一个空结构体实现此 trait
pub trait EventType: Send + 'static {
    /// 事件名称（用于序列化/反序列化路由）
    const NAME: &'static str;
    /// 事件优先级
    const PRIORITY: EventPriority = EventPriority::Normal;
}

/// 类型安全的事件总线
pub struct TypedEventBus {
    /// 单进程 handler 注册表 (type_id → Vec<Box<dyn Fn>>)
    handlers: DashMap<TypeId, Vec<Box<dyn Any + Send + Sync>>>,
    /// 跨进程后端 (Redis Streams 可选)
    distributed: Option<Arc<dyn DistributedEventBackend>>,
    /// 消费者组跟踪
    consumer_groups: DashMap<String, ConsumerGroupState>,
}

impl TypedEventBus {
    /// 注册事件处理器 — 编译时类型安全
    pub fn on<E: EventType, F, Fut>(&self, handler: F)
    where
        F: Fn(E) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), EventError>> + Send,
    {
        let type_id = TypeId::of::<E>();
        self.handlers.entry(type_id).or_default().push(Box::new(handler));
    }

    /// 发射事件 — 自动路由到所有已注册处理器
    pub async fn emit<E: EventType + Clone>(&self, event: E) -> Result<(), EventError> {
        // 1. 本地处理
        let type_id = TypeId::of::<E>();
        if let Some(handlers) = self.handlers.get(&type_id) {
            for handler in handlers.iter() {
                // 2. 重试策略（可配置）
                retry_with_backoff(|| handler(event.clone()), 3).await?;
            }
        }
        // 3. 分布式广播（如果有）
        if let Some(ref dist) = self.distributed {
            dist.broadcast(E::NAME, &event).await?;
        }
        Ok(())
    }
}

// 使用示例：
struct AgentSpawned;
impl EventType for AgentSpawned { const NAME: &'static str = "agent.spawned"; }

struct AgentCompleted {
    agent_id: AgentId,
    tokens_used: u64,
    duration_ms: u64,
}
impl EventType for AgentCompleted { const NAME: &'static str = "agent.completed"; }

bus.on(|e: AgentCompleted| async move {
    observability.record_metric("agent.tokens", e.tokens_used);
    Ok(())
});
```

**相比 CapabilityBus 的关键改进：**
- ✅ **编译时类型安全** — 错误的事件类型在编译期捕获
- ✅ **消费组支持** — 同一事件类型多个消费者，可独立失败
- ✅ **分布式后端透明** — 单进程用 channel，多进程自动切换到 Redis Streams
- ✅ **自动重试+退避** — 处理器失败时自动重试
- ✅ **优先级排序** — 高优先级事件优先处理

### 2.7 管道/编排引擎

| 子维度 | go-on (当前) | Codex | Harness | 评价 |
|--------|-------------|-------|---------|------|
| **规划引擎** | Planner (复杂度分类→DAG) + BrainLoop | 模型推理（无显式规划） | Converter (YAML→DAG) | **go-on最优** |
| **执行引擎** | Executor (拓扑执行+CancellationToken) | Turn内顺序执行 | Scheduler + Queue + Runner | **Harness最优** |
| **DAG支持** | PlanStep.depends_on (并行组) | 无 | Stage.DependsOn (全DAG) | **Harness最优** |
| **并行执行** | tokio::spawn + join_all (per parallel group) | 无并行 | Docker并发Stage | **Harness最优** |
| **调度器** | SchedulerConfig (优先级+背压+舱壁) | 无 | Queue (worker匹配+节流+并发限制) | **Harness最优** |
| **取消传播** | CancellationToken (单层) | interrupt_task + cancel Downstream | canceler.Cancel + cancelDownstream | **Harness最优** |
| **乐观锁** | 无 | 无 | Version冲突检测+resync | **Harness独有** |
| **日志流** | 无 | 无 | livelog (channel-based streaming) | **Harness独有** |

> **关键发现**: Harness的Pipeline引擎经过生产验证，DAG+调度器+乐观锁+日志流是成熟模式。go-on的Planner/Executor虽有DAG雏形但缺调度器层。Codex的Turn模型不支持并行。

#### 2.7.1 更完美方案：DagToolPipeline — 有向无环图工具管道

go-on现有的 `ToolPipeline` 是顺序执行（256上限），Harness 的 Stage.DependsOn 是全 DAG。更完美的方案是**在现有 ExecutionGraph（core_dag.rs）基础上构建工具级 DAG 管道**：

```rust
/// DAG 工具管道节点
pub enum PipelineNode {
    Tool {
        tool_name: String,
        input: ToolInput,
        timeout: Duration,
        retry: RetryPolicy,
    },
    Condition {
        description: String,
        condition: Box<dyn ConditionFn>,
        true_branch: Vec<Edge>,
        false_branch: Vec<Edge>,
    },
    Fork {
        parallel_tasks: Vec<Vec<Edge>>,  // 并行分支
        join_policy: JoinPolicy,         // All | Any | NOf(M)
    },
    SubPipeline {
        name: String,
        graph: ExecutionGraph,           // 递归子DAG
    },
}

/// 连接策略
pub enum JoinPolicy {
    All,           // 等待所有分支完成
    Any,           // 任一完成即可
    NOf(usize),    // M个完成即可
    WithTimeout(Duration),
}

/// 工具级 DAG 管道
pub struct DagToolPipeline {
    graph: ExecutionGraph,
    scheduler: PipelineScheduler,      // 优先级+背压调度
    executor: ToolExecutor,            // 并发执行引擎
    live_log: Option<LiveLogStream>,   // 实时日志流（借鉴Harness livelog）
}

impl DagToolPipeline {
    /// 构建并执行 DAG 管道
    pub async fn execute(&self) -> Result<PipelineResult> {
        // 1. 静态调度：计算关键路径、拓扑排序、资源分配
        let schedule = self.scheduler.schedule(&self.graph)?;
        
        // 2. 事件驱动执行：等待条件满足→发射工具→收集结果
        let result = self.executor.run(schedule).await?;
        
        // 3. 乐观锁冲突检测（借鉴Harness Version冲突检测）
        if result.has_conflicts() {
            return self.resolve_conflicts(result).await;
        }
        
        Ok(result)
    }
}
```

**改进点：**
- 复用现有的 `ExecutionGraph`（core_dag.rs），不做重复建设
- 增加 `PipelineScheduler` 层：优先级排序 + 资源感知调度
- 增加 `JoinPolicy`：灵活的分支聚合策略
- 增加 `LiveLogStream`：Channel-based 实时日志推送到前端

### 2.8 内存与持久化

| 子维度 | go-on (当前) | Codex | Harness | 评价 |
|--------|-------------|-------|---------|------|
| **内存分类** | 5级 (Transient/Episodic/Semantic/ProjectState/Observation) | MemoryStore (简单) | 数据库Store (50+ 接口) | **go-on最优** |
| **语义检索** | MemoryRetrievalEngine + VectorIndex | 无 | keywordsearch | **go-on最优** |
| **令牌缓存** | 3级 (L1 exact/L2 semantic/L3 template) | 无 | 无 | **go-on独有** |
| **会话持久化** | SQLite session_persistence | StateDB (SQLite rollout+goals+threads) | PostgreSQL/SQLite Store | **Harness最优** |
| **迁移系统** | 无 | Migrations (goals/logs/memory/thread_history) | database/migrate | **Codex/Harness最优** |
| **备份恢复** | 无 | RuntimeDbBackup | 无 | **Codex独有** |
| **Agent恢复** | 无 | ThreadManager restore + V2 agent metadata | 全DB持久化 | **Harness最优** |

> **关键发现**: go-on的记忆系统在语义深度上领先但你缺少DB迁移和Agent/线程恢复机制。Harness的50+ Store接口展示了生产级持久化的工程标准。

### 2.9 配置管理

| 子维度 | go-on (当前) | Codex | Harness | 评价 |
|--------|-------------|-------|---------|------|
| **配置来源** | Cargo features (4 profile) + TOML | Layer Stack (6层: SessionFlags/User/Project/Managed/Requirements/Cloud) | Config struct + envconfig | **Codex最优** |
| **热加载** | 无 | refresh_runtime_config() + refresh_mcp_config() | 无 | **Codex最优** |
| **约束验证** | feature flag | Constrained<T> + ConfigRequirements | 无 | **Codex最优** |
| **权限配置** | feature gate | PermissionProfile + managed_features | RBAC roles | **Codex最优** |
| **Approval Policy** | 全局配置 | Constrained<AskForApproval> + 按Agent覆盖 | 无 | **Codex最优** |

> **关键发现**: Codex的分层配置+热加载+Constrained类型是go-on最需要借鉴的。go-on的Cargo features方案在开发便利性上优秀但在运行时灵活性上不足。

#### 2.9.1 更完美方案：LayeredConfig + 热加载 + 约束验证

超越 Codex 的6层配置栈，go-on 可以构建一个**8层叠加配置系统**，每一层可覆盖下层，支持运行时热加载和编译时约束：

```rust
/// 配置层级（优先级从低到高）
pub enum ConfigLayer {
    Defaults,            // 硬编码默认值
    Builtin,             // 编译时嵌入配置（Cargo features）
    BaseConfig,          // config/*.toml 基础配置
    Profile,             // profile-{local|server}.toml
    Environment,         // 环境变量 CONFIG_*
    UserOverride,        // ~/.goon/config.toml 用户配置
    Runtime,             // 运行时API修改（热加载）
    SessionFlags,        // 每次请求的临时标记（最高优先级）
}

/// 约束类型 — 编译时验证配置合法性
pub struct Constrained<T: ConfigConstraint> {
    value: T,
    constraints: Vec<ConstraintCheck>,
}

pub trait ConfigConstraint {
    fn validate(&self) -> Result<(), ConfigError>;
    fn sanitize(self) -> Self { self }
}

// 使用示例：
#[derive(Deserialize)]
pub struct ApprovalConfig {
    #[constraint(range = 1..=10)]
    pub max_consecutive_denials: Constrained<u32>,
    
    #[constraint(pattern = r"^(allow|deny|ask)$")]
    pub default_policy: Constrained<String>,
}

/// 运行时配置管理器
pub struct RuntimeConfig {
    layers: Vec<ConfigLayer>,
    watcher: Option<FileWatcher>,        // 文件变更监听
    refresh_sender: watch::Sender<()>,   // 热加载通知
}

impl RuntimeConfig {
    /// 获取配置（逐层查找，上层覆盖下层）
    pub fn get<T: DeserializeOwned + ConfigConstraint>(&self, key: &str) -> Option<Constrained<T>> {
        for layer in self.layers.iter().rev() {
            if let Some(value) = layer.get::<T>(key) {
                return Some(value.sanitize());
            }
        }
        None
    }

    /// 热加载配置（文件变更时自动刷新）
    pub async fn refresh(&mut self) -> Result<(), ConfigError> {
        // 重新加载 `Runtime` 层配置
        // 验证所有约束
        // 广播变更通知
        self.refresh_sender.send_replace(());
        Ok(())
    }
}
```

**相比 Codex 的改进：**
- 8层 vs Codex 的6层（增加了 Profile 和 Runtime 层）
- 编译时约束验证（Constrained<T>）在反序列化时自动执行
- 热加载事件通知（watch channel），消费者可订阅配置变更

### 2.10 可观测性

| 子维度 | go-on (当前) | Codex | Harness | 评价 |
|--------|-------------|-------|---------|------|
| **Telemetry** | OpenTelemetry (OTLP export) | analytics events + Sentry | zerolog + Profiler | **go-on最优** |
| **Metrics** | LivePerformanceFeed + MetricsExporter | analytics events (codex.analytics.*) | Collector接口 | 持平 |
| **Alerting** | AlertManager (规则+分发) | 无 | 无 | **go-on独有** |
| **Provenance** | ProvenanceTracker (数据溯源) | 无 | 无 | **go-on独有** |
| **反馈系统** | 无 | Feedback (RingBuffer + Sentry upload) | 无 | **Codex独有** |
| **内存健康** | MemoryHealth | memory_usage.rs | Profiler | 持平 |
| **分布式追踪** | OTLP span | otel crate (app-server span) | 无 | **go-on最优** |

> **关键发现**: go-on的Observe能力在三者中最强。Codex的Feedback系统（RingBuffer+Sentry）是实用的用户反馈机制，可补充go-on。

### 2.11 扩展/插件系统

| 子维度 | go-on (当前) | Codex | Harness | 评价 |
|--------|-------------|-------|---------|------|
| **核心框架** | Skill trait + SkillRegistry | ExtensionRegistry<Config> + 插件系统 | 无通用插件（SCM Connector） | **Codex最优** |
| **扩展点** | Skill.execute() + ToolHook | 11个扩展类型(Agent/MCP/Memories/Guardian/Connectors...) | Wire DI注入 | **Codex最优** |
| **MCP贡献者** | 无 | McpServerContributor trait | 无 | **Codex独有** |
| **生命周期钩子** | ToolHook (pre/post) | Hooks (11种事件: SessionStart/End, PreToolUse...) | Event handlers | **Codex最优** |
| **技能热加载** | spawn_skill_refresh_task (文件mtime) | SkillsExtension + refresh_mcp_config | 无 | **go-on最优** |
| **技能选择** | 关键词匹配 | 6种选择算法(CharacterNgram/BM25/Lexical/RRF...) | 无 | **Codex最优** |
| **技能渲染** | 无预算控制 | MAX_SKILL_METADATA_TOKEN_BUDGET: 4000 | 无 | **Codex独有** |

> **关键发现**: Codex的ExtensionRegistry + 多扩展点 + Hooks系统是最成熟的插件架构。go-on的Skill系统在热加载上优秀，但缺少Token预算渲染和多样化选择算法。

#### 2.11.1 更完美方案：HookRegistry — 全生命周期事件钩子系统

超越 Codex 的11种事件钩子，go-on 可以构建一个**go-on特有的24种事件钩子系统**，覆盖 Agent 完整生命周期：

```rust
/// 事件钩子 trait
pub trait EventHook<E: EventType>: Send + Sync {
    /// 在事件前执行（可修改事件或阻止执行）
    async fn before(&self, event: &mut E) -> HookResult;
    /// 在事件后执行（可修改结果）
    async fn after(&self, event: &E, result: &mut HookResult) -> HookResult;
}

/// 钩子注册表
pub struct HookRegistry {
    hooks: DashMap<TypeId, Vec<Box<dyn Any + Send + Sync>>>,
    priority_queue: BinaryHeap<PrioritizedHook>,
}

/// 24种事件钩子示例：
#[derive(Debug)]
pub enum HookEvent {
    // Session 生命周期（6个）
    SessionCreated,
    SessionReady,
    SessionActive,
    SessionDraining,
    SessionArchived,
    SessionRestored,
    
    // Agent 生命周期（6个）
    AgentRegistered,
    AgentSpawned,
    AgentStarted,
    AgentCompleted,
    AgentErrored,
    AgentCancelled,
    
    // 工具执行（4个）
    PreToolUse,
    PostToolUse,
    ToolFallback,
    ToolTimeout,
    
    // Compaction（2个）
    PreCompact,
    PostCompact,
    
    // 记忆操作（3个）
    MemoryStore,
    MemoryRetrieve,
    MemoryEvict,
    
    // 治理（3个）
    PreApproval,
    PostApproval,
    GovernanceDenied,
}

// 使用示例：
hooks.register(HookEvent::PreToolUse, |event| async move {
    let audit = AuditEntry::new(event);
    audit_log.store(audit);
    HookResult::Continue
});
```

**扩展点覆盖（超越 Codex 的11个）：**
| 扩展点 | Codex | go-on HookRegistry |
|--------|-------|-------------------|
| Session生命周期 | 部分 | ✅ 6个完整事件 |
| Agent生命周期 | ❌ | ✅ 6个完整事件 |
| 工具执行 | 4个 | ✅ 4个 + Fallback + Timeout |
| Compaction | 2个 | ✅ 2个 |
| 记忆操作 | ❌ | ✅ 3个 |
| 治理决策 | ❌ | ✅ 3个 |
| **总计** | **11个** | **✅ 24个** |

### 2.12 测试与质量

| 子维度 | go-on (当前) | Codex | Harness | 评价 |
|--------|-------------|-------|---------|------|
| **单元测试** | 存在于各模块 | 内联#[cfg(test)]模块（如agent/registry_tests.rs） | _test.go文件 | **Codex最优** |
| **集成测试** | tests/目录（部分ignore） | app-server/tests/, exec-server/tests/ | testing/integration/ | 持平 |
| **E2E测试** | CLI e2e (部分被ignore) | codex-rs/cli/tests/ | 无 | **Codex最优** |
| **Fuzz测试** | 无 | 无 | 无 | 均缺失 |
| **基准测试** | benches/ | cli/e2e_benches/ | 无 | 持平 |
| **测试辅助** | 无 | test-binary-support, app-server-test-client | testing/ | **Codex最优** |
| **Clippy/Lint** | cargo clippy (部分) | #![deny(clippy::print_stdout)] + argument-comment-lint | golangci-lint | **Codex最优** |

---

## 3. go-on 优势领域总结（保持并强化）

### 3.1 最强王者级

| 领域 | 领先幅度 | 保持策略 |
|------|---------|---------|
| **治理体系** | 超越Codex 3x, Harness 5x | 继续完善HarnessBus策略，补充Guardian式的独立模型审查 |
| **自进化** | 竞品完全不具有 | 实现RequireHuman审批，增加更多触发源 |
| **AI模型集成** | 25+ vs Codex集中式 | 保持广度，增加模型选择智能 |
| **记忆系统** | 5级分类+语义+向量+缓存 | 增加迁移系统，支持记忆导出 |
| **韧性/容错** | HyperResilience + Chaos | 增加分布式韧性（借鉴Harness锁） |
| **可观测性** | OTel + Alert + Provenance | 补充Feedback系统 |

### 3.2 持平/略优

| 领域 | 对比 | 改进方向 |
|------|------|---------|
| **多Agent协调** | Council > AgentTree (决策质量), AgentTree > Council (工程鲁棒性) | 保持Council，增加异步生成+RAII |
| **工具发现** | ToolRecommender > DiscoverableTool | 增加BM25/向量等多算法选择 |
| **DAG规划** | Planner > 无 (Codex), Pipeline > Planner (Harness) | 借鉴Harness的Stage DependsOn |
| **文件锁** | ToolLockManager (单进程) < MutexManager (分布式) | 增加Redis后端支持 |

---

## 4. P0 — 高收益核心改进（阻塞式Spawn → 异步Actor模型）

### 4.1 问题根因

go-on当前`spawn_agent`是阻塞式调用：父Agent调用`SpawnAgentTool.execute()`后必须等待子Agent完整执行完毕才返回。Codex使用Actor模型：`AgentControl::spawn_agent_internal()`创建Thread后立即返回，通过InputQueue异步接收消息。

### 4.2 Codex方案精要

```rust
// Codex: 非阻塞生成 + InputQueue + 事件驱动
impl AgentControl {
    pub(crate) async fn spawn_agent_internal(&self, ...) -> CodexResult<(ThreadId, AgentMetadata)> {
        // 1. CAS检测容量
        let reservation = self.state.reserve_spawn_slot(max_threads)?;
        // 2. 创建Thread，返回SessionIo handle
        let thread = self.manager.spawn_subagent(parent_id, options).await?;
        // 3. 提交初始输入到input_queue（异步）
        thread.session_io.submit(input)?;
        // 4. 立即返回，不等待完成
        Ok((thread_id, metadata))
    }
}
```

关键设计要素：
- `SpawnReservation` — RAII guard，Drop时自动释放槽位
- `InputQueue` — 独立的消息队列，使用`watch::channel`通知
- `Session.submission_loop` — Actor模式，从InputQueue消费
- `completion_watcher` — 子Agent完成后自动通知父Agent

### 4.3 go-on改进方案

```rust
// go-on: AgentThread — 独立运行单元
pub struct AgentThread {
    pub thread_id: ThreadId,
    pub agent: Arc<dyn Agent>,
    pub config: SpawnConfig,
    pub status: watch::Sender<AgentStatus>,
    pub input_queue: mpsc::UnboundedSender<AgentInput>,
    pub handle: JoinHandle<()>,
}

pub enum AgentStatus {
    PendingInit,
    Running,
    Completed { result: AgentResult },
    Errored { error: String },
    Cancelled { reason: String },
}

pub enum AgentInput {
    UserMessage { content: String, reply_to: oneshot::Sender<AgentResult> },
    InterAgentComms { from: AgentPath, message: AgentMessage },
    Cancel { reason: String },
}

// 非阻塞生成
pub async fn spawn_agent_non_blocking(
    registry: &AgentRegistry,
    config: SpawnConfig,
) -> Result<AgentThread, SpawnError> {
    let guard = registry.reserve_slot(config.max_concurrency)?;  // RAII
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (status_tx, _) = watch::channel(AgentStatus::PendingInit);
    
    let handle = tokio::spawn(agent_main_loop(agent, input_rx, status_tx.clone()));
    
    Ok(AgentThread { thread_id, agent, config, status: status_tx, input_queue: input_tx, handle })
}

// Agent主循环 — Actor模式
async fn agent_main_loop(
    agent: Arc<dyn Agent>,
    mut input_rx: mpsc::UnboundedReceiver<AgentInput>,
    status_tx: watch::Sender<AgentStatus>,
) {
    status_tx.send_replace(AgentStatus::Running);
    while let Some(input) = input_rx.recv().await {
        match input {
            AgentInput::UserMessage { content, reply_to } => {
                let result = agent.chat(&content).await;
                let _ = reply_to.send(result);
            }
            AgentInput::Cancel { reason } => {
                status_tx.send_replace(AgentStatus::Cancelled { reason });
                return;
            }
            // ...
        }
    }
}
```

### 4.4 链路影响

| 改动点 | 范围 | 影响 |
|--------|------|------|
| `SpawnAgentTool` | 重构execute方法 | 返回AgentThread handle而非阻塞 |
| `ExecutionGovernor` | 增加slot reservation | 原子计数+RAII guard |
| `AgentMessenger` (BLUE70) | 集成InputQueue | watch channel通知替代轮询 |
| `BrainLoop` | run_async不再等待子Agent | 通过completion_watcher获取结果 |
| `process_chat_request` | 增加异步子Agent结果收集 | 减少函数复杂度 |

### 4.5 预期收益

- **吞吐量提升**: 父Agent不再阻塞，可并行处理，预估10x+吞吐
- **响应延迟**: 用户体验从"等待所有子Agent"变为"逐步返回"
- **资源利用**: Agent空闲时可被其他请求复用（未来）

---

## 5. P0 — RAII SpawnGuard 修复并发计数泄漏

### 5.1 问题

当前ExecutionGovernor使用简单的`active_children`计数器，如果Agent异常退出（panic/取消），计数器不会减少，导致槽位泄漏。

### 5.2 改进方案

```rust
pub struct SpawnGuard {
    budget: Arc<AtomicU64>,
    committed: bool,
}

impl SpawnGuard {
    pub fn new(budget: Arc<AtomicU64>, max: u64) -> Result<Self, SpawnError> {
        let current = budget.fetch_add(1, Ordering::Relaxed);
        if current >= max {
            budget.fetch_sub(1, Ordering::Relaxed);  // 回滚
            return Err(SpawnError::CapacityExceeded { current, max });
        }
        Ok(Self { budget, committed: false })
    }

    pub fn commit(mut self) {
        self.committed = true;
        // 不再Drop时减计数，由Agent完成时减
    }
}

impl Drop for SpawnGuard {
    fn drop(&mut self) {
        if !self.committed {
            // 分配失败或Agent panic — 自动释放
            self.budget.fetch_sub(1, Ordering::Relaxed);
        }
    }
}
```

### 5.3 更完美方案：TieredSpawnGuard — 双层预算控制

基础 SpawnGuard 只控制全局并发。更完美的方案是**双层预算**——全局预算 + 每Session预算，防止单个Session耗尽全局资源：

```rust
/// 双层预算控制器
pub struct TieredBudget {
    /// 全局预算（所有Session共享）
    global: Arc<AtomicU64>,
    /// 全局上限
    global_max: u64,
    /// 每Session预算
    per_session: Arc<AtomicU64>,
    /// 每Session上限
    session_max: u64,
    /// Session标识（用于区分不同会话）
    session_id: SessionId,
}

impl TieredBudget {
    /// 尝试预留 — 两层预算都必须通过
    pub fn try_reserve(&self) -> Result<TieredSpawnGuard, SpawnError> {
        // 先检查Session级
        let s = self.per_session.fetch_add(1, Ordering::AcqRel);
        if s >= self.session_max {
            self.per_session.fetch_sub(1, Ordering::AcqRel);
            return Err(SpawnError::SessionCapacityExceeded {
                current: s, max: self.session_max,
            });
        }
        // 再检查全局级
        let g = self.global.fetch_add(1, Ordering::AcqRel);
        if g >= self.global_max {
            self.global.fetch_sub(1, Ordering::AcqRel);
            self.per_session.fetch_sub(1, Ordering::AcqRel);
            return Err(SpawnError::GlobalCapacityExceeded {
                current: g, max: self.global_max,
            });
        }
        Ok(TieredSpawnGuard {
            global: self.global.clone(),
            per_session: self.per_session.clone(),
        })
    }
}
```

**相比单一 SpawnGuard 的改进：**
- ✅ **公平性保证** — 一个Session不能用尽所有全局槽位
- ✅ **隔离性** — Session级故障不影响其他Session
- ✅ **可观测性** — 可分别查看全局和Session级使用率

---

## 6. P1 — Event-driven 状态传播（替代轮询）

### 6.1 当前问题

BLUE70的`AgentMessenger::wait_for`使用`tokio::time::sleep`轮询`lifecycle`状态，浪费CPU且延迟高（轮询间隔）。

### 6.2 改进方案：watch channel

```rust
pub struct AgentNode {
    pub path: AgentPath,
    pub lifecycle: watch::Sender<AgentLifecycle>,
    // ...
}

impl AgentMessenger {
    /// 订阅Agent状态变更 — 事件驱动，零轮询
    pub async fn subscribe(&self, path: &AgentPath) -> Option<watch::Receiver<AgentLifecycle>> {
        self.tree.read().await.get(path).map(|n| n.lifecycle.subscribe())
    }

    /// 等待Agent进入终止状态
    pub async fn wait_for_completion(
        &self,
        path: &AgentPath,
        timeout: Duration,
    ) -> Result<AgentLifecycle, WaitError> {
        let mut rx = self.subscribe(path).await
            .ok_or(WaitError::AgentNotFound)?;
        
        tokio::time::timeout(timeout, async {
            loop {
                let state = rx.borrow_and_update().clone();
                if state.is_terminal() {
                    return state;
                }
                rx.changed().await?;
            }
        }).await?
    }
}
```

---

## 7. P1 — Agent 生命周期状态机

### 7.1 改进方案

```rust
pub enum AgentLifecycle {
    Registered {
        at_ms: u64,
    },
    Idle {
        since_ms: u64,
    },
    Active {
        phase: AgentPhase,
        started_at_ms: u64,
        tokens_used: u64,
    },
    Completed {
        result: AgentResult,
        tokens_used: u64,
        wall_time_ms: u64,
        completed_at_ms: u64,
    },
    Errored {
        error: AgentError,
        tokens_used: u64,
        wall_time_ms: u64,
    },
    Cancelled {
        reason: String,
        tokens_used: u64,
    },
}

impl AgentLifecycle {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed { .. } | Self::Errored { .. } | Self::Cancelled { .. })
    }
}
```

---

## 8. P1 — Thread 持久化与恢复

### 8.1 改进方案

```rust
/// Agent关系图持久化存储
pub trait AgentGraphStore: Send + Sync {
    async fn upsert_edge(&self, parent: &AgentPath, child: &AgentPath, metadata: &AgentMetadata) -> Result<()>;
    async fn set_edge_status(&self, path: &AgentPath, status: &AgentLifecycle) -> Result<()>;
    async fn list_descendants(&self, path: &AgentPath) -> Result<Vec<(AgentPath, AgentLifecycle)>>;
    async fn remove_subtree(&self, root: &AgentPath) -> Result<u64>;
}

/// SQLite实现
pub struct SqliteAgentGraphStore {
    db: SqlitePool,
}

// Schema:
// CREATE TABLE agent_edges (
//     parent_path TEXT NOT NULL,
//     child_path TEXT NOT NULL UNIQUE,
//     agent_name TEXT NOT NULL,
//     role TEXT,
//     status TEXT NOT NULL,
//     tokens_used INTEGER DEFAULT 0,
//     wall_time_ms INTEGER DEFAULT 0,
//     created_at_ms INTEGER NOT NULL,
//     completed_at_ms INTEGER,
//     PRIMARY KEY (parent_path, child_path)
// );
// CREATE INDEX idx_edges_parent ON agent_edges(parent_path);
// CREATE INDEX idx_edges_status ON agent_edges(status);
```

### 8.2 恢复流程

```
进程重启 → load roots (parent_path IS NULL) → 恢复AgentTree
         → 检查未完成Agent → 标记为Interrupted
         → 通知用户/自动恢复
```

---

## 9. P2 — ContextFragment 注入体系

### 9.1 当前问题

go-on的上下文注入散落在`process_chat_request`函数中，每次新增上下文类型都需要修改核心函数。Codex有30+个独立的`ContextFragment`模块，通过`FragmentRegistration` trait注入。

### 9.2 改进方案

```rust
/// 上下文片段 — 按优先级和角色注入
pub trait ContextFragment: Send + Sync {
    fn role(&self) -> FragmentRole;
    fn priority(&self) -> FragmentPriority;
    fn body(&self, ctx: &TurnContext) -> String;
    fn weight(&self) -> u32 { 1 }
}

pub enum FragmentRole {
    System,
    Developer,
    User,
}

pub enum FragmentPriority {
    Low,       // 可被Token预算裁剪
    Normal,    // 正常注入
    High,      // 倾向于保留
    Critical,  // 必须注入（安全/治理相关）
}

/// 片段注册表
pub struct FragmentRegistry {
    fragments: Vec<Box<dyn ContextFragment>>,
}

impl FragmentRegistry {
    /// 构建上下文 — 按优先级排序，尊重Token预算
    pub fn build_context(
        &self,
        ctx: &TurnContext,
        budget: usize,
    ) -> Vec<(FragmentRole, String)> {
        let mut sorted: Vec<_> = self.fragments.iter().collect();
        sorted.sort_by_key(|f| f.priority() as u8);
        
        let mut result = Vec::new();
        let mut used = 0;
        for fragment in sorted {
            let body = fragment.body(ctx);
            let cost = body.len();
            if fragment.priority() >= FragmentPriority::High || used + cost <= budget {
                result.push((fragment.role(), body));
                used += cost;
            }
        }
        result
    }
}
```

### 9.3 迁移映射

| go-on当前注入位置 | Codex对应Fragment | 新Fragment名称 |
|-------------------|-------------------|----------------|
| 模型instructions (process_chat_request) | `PersonalitySpecInstructions` | `PersonalityFragment` |
| 可用工具列表 | `AvailableSkillsInstructions` | `AvailableToolsFragment` |
| Token预算 | `TokenBudgetContext` | `TokenBudgetFragment` |
| 用户指令 | `UserInstructions` | `UserInstructionsFragment` |
| 安全策略 | `PermissionsInstructions` | `GovernanceFragment` |
| World state | `world_state/` | `WorldStateFragment` |
| 多Agent模式 | `MultiAgentModeInstructions` | `MultiAgentFragment` |
| 环境上下文 | `EnvironmentContext` | `EnvironmentFragment` |

---

## 10. P2 — 内容Compaction（借鉴Codex + 超越）

### 10.1 需求场景

长对话导致上下文窗口溢出，需要自动/手动压缩历史。Codex提供了完整的Compaction方案：

- **Pre-turn Compaction**: 在new turn前压缩历史
- **Mid-turn Compaction**: 在turn中压缩（需要模型支持）
- **Remote Compaction V2**: 使用云端高能力模型压缩
- **Token Budget驱动**: `COMPACT_USER_MESSAGE_MAX_TOKENS = 20_000`
- **Hooks集成**: `PreCompact`/`PostCompact`事件

### 10.2 go-on改进方案（基础版）

```rust
pub struct CompactionManager {
    summarizer: Arc<dyn Agent>,  // 使用廉价模型做摘要
    max_tokens_before_compact: usize,
    keep_last_n_turns: usize,
}

pub enum CompactionStrategy {
    /// 移除最旧轮次，保留最近N轮
    SlidingWindow { keep_turns: usize },
    /// LLM摘要旧轮次，注入为系统消息
    Summarize { max_summary_tokens: usize },
    /// 混合：摘要+滑动窗口
    Hybrid { summary_turns: usize, keep_turns: usize },
}

impl CompactionManager {
    pub async fn compact(
        &self,
        history: &mut ConversationHistory,
        strategy: CompactionStrategy,
    ) -> Result<CompactionResult> {
        match strategy {
            CompactionStrategy::Summarize { max_summary_tokens } => {
                let old_turns = history.drain_oldest(history.len() - self.keep_last_n_turns);
                let summary = self.summarizer.chat(&format!(
                    "Summarize the following conversation:\n{}", 
                    old_turns.to_text()
                )).await?;
                history.prepend_system_summary(summary);
                Ok(CompactionResult::Summarized { 
                    turns_compacted: old_turns.len(),
                    summary_tokens: summary.len(), 
                })
            }
            // ...
        }
    }
}
```

### 10.3 更完美方案：AdaptiveCompactor — 自适应学习型Compaction

超越 Codex 的固定策略，go-on 可以构建一个**自适应学习型 Compactor**，根据对话历史的效果自动调整压缩策略：

```rust
/// 自适应Compactor
pub struct AdaptiveCompactor {
    /// 基础 CompactionManager
    base: CompactionManager,
    /// 历史效果记录
    effectiveness_history: VecDeque<CompactionRecord>,
    /// 当前最佳策略
    best_strategy: CompactionStrategy,
    /// 自适应阈值（初始值，随时间调整）
    adaptive_threshold: AdaptiveThreshold,
}

/// 压缩效果记录
#[derive(Debug)]
pub struct CompactionRecord {
    pub strategy: CompactionStrategy,
    pub tokens_saved: usize,
    pub quality_score: f64,        // 压缩后模型回答质量的评估分数
    pub user_feedback: Option<f64>,// 用户反馈（如果有）
    pub timestamp: u64,
}

impl AdaptiveCompactor {
    /// 执行自适应Compaction
    pub async fn compact(
        &self,
        session: &mut SessionActor,
    ) -> Result<CompactionResult> {
        // 1. 检查是否需要进行Compaction
        if !self.should_compact(session) {
            return Ok(CompactionResult::Skipped);
        }

        // 2. 选择策略：基于历史效果
        let strategy = self.select_strategy();
        
        // 3. 执行前的钩子
        session.hooks.run(HookEvent::PreCompact).await;
        
        // 4. 执行Compaction
        let result = self.base.compact(&mut session.history, strategy).await?;
        
        // 5. 执行后的钩子
        session.hooks.run(HookEvent::PostCompact).await;
        
        // 6. 记录效果（用于未来策略选择）
        self.record_effectiveness(CompactionRecord {
            strategy,
            tokens_saved: result.tokens_saved,
            quality_score: result.quality_score,
            user_feedback: None,
            timestamp: now_ms(),
        });
        
        // 7. 更新自适应阈值
        self.adaptive_threshold.adjust(&self.effectiveness_history);
        
        Ok(result)
    }

    /// 选择最优策略（基于历史效果 + 当前对话特征）
    fn select_strategy(&self) -> CompactionStrategy {
        let conversation_length = self.base.history.len();
        let avg_quality = self.effectiveness_history.iter()
            .map(|r| r.quality_score)
            .sum::<f64>() / self.effectiveness_history.len().max(1) as f64;
        
        match (conversation_length, avg_quality) {
            // 短对话 + 高质量 → 滑动窗口（快速、低成本）
            (l, _) if l < 20 => CompactionStrategy::SlidingWindow { keep_turns: 10 },
            // 长对话 + 低质量 → 摘要（提升质量）
            (_, q) if q < 0.6 => CompactionStrategy::Summarize { max_summary_tokens: 2000 },
            // 默认 → 混合
            _ => CompactionStrategy::Hybrid { summary_turns: 8, keep_turns: 5 },
        }
    }
}
```

**超越 Codex 的关键改进：**
| 能力 | Codex | go-on AdaptiveCompactor |
|------|-------|------------------------|
| 策略选择 | 固定 | **自适应**（基于历史效果） |
| 质量评估 | 无 | ✅ 自动评分 + 用户反馈 |
| 阈值调整 | 静态 | ✅ 动态调整 |
| 效果追踪 | 无 | ✅ CompactionRecord 持久化 |
| 钩子集成 | Pre/Post | ✅ Pre/Post + 效果回调 |

---

## 11. P2 — Guardian-style 独立模型审查

### 11.1 借鉴Codex Guardian

Codex的Guardian Review Session最独特的设计是**使用独立的模型实例**来审查操作安全性：

1. 构建精简transcript（保留用户意图+相关上下文）
2. 提交给专用Guardian Review Session（独立配置/模型）
3. Guardian模型返回Allow/Deny + 风险等级 + 理由
4. Fail-closed: 超时/解析失败/执行错误 → 拒绝
5. 熔断器: 连续3次拒绝 → 停止自动审查

### 11.2 go-on集成方案

```rust
pub struct GuardianReviewer {
    review_agent: Arc<dyn Agent>,  // 独立模型（如廉价模型做审查）
    circuit_breaker: GuardianCircuitBreaker,
    timeout: Duration,  // 90s — Codex标准
}

pub struct GuardianCircuitBreaker {
    max_consecutive_denials: u32,  // 3
    max_recent_denials: u32,       // 10 out of 50
    denials: VecDeque<bool>,
}

impl GuardianReviewer {
    pub async fn review_action(
        &self,
        action: &ToolInput,
        transcript: &ConversationSummary,
    ) -> GuardianDecision {
        if self.circuit_breaker.should_skip_review() {
            return GuardianDecision::EscalateToUser;
        }
        
        let prompt = self.build_review_prompt(action, transcript);
        match tokio::time::timeout(self.timeout, self.review_agent.chat(&prompt)).await {
            Ok(Ok(response)) => self.parse_decision(&response),
            _ => GuardianDecision::Deny { 
                reason: "Guardian review failed — fail closed".into() 
            },
        }
    }
}
```

---

## 12. P3 — 技能选择多算法集成（借鉴Codex skills extension）

Codex的`ext/skills/`提供了6种技能选择算法：

| 算法 | 特点 | 适用场景 |
|------|------|---------|
| `CharacterNgram` | 字符n-gram相似度 | 模糊匹配 |
| `FieldedBm25` | BM25信息检索 | 关键词搜索 |
| `MultiQueryLexical` | 多查询词法匹配 | 多关键词场景 |
| `RoutingCardLexical` | 路由卡片 | 预定义路由 |
| `RrfLexicalChar` | 混合RRF排序 | 综合场景 |
| `WeightedLexical` | 加权词法匹配 | 自定义权重 |

go-on当前只有`ToolRecommender`的关键词匹配+成功率模型。建议增加BM25和向量相似度算法。

---

## 13. 无需引入的改进（低收益/无意义）

### 13.1 Bazel构建系统
Codex使用Bazel, go-on使用Cargo。**不引入**：Cargo是Rust标准工具，Bazel引入的学习成本>收益。

### 13.2 完整的Go Wire DI
Harness使用Google Wire编译时DI。go-on的Rust无需等效方案：trait对象+Arc已足够灵活。

### 13.3 容器化沙箱（Docker Sandbox）
Harness/Codex使用Docker/bubblewrap沙箱。**短期不引入**：go-on的SandboxExecutor已能满足代码修改测试需求。长期可考虑bubblewrap集成。

### 13.4 Redis Streams事件系统
Harness的生产级Redis Streams事件系统。**短期不引入**：go-on的单进程场景用channel即可。多进程时再考虑（Hub模块已预留）。

### 13.5 OCI Registry
Harness的完整Container/Artifact Registry。**不引入**：与go-on核心定位无关。

### 13.6 CI/CD Pipeline引擎
Harness的Pipeline Scheduler/Runner。**不引入**：go-on的Planner-Executor已满足Agent场景需求。Harness的DAG+调度器模式可在Planner中借鉴。

### 13.7 本章补充说明：BLUE71.1 新增方案的设计原则

上述 §2.1.1~§10.3 中新增的7个改进方案（SessionActor、TypedEventBus、DagToolPipeline、LayeredConfig、HookRegistry、TieredBudget、AdaptiveCompactor）均遵循以下原则：

1. **复用现有基础设施** — 所有新方案都建立在已有代码之上（如DagToolPipeline复用ExecutionGraph）
2. **不引入新外部依赖** — 全部使用Rust标准库+现有依赖（tokio、dashmap）实现
3. **可逐步落地** — 每个方案都有清晰的P0→P1→P2→P3分阶段路径
4. **不牺牲现有优势** — 治理、自进化、记忆、韧性等核心竞争力保持不变
5. **编译时安全优先** — TypedEventBus和LayeredConfig在编译期捕获类型错误

## 14. 改进路线图

### 第一阶段：P0 核心修复（5-7天）

| 任务 | 预估 | 依赖 | 收益 |
|------|------|------|------|
| AgentThread非阻塞生成 | 2天 | — | 10x吞吐提升 |
| SpawnGuard RAII防护 | 1天 | AgentThread | 消除泄漏 |
| InputQueue + Actor Loop | 1.5天 | AgentThread | 架构根基 |
| AgentStatus watch channel | 0.5天 | AgentThread | 事件驱动 |
| completion_watcher | 1天 | AgentThread + InputQueue | 自动协作 |
| **集成测试 + 性能验证** | 1天 | 以上全部 | 质量保证 |

### 第二阶段：P1 能力增强（7-10天）

| 任务 | 预估 | 依赖 | 收益 |
|------|------|------|------|
| AgentLifecycle FSM | 1天 | P0完成 | 状态可观测 |
| SessionActor 包装层 | 2天 | AgentLifecycle + AgentThread | 会话级资源管理 |
| AgentGraphStore + SQLite迁移 | 2天 | AgentLifecycle | 持久化恢复 |
| ContextFragment注册体系 | 2天 | — | 可扩展上下文 |
| Fragment迁移（8个片段） | 2天 | 注册体系 | 解耦核心函数 |
| CompactionManager（基础版） | 2天 | ContextFragment | 长对话支持 |
| **E2E测试** | 1天 | 以上全部 | 质量保证 |

### 第三阶段：P2 高级特性（7-10天）

| 任务 | 预估 | 依赖 | 收益 |
|------|------|------|------|
| Guardian-style 独立审查 | 2天 | — | 安全性提升 |
| BM25 + 向量技能选择 | 1.5天 | — | 技能匹配精度 |
| SandboxExecutor增强（bubblewrap） | 2天 | — | 真正沙箱隔离 |
| 分布式ToolLockManager | 1.5天 | — | 多进程支持 |
| **SessionActor 持久化+恢复** | 2天 | SessionActor | 进程重启不丢失 |
| **DagToolPipeline** | 2天 | ExecutionGraph | 工具级DAG执行 |
| **TypedEventBus 基础版** | 1天 | — | 类型安全事件总线 |

### 第四阶段：P3 长期演进（持续）

| 任务 | 方向 |
|------|------|
| Feedback系统（RingBuffer+Sentry） | 用户反馈闭环 |
| Hub多进程架构激活 | 分布式部署 |
| EvolutionLoop RequireHuman审批 | 人工审核自进化 |
| Redis事件系统 + TypedEventBus集成 | 分布式事件 |
| AdaptiveCompactor 学习型 | 自适应压缩策略 |
| HookRegistry 24事件钩子 | 全生命周期扩展 |
| TieredBudget 双层预算 | 公平性保证 |
| LayeredConfig 8层+热加载 | 运行时灵活性 |
| Session-level Mid-turn Steering | 中间注入能力 |

---

## 15. 总结

### 15.1 三系统最强模块（可互相借鉴）

| 模块 | 最强系统 | go-on可借鉴 |
|------|---------|------------|
| Agent会话模型 | **Codex** (Actor + InputQueue) | ✅ P0核心改进 |
| 会话级管理（超越） | **go-on SessionActor** (树状架构) | ✅ P1全新设计 |
| 多Agent协调质量 | **go-on** (Council投票) | 保持 |
| 多Agent工程鲁棒性 | **Codex** (RAII + 事件 + 异步) | ✅ P0/P1改进 |
| 工具系统模块化 | **Codex** (Tools独立crate) | ✅ 长期考虑 |
| 工具执行DAG | **go-on DagToolPipeline** (ExecutionGraph+) | ✅ P2全新设计 |
| 安全治理 | **go-on** (HarnessBus) | ✅ 保持+Guardian |
| 自进化 | **go-on** (EvolutionLoop) | 保持 |
| 事件系统（超越） | **go-on TypedEventBus** (编译时类型安全) | ✅ P2/P3全新设计 |
| 配置管理（超越） | **go-on LayeredConfig** (8层+热加载) | ✅ P2/P3全新设计 |
| 技能选择 | **Codex** (6算法) | ✅ P3考虑 |
| 内存系统 | **go-on** (5级分类) | 保持 |
| Agent持久化 | **Harness** (50+ Store) | ✅ P1改进 |
| 可观测性 | **go-on** (OTel + Alert) | 保持 |
| 插件架构（超越） | **go-on HookRegistry** (24事件钩子) | ✅ P3全新设计 |
| Compaction（超越） | **go-on AdaptiveCompactor** (自适应学习) | ✅ P2全新设计 |

### 15.2 go-on的核心竞争力（改进后）

1. **唯一的自进化Agent系统** — EvolutionLoop + SandboxExecutor
2. **最强的治理体系** — HarnessBus 12策略超越所有竞品
3. **最丰富的AI模型集成** — 25+提供商
4. **最完整的记忆系统** — 5级分类+语义+向量+3级令牌缓存
5. **最强韧性/容错** — HyperResilience + Chaos + FaultTolerance
6. **最优多Agent决策质量** — Council投票制+多轮审议
7. **✅ SessionActor 树状架构** — 超越Codex的会话管理（改进后）
8. **✅ TypedEventBus** — 编译时类型安全的事件总线（改进后）
9. **✅ AdaptiveCompactor** — 自适应学习型内容压缩（改进后）

### 15.3 最需要补齐的差距

1. **异步非阻塞Agent生成** — 当前阻塞式10x性能损失（P0）
2. **RAII资源管理** — 消除并发计数泄漏（P0）
3. **事件驱动状态传播** — 替代轮询（P0）
4. **Agent生命周期FSM** — 可观测的状态管理（P1）
5. **SessionActor 树状架构** — 会话级资源+生命周期管理（P1）
6. **Agent持久化恢复** — 进程重启不丢失状态（P1）
7. **ContextFragment注入体系** — 解耦核心函数（P1）
8. **Compaction（基础+自适应）** — 支持长对话（P2）
9. **DagToolPipeline** — 工具级DAG执行（P2）
10. **TypedEventBus** — 类型安全事件总线（P2）
11. **TieredBudget** — 双层公平预算（P2）
12. **LayeredConfig + 热加载** — 运行时灵活性（P3）
13. **HookRegistry 24钩子** — 全生命周期扩展（P3）

> **预计总工期**: P0(5-7天) + P1(7-10天) + P2(7-10天) + P3(持续) = **19-27天**  
> **预期收益**: 吞吐量10x+, 架构现代化（从单体→SessionActor树状架构）, 生产级鲁棒性  
> **保持优势**: 治理、自进化、记忆、韧性等核心竞争力不变  
> **新增优势**: SessionActor + TypedEventBus + AdaptiveCompactor + DagToolPipeline
