# BLUE50 — go-on 超级智能全能打工王者：多Agent编排终极进化

> 更新时间：2026-05-31
>
> 目标：BLUE49 已将14个维度全部拉到 10/10，BLUE50 在此基础上进行三轮深度扫描（SRC 302个.rs文件 94,543行、GUI 40个.rs文件、VSCode-Addon 19个.ts文件），
> 从**速度/流畅度、智能深度、三端通信、多Agent协作效率、架构完整性、测试覆盖**六个核心方向，识别真实运行时的深层瓶颈，
> 并提出可逐步实施的改进计划，使系统真正达到"超级智能全能打工王者"的境界。

## 0. 核心规则

1. **排除 i18n 字段硬编码** — 不涉及 locale 文本本身的结构调整。
2. **排除分拆文件** — 不将现有文件拆分为更小文件。
3. **三端一统（backend / GUI / vscode-addon）** — 考虑三端配合、通讯流畅稳定性。
4. **注释英文** — 所有新增模块的代码注释必须使用英文。
5. **3 种服务器 Profile 全链路闭合** — profile-local、profile-simple-server、profile-multi-users-server 必须正确编译和行为一致。
6. **5 种协议全链路闭合** — auto、acp stdio、acp http、mcp stdio、mcp http。
7. **零警告、零冲突、零遗漏** — 最终验证 `cargo clippy --all-features -- -D warnings` 零警告。
8. **完整闭合** — 每个模块最终必须达到：编译通过、零警告、接入 governance.status、可通过 health 端点观测、有集成测试覆盖。
9. **不允许占位、空函数、逻辑错误** — 所有功能必须完整实现。
10. **回写完成率** — 每轮完成后，回写完成率（简述）。
11. **不得随意变更计划** — 严格按计划完整实施改进。
12. **最完美、最优化修改** — 不需要简化修改或最小修改。

---

## 1. BLUE49 基线回顾（10/10 全层）

| 维度 | BLUE49 终分 | 核心资产 |
|:----:|:----------:|:---------|
| 架构层 | 10/10 | ProtocolNegotiator + BrainLoop + Orchestrator 8步清晰 |
| 运行层 | 10/10 | 并行Agent执行 + SemanticResponseCache + 零 UI thread::sleep |
| 智能层 | 10/10 | Council声誉+自动淘汰 + MultiModelVoter + Metacognitive |
| 治理层 | 10/10 | SecurityGovernor + PUA + Audit + HarnessBus 策略引擎 |
| 协议层 | 10/10 | 5协议+Negotiator+RateLimit+MultiChannelTransport |
| 韧性层 | 10/10 | ChaosEngine + HyperResilience + CircuitBreaker全链路 |
| 可观测层 | 10/10 | AlertManager + Telemetry + LivePerformanceFeed |
| 内存层 | 10/10 | 17+子系统 LRU/FIFO + SemanticCache + 有界化 |
| GUI层 | 10/10 | egui ChatView + SSE接口预留 + keyring安全 |
| SDK层 | 10/10 | Rust SDK + Python SDK + 指数退避 |
| VSCode层 | 10/10 | ESLint零错误 + TSC零错误 + JSON-RPC stdio |
| 测试层 | 10/10 | 26+测试 + 集成测试 + Benchmark |
| 部署层 | 10/10 | 2套方案 + 25脚本 + Docker HEALTHCHECK |
| 安全层 | 10/10 | RateLimitMiddleware + keyring:// + MCP常数时间比较 |

---

## 2. 三轮深度扫描发现的深层瓶颈（62个）

虽然14个维度已在代码层面达到 10/10（功能齐全、编译零警告、测试全通过），但经过三轮深度扫描发现：**~15,000行核心代码处于孤岛状态**（CapabilityBus 7.2k + Scheduler 800 + Schema 500 + HarnessBus + Omnipotent + PluginSystem 等从未被调用），ACP helpers/ 36文件 + ACP impl/ 27文件零单元测试。

以下按类别列出全部62个瓶颈：

### 2.1 速度与流畅度瓶颈（S1-S10）

| # | 瓶颈 | 位置 | 严重度 | 现象 |
|---|------|------|:------:|------|
| S1 | BrainLoop 完全同步 | `src/orchestration/brain_loop.rs` | 🔴 | Plan→Execute→Reflect→Replan 全同步 Mutex，execute_step 阻塞持锁 |
| S2 | DAG 层级屏障 | `src/orchestration/dag_executor.rs` | 🟠 | 层级N+1等N全完成，无投机执行 |
| S3 | GUI 无真流式 SSE | `gui/src/backend.rs:734` | 🟠 | chat_stream() 已实现但标记 dead_code |
| S4 | VSCode 无流式 | `vscode-addon/src/chatView.ts:204` | 🟠 | 明确注释 non-streaming |
| S5 | SemanticCache 假 LRU | `src/memory/semantic_cache.rs` | 🟡 | 按created_at驱逐(FIFO)，access_count未用 |
| S6 | SemanticCache 哈希慢 | `src/memory/semantic_cache.rs` | 🟡 | DefaultHasher(SipHash)，O(n)大gram扫描 |
| S7 | 多子系统单 Mutex | RateLimit/HyperResilience/LivePerformance等 | 🟡 | 各子系统独立粗粒度锁，高并发争用 |
| S8 | Council 三锁顺序 | `council.rs:tally_votes()` | 🟡 | proposals→votes→members 三锁顺序持锁 |
| S9 | Metacognitive 线性扫描 | `metacognitive.rs` | 🟢 | .iter().find() O(n)，无HashMap索引 |
| S10 | Telemetry SHA-256 | `observability/telemetry.rs` | 🟢 | 每span计算完整SHA-256 |

### 2.2 智能深度瓶颈（I1-I8）

| # | 瓶颈 | 严重度 | 现象 |
|---|------|:------:|------|
| I1 | 无CoT上下文传播 | 🔴 | DAG节点仅传JSON Value，无推理上下文 |
| I2 | BrainLoop无深度推理 | 🟠 | plan/reflect仅数值判断，不调用LLM |
| I3 | SemanticCache非真语义 | 🟠 | bigram Jaccard，非embedding相似度 |
| I4 | Council无辩论 | 🟡 | 投票箱模式，无多轮审议 |
| I5 | MultiModelVoter无融合 | 🟡 | 只选单一获胜响应，不做推理融合 |
| I6 | Metacognitive反馈不闭合 | 🟡 | 纠正行动效果未反馈回Planner |
| I7 | SelfModel能力静态 | 🟢 | effectiveness/confidence静态配置 |
| I8 | 无任务优先级 | 🟢 | DAG所有节点优先级相同 |

### 2.3 三端通信瓶颈（C1-C7）

| # | 瓶颈 | 严重度 | 现象 |
|---|------|:------:|------|
| C1 | 无WebSocket/持久连接 | 🔴 | 全部HTTP请求/响应，无双向实时 |
| C2 | GUI↔VSCode无同步 | 🟠 | 两端完全独立，无共享会话状态 |
| C3 | stdio JSON-RPC脆弱 | 🟡 | 仅按行解析JSON，无消息分帧 |
| C4 | GUI Backend重启简单 | 🟡 | 无健康预热等待、无优雅降级 |
| C5 | VSCode TOML regex脆弱 | 🟡 | upsertFlowPhases用regex，自认fragile |
| C6 | VSCode健康检查5min | 🟢 | 默认300s轮询 |
| C7 | 两HTTP客户端 | 🟢 | quick+long独立连接池 |

### 2.4 内存与资源瓶颈（M1-M5）

| # | 瓶颈 | 严重度 | 现象 |
|---|------|:------:|------|
| M1 | RateLimit无租户淘汰 | 🟡 | HashMap永不驱逐闲置租户 |
| M2 | LivePerformance无淘汰 | 🟡 | 模型EMA永不驱逐 |
| M3 | MemoryStore O(n)插入 | 🟡 | 每次store()三次线性扫描 |
| M4 | AlertManager无历史 | 🟢 | 仅有计数器，无环形缓冲区 |
| M5 | Cache TTL残留 | 🟢 | 过期条目仅get()时清理 |

### 2.5 孤岛模块瓶颈（O1-O15）— 第二轮+第三轮核心发现

| # | 瓶颈 | 位置 | 严重度 | 现象 |
|---|------|------|:------:|------|
| O1 | CapabilityBus 7,200行死代码 | `intelligence/capability_bus/` 8文件 | 🔴 | new()零调用点，Sense→Decide→Act→Feedback→Evolve管线从未运行 |
| O2 | Scheduler 800行孤岛+竞态 | `orchestration/scheduler.rs` | 🔴 | 双层调度器从未调用，fail()存在TOCTOU竞态 |
| O3 | HarnessBus策略引擎未集成 | `governance/harness_bus.rs` | 🔴 | 聚合全部governance组件但Phase 0实现，未接入请求生命周期 |
| O4 | Schema类型500+行死代码 | `schema/` 6文件 | 🟠 | ACP v0.13.2规范类型全部dead_code，Handler用ad-hoc类型 |
| O5 | ThreadSafeAuditLog未接通 | `governance/audit.rs` | 🟠 | NDJSON持久化已实现但标记F-GAP-49 |
| O6 | FullAutoFlow空注册表 | `orchestration/full_auto.rs` | 🟠 | 创建空SkillRegistry/ToolRegistry，零技能零工具运行 |
| O7 | TokenCache未包装 | `intelligence/token_cache/` | 🟠 | CachedAgentWrapper就绪但从未包装任何Agent |
| O8 | Omnipotent Mode无入口 | `orchestration/omnipotent.rs` | 🟡 | 全部实现但select_mode无"omnipotent"分支 |
| O9 | PromptLayers空洞 | `orchestration/prompt_layers.rs` | 🟡 | 8层架构定义但每层内容生成器未实现 |
| O10 | ToolPipeline死代码 | `orchestration/tool_pipeline.rs` | 🟡 | Parallel/Sequence/Conditional全部dead_code |
| O11 | PluginSystem空壳 | `orchestration/plugin_system.rs` | 🟡 | trait+manifest+registry全定义，零实现 |
| O12 | SystemIntegration不存在feature | `orchestration/integration.rs` | 🟡 | gate在不存在feature sub-bus-tool-future后 |
| O13 | SubAgentFactory数据记录 | `agents/factory/` | 🟡 | 创建数据记录不启动Agent，status硬编码"running" |
| O14 | memory_health近孤岛 | `observability/memory_health/` | 🟡 | check_startup_memory等全部零调用点 |
| O15 | Rationalization guard零调用 | `governance/rationalization.rs` | 🟢 | 自述zero-call，无重试预算 |

### 2.6 测试覆盖瓶颈（T1-T7）

| # | 瓶颈 | 位置 | 严重度 | 现象 |
|---|------|------|:------:|------|
| T1 | ACP helpers/ 36文件零测试 | `acp/helpers/` | 🔴 | 核心agent选择、autonomy循环、governance策略全无测试 |
| T2 | ACP impl/ 27文件零测试 | `acp/impl/` | 🔴 | 27个handler pack仅chat_tests有测试 |
| T3 | transport_factory零测试 | `acp/transport_factory.rs` | 🟠 | 多后端cache/vector初始化、5协议dispatch无测试 |
| T4 | OrchestrationContext仅构造测试 | `orchestration/context.rs` | 🟠 | record_model_execution行为、failover、并发无测试 |
| T5 | E2E benchmark <57%覆盖 | `tests/comprehensive_feature_benchmark.rs` | 🟡 | 21维度中9个Qualitative（score 0.0） |
| T6 | 无流式性能回归CI | 全局 | 🟡 | TTFT/TPS/TTC无CI防护 |
| T7 | Schema无序列化往返测试 | `mcp/schema.rs` | 🟢 | — |

### 2.7 架构集成瓶颈（A1-A19）

| # | 瓶颈 | 严重度 | 现象 |
|---|------|:------:|------|
| A1 | CapabilityGraph未从AgentRegistry喂入 | 🔴 | 能力路由图无法反映40+实际模型 |
| A2 | WorldModel从未被Planner查询 | 🟠 | 环境数据收集→零消费 |
| A3 | ContinuousLearning/Discovery/Evolution不驱动 | 🟠 | evolve数据收集但从不驱动路由器/Planner |
| A4 | Reputation未流入AgentRegistry | 🟠 | Agent评分永不衰减 |
| A5 | Verification hooks未在review gate调用 | 🟡 | DeterministicVerifier可调用但无人调用 |
| A6 | FederatedRL两套独立实现 | 🟡 | federated_rl.rs + reinforcement/federated.rs 均未接通 |
| A7 | Drift auto_remediate空操作 | 🟡 | 标志存在但无remediation逻辑 |
| A8 | RBAC escalation不完整 | 🟡 | AccessDecision::Escalate无resolver |
| A9 | Config热加载轮询非inotify | 🟡 | WatchDog用tokio::time::sleep |
| A10 | 无结构化Tracing Span | 🟡 | AcpServer→FlowManager→Agent无Span传播 |
| A11 | 无优雅关机编排 | 🟡 | 无DrainGuard、无/health/ready |
| A12 | 无Prometheus /metrics | 🟡 | HistogramBuckets存在但无导出端点 |
| A13 | 无反压/Load Shedding | 🟡 | 超载时请求堆积非早期拒绝 |
| A14 | 无服务端Auth中间件 | 🟡 | rbac存在但请求路径无auth |
| A15 | FaultTolerance无持久化 | 🟡 | 重启全部丢失 |
| A16 | FailurePrevention纯规则 | 🟡 | 硬编码字符串匹配，无统计分析 |
| A17 | Review controls单reviewer | 🟢 | 无quorum逻辑 |
| A18 | MCP logging/setLevel不传播 | 🟢 | 级别存储但不传播到子系统 |
| A19 | Hardening租户预算竞态 | 🟢 | Cell<i64> + Mutex无同步 |

### 2.8 VSCode深度瓶颈（V1-V6）+ 代码质量（Q1-Q6）

| # | 瓶颈 | 严重度 | 现象 |
|---|------|:------:|------|
| V1 | VSCode无AbortController | 🟠 | sendRequest无abort路径 |
| V2 | VSCode sessions不同步GUI | 🟡 | globalState vs chat_sessions.json |
| V3 | commandRegistry无重试 | 🟢 | try/catch + 显示错误 |
| V4 | GUI SSE流不可取消 | 🟡 | stop_requested仅在发送时检查 |
| V5 | GUI CachedView hash=0 | 🟢 | 大部分视图缓存被禁用 |
| V6 | GUI Channel溢出丢消息 | 🟢 | try_send满时仅eprintln |
| Q1 | AcpServer God Object | 🟠 | 40+字段单一struct |
| Q2 | Wildcard re-exports | 🟡 | pub use autotune::* 污染命名空间 |
| Q3 | anyhow+thiserror混用 | 🟡 | AppError::External包装anyhow::Error |
| Q4 | 锁类型混用 | 🟡 | tokio/std Mutex/RwLock混用 |
| Q5 | ErrorContext样板代码 | 🟢 | 80行双重match |
| Q6 | SystemContext同步阻塞 | 🟢 | load_repo_context同步读取git/文件 |

---

## 3. BLUE50 改进计划（41 GAP，10 Step）

### 3.1 Step 1（P0 — 速度革命）：流式化 + 异步化核心链路

#### GAP-B50-01（CRITICAL）：GUI 真流式 SSE 接入

**文件**：`gui/src/backend.rs`、`gui/src/views/chat/`

**问题**：`chat_stream()` 端点已实现但被 `#[allow(dead_code)]` 禁用。GUI 使用非流式 HTTP POST，收到完整响应后再通过 mpsc channel 模拟流式输出——用户等待全文返回后才看到第一个 token。流进行中无法取消（`stop_requested` 仅在发送时检查）。

**修复**：
1. 启用 `chat_stream()` → 使用 `reqwest::Response::bytes_stream()` 消费 SSE 流
2. 在 `ChatView` 中实现 `StreamProcessor` trait：逐 token 到达 → 直接推送到 UI
3. 保留 mpsc channel fallback（网络不支持流式时）
4. 添加 `UiStabilityConfig::stream_token_flush_ms` 控制逐 token 渲染帧率
5. 添加 token-level 进度条（已接收 / 预估总量）
6. 添加 AbortController 机制实现流中取消

**验收**：GUI 发送消息后，首个 token 在 <200ms 内出现在界面上（实测 p50）。流中可取消。

#### GAP-B50-02（CRITICAL）：VSCode 流式 Chat 接入

**文件**：`vscode-addon/src/chatView.ts`、`vscode-addon/src/runtimeManager.ts`

**问题**：VSCode 完全非流式，等待完整响应后才渲染。`sendRequest` 无 abort 路径。注释明确标注 "Current mode is request/response (non-streaming)"。

**修复**：
1. 在 `GoOnManager` 中新增 `sendStreamingRequest()` 方法
2. 通过 stdout 逐行解析 JSON-RPC "notification" 消息（无 id 字段的增量 token）
3. 或新增 HTTP SSE fallback：`EventSource` → webview postMessage 逐 token 推送
4. `chatView.ts` 的 webview 内实现逐 token 渲染
5. 添加 "stop generating" 按钮 + AbortController（发送 JSON-RPC cancel 通知）

**验收**：VSCode 内首 token 延迟 <300ms（含进程间通信开销）。可取消进行中的请求。

#### GAP-B50-03（HIGH）：BrainLoop 异步化

**文件**：`src/orchestration/brain_loop.rs`

**问题**：BrainLoop 完全同步（`Arc<Mutex<>>`），execute_step 阻塞持锁。整个 Plan→Execute→Reflect→Replan 使用单一 Mutex 串行化所有操作。Planner 和 Reflect 阶段无法并行。

**修复**：
1. 将 `BrainLoopInner` 的 Mutex 改为 `tokio::sync::RwLock`
2. `execute_step()` 改为 `async fn`，内部工具调用使用 tokio::spawn 并发
3. Plan 和 Reflect 阶段可并行（Plan 生成下一步，Reflect 分析上一步结果）
4. 添加 `async fn run_async(&self, task) -> Result<BrainLoopProfile>` 作为主入口
5. 保留同步 `run()` 作为兼容层（内部 spawn_blocking 调用 async 版本）

**验收**：BrainLoop 不再阻塞 tokio runtime。Plan+Execute+Reflect 总延迟降低 30%+。

#### GAP-B50-04（HIGH）：DAG 投机执行

**文件**：`src/orchestration/dag_executor.rs`

**问题**：DAG 层级 N+1 必须等层级 N 所有节点完成。长尾节点阻塞整个层级。无投机执行（speculative execution）。

**修复**：
1. 层级内节点完成后，立即检查并启动下一层级中**依赖已满足**的节点（而非等待整个层级）
2. 使用 `tokio::sync::Notify` 替代层级 barrier
3. 每个节点完成后 notify 其下游依赖
4. 保留 `max_concurrency` Semaphore 控制总体并发
5. 添加 `speculative_execution: bool` 配置开关

**验收**：DAG 中短路径节点可与前一层级的长尾节点并行执行。Pipeline 总延迟降低 20-40%。

### 3.2 Step 2（P0 — 智能升级）：深度推理 + 真语义 + 审议机制

#### GAP-B50-05（CRITICAL）：Chain-of-Thought 任务上下文传播

**文件**：`src/orchestration/dag_executor.rs` + 新建 `src/orchestration/task_context.rs`

**问题**：DAG 节点间仅传递 JSON `Value` 结果。子任务之间不共享推理上下文、中间思考状态。无法实现"思考链（Chain-of-Thought）"跨子任务传播。

**修复**：
1. 创建 `TaskContext` 结构体含 `id`, `reasoning_trace: Vec<String>`, `intermediate_findings: HashMap<String, Value>`, `confidence: f64`, `open_questions: Vec<String>`, `assumptions: Vec<String>`, `parent_context_id: Option<String>`
2. 每个 `DagNode` 执行后可产出 `TaskContext`
3. 下游节点接收上游 `Vec<TaskContext>`，合并推理链
4. `DagExecutor` 在节点间传递 `Arc<TaskContext>`
5. 集成到 `process_chat_request` 第 5/6 步（Execute/Reflect 之间）

**验收**：多步任务中，后续步骤能引用前面步骤的推理结果。"为什么这样做"可追溯。

#### GAP-B50-06（HIGH）：BrainLoop 深度推理集成

**文件**：`src/orchestration/brain_loop.rs`

**问题**：Plan/Reflect 阶段仅做数值判断（`convergence_threshold: 0.05`），没有真正的"推理→验证→修正"逻辑。不调用 LLM 进行计划分析或反思改进。WorldModel 记录数据但永不查询。

**修复**：
1. `BrainLoopConfig` 新增 `enable_deep_reasoning: bool`
2. `plan()` 阶段：将 `TaskContext` + 当前状态发送给 LLM，获取结构化 `BrainLoopPlan`
3. `reflect()` 阶段：将执行结果 + 历史发送给 LLM，获取 `BrainLoopReflection`（含改进建议）
4. `replan()` 阶段：基于 reflection 内容（而非仅置信度）调整计划
5. 使用 `MultiModelVoter` 进行 plan/reflect 质量验证（多个模型独立评估方案）
6. 集成 WorldModel 查询：plan() 时查询环境实体/关系/事件
7. 添加 `max_deep_reasoning_tokens` 控制 LLM 调用成本

**验收**：复杂多步任务完成率提升。Reflection 包含具体改进建议而非仅数值。WorldModel 数据被 Planner 消费。

#### GAP-B50-07（HIGH）：真语义缓存（Embedding-based）

**文件**：`src/memory/semantic_cache.rs`

**问题**："语义缓存"使用字节级 bigram Jaccard 相似度，不是真正的 embedding 相似度。无法匹配语义相同但措辞不同的请求。

**修复**：
1. 新增 `EmbeddingSemanticCache` 结构体含 `cosine_threshold: 0.92`
2. `get()` 流程：exact hash → cosine similarity（top-3 最近邻）→ 阈值过滤
3. 提供 `SimpleEmbeddingCache`（local TF-IDF）作为零依赖 fallback
4. 提供 `RemoteEmbeddingCache`（通过 MCP 工具调用远程 embedding 服务）
5. 保留 `SemanticResponseCache`（bigram）作为快速路径（<1ms 查找）
6. 添加 `EmbeddingCacheConfig::embedding_dim` 和 `use_embedding: bool`

**验收**：语义相同但措辞不同的请求能被缓存命中（如 "fix the bug in login" ≈ "resolve login defect"）。

#### GAP-B50-08（MEDIUM）：Council 审议/辩论机制

**文件**：`src/orchestration/council/council.rs`

**问题**：Council 是"投票箱"模式——提交→投票→计票，无多轮辩论、反驳、修改提案的审议过程。类似"投票箱"而非"议会讨论"。

**修复**：
1. 新增 `Deliberation` 结构体含 `rounds: Vec<DeliberationRound>`, `max_rounds: 3`, `consensus_reached: bool`
2. `start_deliberation(proposal_id) -> DeliberationId`
3. 每轮：成员提交 `DeliberationStatement`（支持/反对/修改建议）
4. 每轮结束：投票 → 如未达成共识 → 下一轮
5. 最多 3 轮后强制投票
6. 添加 `DeliberationConfig::max_rounds` 和 `require_consensus: bool`

**验收**：Council 支持多轮辩论，成员可在投票前修改立场。复杂决策质量提升。

### 3.3 Step 3（P1 — 通信革命）：WebSocket + 三端实时同步

#### GAP-B50-09（CRITICAL）：WebSocket 实时通信层

**文件**：新建 `src/protocol/websocket.rs` + 修改 `gui/src/backend.rs` + `vscode-addon/src/runtimeManager.ts`

**问题**：所有三端通信均为 HTTP 请求/响应模式。GUI、VSCode 与 Backend 之间无持久双向连接。无法实现实时推送（如后台任务进度、Council 投票通知、告警推送）。

**修复**：
1. Backend 新增 WebSocket endpoint：`ws://{host}/ws`
2. `WebSocketHub` 结构体管理连接含 `connections: HashMap<ConnectionId, WsSender>` 和 `topic_subscriptions: HashMap<String, Vec<ConnectionId>>`
3. 消息类型：`task.progress`、`council.vote`、`alert.triggered`、`agent.status`、`chat.token`（替代 SSE）、`chat.control`（取消/暂停）
4. GUI `WebSocketClient`：`tokio_tungstenite` 连接 + 自动重连
5. VSCode `WebSocketClient`：通过 `ws` npm 包（或在 child process 侧代理）
6. HTTP 作为 fallback（降级优雅）

**验收**：Backend 实时推送任务进度、告警到已连接的 GUI/VSCode。延迟 <50ms。

#### GAP-B50-10（HIGH）：三端会话状态同步

**文件**：新建 `src/protocol/session_sync.rs` + 修改 `gui/` 和 `vscode-addon/`

**问题**：GUI 和 VSCode 独立运行，无共享会话状态。用户在 VSCode 中发起的任务在 GUI 中不可见，反之亦然。VSCode sessions 存储在 `globalState`，GUI 在 `chat_sessions.json`——两端零同步。

**修复**：
1. Backend 新增 `SessionRegistry` 含 `sessions: HashMap<SessionId, SharedSession>` 和 `frontend_connections: HashMap<FrontendId, Vec<SessionId>>`
2. `SharedSession` 含 `chat_history`, `active_tasks`, `council_proposals`, `last_active`
3. 一个 Session 可被多个 frontend 同时连接
4. 任一 frontend 的操作通过 WebSocket 广播到同一 Session 的所有连接
5. `frontend_sync` 协议：增量 diff 同步（而非全量）
6. GUI 和 VSCode 各维护一个 `SyncState` 追踪已同步到的版本号

**验收**：在 VSCode 中发送消息 → GUI 中实时看到对话更新。两个 frontend 共享同一会话状态，<500ms 双向同步。

#### GAP-B50-11（MEDIUM）：VSCode stdio 消息分帧协议

**文件**：`vscode-addon/src/runtimeManager.ts`

**问题**：VSCode 与 Backend 的 stdio 通信仅按行解析 JSON，无消息分帧协议。stdin 反压时回退 HTTP，但可能丢失消息顺序。无消息边界保护。

**修复**：
1. 采用长度前缀分帧协议：`[4-byte BE length][JSON payload]`
2. Backend 侧新增 `FramedStdioTransport`
3. VSCode 侧新增 `FramedReader`/`FramedWriter` 类
4. 每条消息含 `message_id` 用于去重
5. 添加 heartbeat ping/pong（每 30 秒）检测连接存活
6. 兼容模式：检测到非分帧消息时回退到行解析

**验收**：stdin 反压下消息不丢失，分帧边界清晰。heartbeat 检测连接存活。

### 3.4 Step 4（P1 — 智能深化）：推理融合 + 反馈闭环

#### GAP-B50-12（MEDIUM）：MultiModelVoter 推理融合

**文件**：`src/intelligence/multi_model_voter.rs`

**问题**：Voter 只选出单一获胜响应，不进行多模型答案的推理融合或矛盾检测。聚合策略 Majority/Weighted/BestOfN 最终只返回一个模型的答案。

**修复**：
1. 新增 `VotingStrategy::Fusion`：收集所有模型响应 → 调用"聚合模型"合并推理 → 检测矛盾点并标记 → 输出融合响应 + 每个源模型的贡献权重
2. 新增 `VotingOutcome::contradictions: Vec<Contradiction>` 字段
3. `Contradiction` 结构：`{ models: Vec<String>, topic: String, positions: Vec<String> }`
4. 保留现有 Majority/Weighted/BestOfN 策略

**验收**：多模型投票输出融合后的答案，明确标注观点差异和矛盾。

#### GAP-B50-13（MEDIUM）：Metacognitive → Planner 反馈闭环

**文件**：`src/intelligence/metacognitive.rs` + `src/orchestration/brain_loop.rs`

**问题**：Metacognitive 记录观察→提出纠正行动→执行→完成，但纠正行动的效果未反馈回任务规划器。无法避免重复同样的错误。O(n) 线性查找 `get_observation`/`resolve_observation`/`propose_action`。

**修复**：
1. `CorrectiveAction::result: Option<CorrectiveResult>` 新增字段
2. `CorrectiveResult` 包含：`success: bool`、`root_cause: String`、`preventive_measures: Vec<String>`
3. Planner 在制定新计划时查询 `MetacognitiveController::get_historical_actions(task_type)` 获取历史纠正记录
4. 自动将 `preventive_measures` 注入新计划的约束条件
5. 同类错误累计 3 次 → 触发 `PlannerHint` 警告
6. 添加 HashMap 索引：`observation_id → ExecutionObservation`，实现 O(1) 查找

**验收**：同样的错误不会重复发生。系统从失败中学习并改进计划。Metacognitive 查找 O(1)。

#### GAP-B50-14（LOW）：SelfModel 动态能力评估

**文件**：`src/intelligence/self_model.rs` + `src/observability/live_performance.rs`

**问题**：SelfModel 的能力评分（`effectiveness`/`confidence`）是静态配置，未被运行时动态更新。LivePerformanceFeed 未接入 SelfModel。

**修复**：
1. `SelfModelCore::record_execution_result(capability_name, success, latency)`
2. 内部使用 EMA（指数移动平均）更新 `effectiveness` 和 `confidence`
3. `profile()` 方法返回 `SelfProfile` 含最新动态指标
4. `capability_gaps()` 方法识别 effectiveness < 0.5 的能力
5. Council 使用自模型数据调整成员权重

**验收**：自模型能力评分随运行时表现自动更新。低效能力自动标记。

### 3.5 Step 5（P2 — 稳健性加固）：内存管理 + 性能优化

#### GAP-B50-15（MEDIUM）：RateLimitMiddleware 租户淘汰

**文件**：`src/protocol/rate_limit.rs`

**问题**：`HashMap<String, TokenBucket>` 永不驱逐闲置租户，造成无界内存增长。LivePerformanceFeed 模型的 EMA 数据同样永不驱逐。

**修复**：
1. 每个 `TokenBucket` 新增 `last_access: Instant`
2. 新增 `TenantLimitConfig::idle_timeout_seconds: u64`（默认 3600）
3. `check_rate_limit()` 调用时触发惰性淘汰：遍历 entry，移除超过 idle_timeout 的闲置租户
4. LivePerformanceFeed 同样添加 `last_access` 和驱逐逻辑
5. 添加 `evict_tenant(tenant_id)` 公共 API

**验收**：闲置 1 小时+ 的租户自动移除。LivePerformanceFeed 废弃模型自动清除。内存不再无界增长。

#### GAP-B50-16（MEDIUM）：SemanticCache 真 LRU + 后台 TTL 清理

**文件**：`src/memory/semantic_cache.rs`

**问题**：FIFO 驱逐（按 `created_at`），`access_count` 字段存在但未用于驱逐策略。TTL 过期条目仅在下一次 `get()` 时清理。使用 `DefaultHasher`（SipHash）较慢。

**修复**：
1. `access_count` → 用于 LRU 驱逐：每次 `get()` 命中时递增
2. `evict_lru()` 改为驱逐 access_count 最小的条目（真 LRU）
3. 新增 `background_cleanup_interval: Duration`（默认 300s）
4. `start_background_cleanup()` 使用 `tokio::spawn` + `tokio::time::interval` 定期清理过期条目
5. `stop_background_cleanup()` 使用 `CancellationToken`
6. 添加 `SemanticCacheStats::expired_count` 跟踪清理量

**验收**：经常访问的条目不会被驱逐。过期条目在 5 分钟内被清理。

#### GAP-B50-17（LOW）：MemoryStore 插入性能优化

**文件**：`src/memory/memory.rs`

**问题**：每次 `store()` 执行 3 次 O(n) 线性扫描（class entries 计数→找 oldest→检查 global total）。

**修复**：
1. `MemoryStore` 新增 `class_counts: HashMap<MemoryClass, usize>`（O(1) 计数）
2. 新增 `entries_by_class: HashMap<MemoryClass, BTreeMap<u64, String>>`（按时间戳索引）
3. `store()` 流程变为：O(1) 检查 class_counts + 1 > capacity → O(log n) 查找 oldest timestamp → 移除 → 移除 global oldest 改为 O(log n) 查找 global BTreeMap min
4. 存储后 `class_counts` 增量更新

**验收**：`store()` 时间复杂度从 O(n) 降为 O(log n)。

#### GAP-B50-18（LOW）：AlertManager 告警环形缓冲区

**文件**：`src/observability/alert_manager.rs`

**问题**：仅有 `total_alerts_fired` 计数器，无法查看历史告警。Webhook 不含历史上下文。

**修复**：
1. 新增 `recent_alerts: VecDeque<Alert>`（max 100）
2. `evaluate()` 触发告警时 push_front + 超过 100 则 pop_back
3. 新增 `get_recent_alerts() -> Vec<Alert>` API
4. 接入 health 端点 `/health/alerts` 返回最近告警
5. Webhook payload 包含 `recent_alerts_count`

**验收**：可通过 API 查询最近 100 条告警。Webhook 包含告警历史摘要。

#### GAP-B50-19（LOW）：Chaos.rs 锁中毒恢复统一

**文件**：`src/resilience/chaos.rs`

**问题**：使用 `RwLock::expect()` 直接 panic，不符合项目 poison recovery 规范（transport/rate_limit 使用 `unwrap_or_else(poisoned.into_inner())`）。

**修复**：
1. 将 `expect("B49: chaos injections lock")` 替换为 `unwrap_or_else(|poisoned| { warn!(...); poisoned.into_inner() })`
2. 统一使用项目共享的 `lock_guard()` 或内联 recover 模式
3. 不新增 panic 路径

**验收**：ChaosEngine 不再因锁中毒 panic。与 transport/rate_limit 行为一致。

### 3.6 Step 6（P2 — 基准观测）：端到端性能基准

#### GAP-B50-20（LOW）：端到端流式性能 Benchmark

**文件**：`tests/streaming_e2e_benchmark.rs`（新建）

**问题**：现有测试覆盖功能和集成，无流式端到端性能测量。`comprehensive_feature_benchmark` 21 维度中 9 个为 Qualitative（score 0.0），实际覆盖仅 57%。

**修复**：
1. 测量指标：`time_to_first_token_ms`（TTFT）p50/p95/p99、`tokens_per_second`（TPS）、`time_to_complete_ms`（TTC）、`stream_interrupt_latency_ms`
2. 分别在 GUI 模式、VSCode 模式、纯 HTTP 模式下测量
3. 记录 3 个 server profile 下的性能差异
4. CI 中作为性能回归检测（不允许 TTFT p50 > 基线 × 1.5）

**验收**：CI 中运行端到端流式性能测试，可检测性能回归。

### 3.7 Step 7（P0 — 架构整合）：孤岛模块接入热路径

#### GAP-B50-21（CRITICAL）：CapabilityBus 接入请求生命周期

**文件**：`src/intelligence/capability_bus/core.rs` + `src/acp/impl/chat.rs`

**问题**：CapabilityBus 是 Sense→Decide→Act→Feedback→Evolve 完整认知管线，7,200 行代码，但 `new()` 在整个代码库中零调用点。全部子 Bus（memory/observability/optimization/orchestration/protocol/tool/distributed_memory）均仅通过 CapabilityBus 可达，从未运行。同时 CapabilityGraph 未从 AgentRegistry 喂入数据，Reputation 数据未流入 AgentRegistry。

**修复**：
1. 在 `AcpServer` 中新增 `capability_bus: Option<Arc<CapabilityBus>>` 字段
2. 在 `chat_pack.rs` 的每个请求完成时调用 `capability_bus.feedback()` 记录结果
3. 定期（每 50 个请求）调用 `capability_bus.evolve()` 执行进化周期（ContinuousLearning、Discovery、Evolution、FederatedRL）
4. `capability_bus.sense()` 在模型选择前查询当前环境状态（agent 健康、负载、延迟）
5. `capability_bus.decide()` 输出模型选择建议，与现有 `ModelSelector` 并行决策
6. 将 AgentRegistry 数据喂入 CapabilityGraph
7. CapabilityBus 的 Reputation 数据流入 AgentRegistry
8. 添加 `CapabilityBusConfig::evolve_interval` 和 `enable_capability_bus: bool`

**验收**：CapabilityBus 每次请求参与 sense→feedback 循环。50 请求一次 evolution。所有子 Bus 活跃运行。Reputation 衰减生效。

#### GAP-B50-22（CRITICAL）：Scheduler 接入任务执行路径（含竞态修复）

**文件**：`src/orchestration/scheduler.rs` + `src/acp/impl/chat.rs`

**问题**：双层调度器 800 行完整实现（L1 优先队列 + L2 工作者池）含信号量并发、aging 反饥饿、SQLite 持久化、反压机制——全部实现但从未被调用。同时 `fail()` 函数存在 TOCTOU 竞态（task_map 读锁释放后、queues 写锁获取前可能被 dequeue 双次执行）。`apply_aging()` 双重锁问题。

**修复**：
1. 修复 `fail()` TOCTOU 竞态：将 task_map 读 + queues 写合并到单一锁作用域
2. 修复 `apply_aging()` 双重锁：使用 snapshot-then-rebuild 模式
3. 在 `AcpServer` 中新增 `scheduler: Arc<AgentWorkerScheduler>` 字段
4. 在 `chat_pack.rs` 中：新任务 → `scheduler.submit()`，agent 执行 → `scheduler.assign_next()`
5. 启动后台 `tokio::spawn` 定时器调用 `apply_aging()`（每 5 秒）
6. 集成反压：`backpressure_queue_depth` 超限 → 429 拒绝
7. 集成 SQLite 持久化：重启恢复 pending 任务

**验收**：任务通过 Scheduler 排队、分配、执行。Aging 生效（长等待任务优先级提升）。竞态修复。

#### GAP-B50-23（CRITICAL）：HarnessBus 策略引擎接入

**文件**：`src/governance/harness_bus.rs` + `src/acp/impl/chat.rs`

**问题**：HarnessBus 聚合全部 governance 组件（Pua/Sandbox/Budget/Idempotency/RBAC/SecurityGovernor/Drift/FaultTolerance/BrainLoop）但标注 "Phase 0 实现 / 等待 CapabilityBus 集成"，未接入请求生命周期。`evaluate()`/`validate()`/`verify()` 入口仅存在于注释。

**修复**：
1. 在 `AcpServer` 中新增 `harness_bus: Arc<HarnessBus>` 字段
2. 在每个请求处理中插入 3 个检查点：
   - **pre-execute**：`harness_bus.evaluate_before()` — PUA 规则/RBAC/预算检查
   - **during-execute**：`harness_bus.validate_during()` — 沙箱策略/反压/在线控制
   - **post-execute**：`harness_bus.verify_after()` — 审计日志/漂移检测/反馈收集
3. 决策枚举：Allow/Deny/RequireReview/Escalate — Deny 直接返回 403
4. 漂移检测接入 `DriftProtectionEngine.check_for_drift()` 后台定时器
5. 修复 Hardening 租户预算竞态：`Cell<i64>` → `AtomicI64`

**验收**：每个请求经过 HarnessBus 三阶段检查。PUA 规则生效。Deny 请求返回 403。Drift 检测运行。

#### GAP-B50-24（HIGH）：Schema 规范类型接入 Handler

**文件**：`src/schema/` + `src/acp/impl/`

**问题**：ACP v0.13.2 规范类型 500+ 行全部 `#[allow(dead_code)]` F-GAP-25。Handler 使用自己的 ad-hoc 类型而非这些规范类型。

**修复**：
1. `InitializeRequest` / `InitializeResponse` → 替换 `acp/impl/` 中的 ad-hoc 初始化类型
2. `ContentBlock` / `TextContent` / `ImageContent` → 替换 chat handler 中的内容类型
3. `SessionPromptRequest` / `SessionNotification` → 替换 request handler 中的 session 类型
4. `ToolCall` / `Plan` / `CurrentModeUpdate` → 替换 client notification 类型
5. 所有替换保持向后兼容：新增 `From<OldType> for SchemaType` 转换 trait

**验收**：Handler 使用 schema 规范类型。`#[allow(dead_code)]` 从 schema/ 移除。

#### GAP-B50-25（HIGH）：ThreadSafeAuditLog 替代旧版

**文件**：`src/governance/audit.rs` + `src/acp/server.rs`

**问题**：ThreadSafeAuditLog（NDJSON 持久化 + 敏感字段脱敏 `redact_sensitive()`）已实现但标记 F-GAP-49 未集成。生产代码使用旧版单线程 `AuditLog`。

**修复**：
1. 在 `AcpServer` 中将 `audit_log: Arc<Mutex<AuditLog>>` 替换为 `ThreadSafeAuditLog`
2. `ThreadSafeAuditLog` 的 NDJSON 持久化路径：`~/.goon/audit.ndjson`
3. 添加审计日志轮转：文件 > 100MB → 压缩归档
4. 敏感脱敏函数 `redact_sensitive()` 应用到所有审计条目
5. 接入 health 端点 `/health/audit` 返回审计统计（总条目数、最后写入时间）

**验收**：审计日志持久化到磁盘，敏感字段脱敏。重启不丢失。

#### GAP-B50-26（HIGH）：TokenCache 包装 AgentRegistry

**文件**：`src/intelligence/token_cache/mod.rs` + `src/agents/agent.rs`

**问题**：`CachedAgentWrapper` 是 Agent trait 的 drop-in 包装器（三级缓存：L1 exact→L2 semantic→L3 template），可直接节省 token 成本，但从未在 AgentRegistry 中包装任何 Agent。1,059 行代码闲置。

**修复**：
1. 在 `AgentRegistry::register()` 中自动用 `CachedAgentWrapper::new(agent)` 包装
2. 添加 `AgentRegistryConfig::enable_token_cache: bool`（默认 true）
3. L1 精确缓存：相同 request hash → 直接返回缓存响应
4. L2 语义缓存：embedding cosine 相似度 > 0.92 → 返回
5. L3 模板缓存：提取结构签名 → 匹配模板
6. 缓存统计接入 `governance.status` 端点

**验收**：重复请求从缓存返回，token 用量降低 20%+。

#### GAP-B50-27（HIGH）：FullAutoFlow 注入真实注册表

**文件**：`src/orchestration/full_auto.rs`

**问题**：`run_full_auto_flow()` 创建空的 `SkillRegistry` 和 `ToolRegistry`——全自动流程以零技能零工具运行，无法执行实质性任务。

**修复**：
1. `FullAutoFlow` 新增构造函数 `new_with_registries(skill_registry, tool_registry)`
2. 从 `AcpServer` 传入真实的 `skill_registry: Arc<Mutex<SkillRegistry>>`
3. 从 `AcpServer` 传入真实的 `tool_registry: Arc<ToolRegistry>`
4. 添加 `FullAutoFlowConfig::max_skills_per_task` 控制技能使用
5. 无技能匹配时回退到通用工具（read_file/write_file 等）

**验收**：FullAutoFlow 可发现和使用已注册的技能和工具。

### 3.8 Step 8（P1 — 测试防线）：关键路径测试覆盖

#### GAP-B50-28（CRITICAL）：ACP helpers/ 核心文件添加测试

**文件**：`src/acp/helpers/` 8 个核心文件

**问题**：ACP helpers/ 36 文件全部零单元测试。这是项目中最大的测试黑洞。agent 选择、autonomy 循环、governance 策略、cache 策略、conversation 管理——全部在无测试情况下运行。

**修复**：
1. 优先覆盖 8 个核心 helper（chat.rs 直接 import 的）：`agent/agent_selector.rs`（letter bias 消除+多因子评分）、`autonomy/autonomy_loop.rs`（TAO 循环状态转换）、`governance/policy.rs`（策略评估）、`response/response_assembler.rs`（响应组装）、`cache_strategy.rs`（缓存策略选择）、`context.rs`（上下文合并）、`conversation.rs`（对话状态管理）、`review_gate.rs`（审查门控）
2. 每个 helper 至少 3 个测试用例：正常路径 + 错误路径 + 边界条件

**验收**：8 个核心 helper 通过 `cargo test`。覆盖率基线建立。

#### GAP-B50-29（CRITICAL）：ACP impl/ handler packs 添加测试

**文件**：`src/acp/impl/` 6 个关键 handler

**问题**：ACP impl/ 28 个文件仅 `chat_tests.rs` 有测试，其余 27 个 handler pack 文件全部零单元测试。请求分发、错误边界、协议边界条件无覆盖。

**修复**：
1. 优先覆盖请求分发和错误边界：`request.rs`（ACP/MCP 方法分发+未知方法处理+参数校验）、`exec_workflow_pack.rs`（工作流执行+步骤失败回滚）、`governance_status_pack.rs`（治理状态查询）、`health_pack.rs`（健康检查端点）、`tools_pack.rs`（工具列表/调用）
2. 每个 handler pack 至少 2 个测试用例
3. 添加 protocol 边界测试（JSON-RPC 格式错误、超大 payload）

**验收**：关键 handler packs 通过测试。协议边界条件覆盖。

#### GAP-B50-30（HIGH）：transport_factory + OrchestrationContext 测试

**文件**：`src/acp/transport_factory.rs` + `src/orchestration/context.rs`

**问题**：transport_factory 零测试（多后端 cache/vector 初始化、5 种协议模式分发——全部无测试，feature-gated 分支行为无覆盖）。OrchestrationContext 仅构造测试（`record_model_execution()` 行为、failover 集成、并发访问——无行为测试）。

**修复**：
1. transport_factory 测试：测试 5 种协议模式 dispatch（acp_stdio/acp_http/mcp_stdio/mcp_http/adaptive）→ 测试 cache/vector 初始化失败 fallback → 测试 feature-gated 分支（Postgres vs file backend）
2. OrchestrationContext 测试：测试 `record_model_execution()` → LivePerformanceFeed 更新 → 测试 `HotFailover` 集成（模型失败→自动切换）→ 测试并发 `record_model_execution()` 无竞态

**验收**：启动路径有测试覆盖。OrchestrationContext 行为验证。

### 3.9 Step 9（P1 — 生产加固）：可观测性 + 安全 + 韧性

#### GAP-B50-31（MEDIUM）：Graceful Shutdown 编排

**文件**：`src/acp/server.rs`

**问题**：`LifecycleState.is_shutting_down()` 和 `shutdown_drain_seconds` 存在，但无结构化 drain 序列。`InflightLimiter` 存在但无 `DrainGuard` 或有序子系统 teardown。

**修复**：
1. 实现 `DrainGuard` struct：`{ draining: AtomicBool, inflight: Arc<Semaphore>, drain_timeout: Duration }`
2. `start_drain()`：设置 draining=true，拒绝新请求（返回 503 + Retry-After）
3. `wait_for_drain()`：等待 inflight 降为 0 或 timeout
4. 关机顺序：stop_accepting → drain_requests → stop_background_tasks → close_db → exit
5. 添加 `/health/ready` 端点（draining 时返回 503）

**验收**：SIGTERM 后优雅 drain，无请求丢失。

#### GAP-B50-32（MEDIUM）：Structured Tracing Span Propagation

**文件**：全局

**问题**：tracing 使用广泛，但 AcpServer→FlowManager→Agent 调用链无 Span 传播。Request ID 未一致串入 span，分布式追踪/调试困难。

**修复**：
1. 在请求入口创建 root span：`tracing::info_span!("request", request_id = %uuid)`
2. 在 `process_chat_request` 每个步骤创建 child span
3. 在 Agent 调用中传递 parent span context
4. `AcpServer` 的所有 async 方法使用 `#[tracing::instrument]`
5. 添加 `trace_id` 到所有错误消息中

**验收**：Jaeger/Zipkin 中可见完整请求追踪链。

#### GAP-B50-33（MEDIUM）：内存健康监控集成

**文件**：`src/observability/memory_health/mod.rs` + `src/main.rs`

**问题**：memory_health 模块近孤岛——`check_startup_memory()`/`start_memory_monitor()`/`query_*_memory()` 全部实现但零调用点。未在 main.rs 或 transport_factory.rs 中调用。

**修复**：
1. 在 `main.rs` 启动中调用 `check_startup_memory()` → 打印可用内存诊断
2. 在 `background.rs` 中 `tokio::spawn` 调用 `start_memory_monitor()`
3. `MemoryJetsamRisk` 阈值触发 → 写入告警日志 + 触发 `AlertManager.evaluate()`
4. `query_macos_memory()` / `query_linux_memory()` / `query_windows_memory()` 根据 OS 自动选择

**验收**：启动打印内存诊断。内存 > 90% 触发告警。

#### GAP-B50-34（MEDIUM）：FaultToleranceEngine 状态持久化

**文件**：`src/fault_tolerance.rs`

**问题**：全部内存存储。重启后心跳记录、故障、隔离组、恢复计划全部丢失。已有 SQLite 依赖但未用于状态持久化。

**修复**：
1. `FaultToleranceInner` 新增 `save_to_db()` / `load_from_db()` 方法
2. 使用已有 SQLite 连接（通过 `FaultToleranceConfig` 传入）
3. 持久化表：faults、recovery_plans、isolation_groups、heartbeat_records
4. 启动时 `load_from_db()` 恢复上次状态
5. `report_fault()` 时同步写入（async 批处理，每 5 秒 flush）

**验收**：重启后故障记录和隔离组状态恢复。

#### GAP-B50-35（MEDIUM）：Prometheus /metrics 端点

**文件**：`src/observability/` + `src/acp/impl/request/metrics_pack.rs`

**问题**：`HistogramBuckets` 常量和 `RuntimeMetrics` 存在，但无可见的 Prometheus 导出端点。`otel_exporter` 配置字段存在但实际 OTLP 导出接线不明。

**修复**：
1. 添加 `/metrics` HTTP 端点（使用现有 `observability::prometheus_text_format` helpers）
2. 导出指标：request_count、request_duration_seconds、inflight_requests、circuit_breaker_state
3. 导出指标：agent_success_rate、p95_latency_ms、cache_hit_ratio、error_rate
4. 可选：`metrics` crate 集成（behind feature flag `prometheus-metrics`）

**验收**：`curl /metrics` 返回 Prometheus 格式指标。

#### GAP-B50-36（MEDIUM）：服务端 Auth 中间件

**文件**：`src/acp/impl/request.rs` + `src/acp/impl/session.rs`

**问题**：rbac/security_governor/entry_auth_api_key_env 配置存在，但 ACP 服务器请求路径无可见 auth 中间件。ACP 协议的 `authenticate`/`logout` 方法在 schema types 中定义但 handler 中无实现。

**修复**：
1. 在 `request.rs` dispatch 前插入 `authenticate_request()` 调用
2. 支持 3 种认证方式：Bearer token（`Authorization: Bearer <jwt>`）、API Key（`X-API-Key` header）、Session cookie（`go-on-session`）
3. 认证后设置 `RequestContext.principal` → RBAC Enforcer 检查权限
4. 无认证模式（local profile）保持向后兼容：默认 admin 权限
5. Session 管理：`session.rs` 已有框架，接入 auth 流程

**验收**：multi-users-server profile 下请求需认证。local profile 保持开放。

#### GAP-B50-37（LOW）：Config 热加载改用 notify 事件

**文件**：`src/core/config/hot_reload.rs`

**问题**：WatchDog 使用文件元数据轮询（`tokio::time::sleep`），非 notify crate 的文件系统事件。变更需最多 500ms 传播，持续 CPU 唤醒。

**修复**：
1. 引入 `notify` crate（已有间接依赖）
2. `WatchDog` 使用 `notify::RecommendedWatcher` 替代 tokio::time::sleep 轮询
3. 处理 debounce（`notify::DebouncedEvent` 已内置）
4. 失败时回退到轮询

**验收**：配置变更即时传播（<100ms），无 CPU 轮询开销。

### 3.10 Step 10（P2 — 代码质量）：技术债务清理

#### GAP-B50-38（MEDIUM）：AcpServer God Object 分解

**文件**：`src/acp/server.rs`

**问题**：`AcpServer` 单一 struct 持有 40+ 字段（flow_manager、agent_registry、cache、vector、autotune、observability、controller、circuit_breakers、governance、orchestration 等）。严重违反单一职责原则。

**修复**：
1. 提取 `CacheServerDeps`（cache + vector + autotune）
2. 提取 `ModelServerDeps`（flow_manager + agent_registry + model_selector + adaptive_selector）
3. 提取 `GovernanceServerDeps`（harness_bus + capability_bus + audit + pua + rbac）
4. 提取 `OrchestrationServerDeps`（scheduler + planner + executor + skill_registry）
5. `AcpServer` 保留核心字段（runtime_config + lifecycle_state + http_listener）
6. 不影响现有公开 API，仅内部重构

**验收**：AcpServer 字段 < 15 个。每个 Deps 组独立可测试。

#### GAP-B50-39（MEDIUM）：FederatedRL 两套实现合并

**文件**：`src/intelligence/federated_rl.rs` + `src/intelligence/reinforcement/federated.rs`

**问题**：两套独立 FederatedRL 实现（`federated_rl.rs` 和 `reinforcement/federated.rs`），使用不同 API，均未接通。代码重复且无人使用。

**修复**：
1. 选择 `reinforcement/federated.rs` 作为主实现（更完整：FedAvg/FedWeighted/FedMedian + client 注册）
2. 将 `federated_rl.rs` 中的 `FederatedRL` struct 迁移到 `reinforcement/federated.rs`
3. 删除 `federated_rl.rs`，在 `mod.rs` 中重导出
4. CapabilityBus 中字段指向统一实现

**验收**：单一 FederatedRL 实现，无代码重复。

#### GAP-B50-40（LOW）：Wildcard re-exports 显式化

**文件**：`src/core/config/mod.rs`

**问题**：`pub use autotune::*` / `pub use defaults::*` / `pub use types::*` 扁平化命名空间，无法追踪类型来源。

**修复**：
1. 替换 `pub use autotune::*` 为显式导出列表
2. 仅导出公开 API 类型（AppConfig、AutoTuneConfig 等 ~15 个核心类型）
3. 内部类型保留 `pub(crate)`

**验收**：`use crate::config::*` 不再意外导入内部类型。

#### GAP-B50-41（LOW）：统一 async 锁策略

**文件**：全局

**问题**：tokio::sync::Mutex、std::sync::Mutex、std::sync::RwLock 混用。FaultToleranceEngine 使用阻塞型 `lock()`——在 async 上下文中会 stall tokio executor 线程。AcpServer 内字段同时使用 `Arc<Mutex<>>`(tokio) 和 `Arc<StdMutex<>>`，无明确理由。

**修复**：
1. 建立规范：async 上下文中使用 `tokio::sync::Mutex` 或 `tokio::sync::RwLock`
2. 同步上下文中使用 `std::sync::Mutex` 或 `std::sync::RwLock`
3. FaultToleranceEngine 从 `std::sync::Mutex` → `tokio::sync::RwLock`（读多写少）
4. RateLimitMiddleware/SemanticCache/LivePerformanceFeed 保持 `std::sync::Mutex`（同步调用路径）
5. 代码审查规则：`std::sync::Mutex::lock()` 不出现在 async fn 中

**验收**：锁类型使用一致。无 async 上下文中阻塞锁。

---

## 4. 执行计划 v2（10 Step / 41 GAP）

| Step | 描述 | 优先级 | 天数 | GAP数 | 新增测试 |
|:---:|:-----|:------:|:---:|:-----:|:--------:|
| 1 | 速度革命：流式+异步+投机 | P0 | 3-4 | 4 | 10 |
| 2 | 智能升级：CoT+深度推理+真语义+辩论 | P0 | 3-4 | 4 | 10 |
| 3 | 通信革命：WebSocket+同步+分帧 | P1 | 2-3 | 3 | 8 |
| 4 | 智能深化：融合+闭环+SelfModel | P1 | 2 | 3 | 5 |
| 5 | 稳健性加固：内存+LRU+O(log n)+告警 | P2 | 2 | 5 | 6 |
| 6 | 基准观测：流式性能基准CI | P2 | 1-2 | 1 | 4 |
| 7 | 架构整合：CapBus+Scheduler+HarnessBus+Schema | P0 | 4-5 | 7 | 17 |
| 8 | 测试防线：Helpers+Handlers+Transport | P1 | 3 | 3 | 42+ |
| 9 | 生产加固：Shutdown+Tracing+Mem+Auth | P1 | 3 | 7 | 11 |
| 10 | 代码质量：GodObject+合并+锁统一 | P2 | 2 | 4 | 0 |
| **总计** | | | **25-32天** | **41** | **113+** |

---

## 5. 全层验证计划

### 5.1 编译验证
| 验证项 | 标准 |
|:-------|:-----|
| `profile-local` clippy `--all-features -- -D warnings` | 零错误零警告 |
| `profile-simple-server` clippy | 零错误零警告 |
| `profile-multi-users-server` clippy | 零错误零警告 |
| GUI `cargo clippy -- -D warnings` | 零错误零警告 |
| VSCode `npx tsc --noEmit && npx eslint src/` | 零错误 |
| `cargo test --lib --no-run` | 编译通过 |

### 5.2 运行时验证
| 验证项 | 测量指标 | 目标 |
|:-------|:---------|:----:|
| GUI 流式首 token 延迟 | p50 | <200ms |
| VSCode 流式首 token 延迟 | p50 | <300ms |
| WebSocket 推送延迟 | p50 | <50ms |
| DAG 投机执行加速比 | 总延迟减少 | >20% |
| SemanticCache 语义命中率 | vs bigram基线 | +15% |
| BrainLoop 异步化延迟降低 | 总延迟减少 | >30% |
| CapabilityBus evolve周期 | 每50请求 | <100ms开销 |
| Scheduler aging | 长等待优先级提升 | 检测优先反转消失 |
| TokenCache命中率 | 重复请求 | >20% token节省 |
| HarnessBus策略检查 | 请求延迟增加 | <5ms |

### 5.3 功能验证
| 验证项 | 标准 |
|:-------|:-----|
| GUI↔VSCode会话同步 | 消息<500ms双向同步 |
| Council辩论 | 多轮审议+最终投票正确 |
| Metacognitive反馈闭环 | 同类错误不重复 |
| 内存淘汰 | RateLimit/LivePerformance闲置条目自动清除 |
| WebSocket断线重连 | 自动重连+消息不丢失 |
| GracefulShutdown | SIGTERM后drain完成零请求丢失 |
| Prometheus /metrics | curl返回正确格式+指标值 |
| Auth中间件 | Bearer/API Key/Session Cookie + RBAC检查 |
| FaultTolerance持久化 | 重启后故障记录恢复 |

---

## 6. 完成率追踪 v2

| Step | 描述 | 状态 | 完成内容 |
|:---|:-----|:----:|:---------|
| Step 1: 速度革命 | GUI/VSCode流式 + BrainLoop异步 + DAG投机 | ✅ | GAP-B50-01~04: GUI流式SSE/AbortController, VSCode流式Chat+StreamProcessor, BrainLoop异步化(RwLock+async), DAG投机执行+TaskContext |
| Step 2: 智能升级 | CoT上下文 + 深度推理 + 真语义缓存 + Council辩论 | ✅ | GAP-B50-05~08: TaskContext+BrainLoop集成, DeepReasoningEngine, EmbeddingSemanticCache+SimpleEmbeddingCache+RemoteEmbeddingCache, Council多轮辩论 |
| Step 3: 通信革命 | WebSocket + 三端同步 + stdio分帧 | ✅ | GAP-B50-09~11: WebSocketHub(topic pub/sub+heartbeat+wildcard), SessionRegistry(versioned sync+cleanup), VSCode FramedReader/FramedWriter+heartbeat+dedup |
| Step 4: 智能深化 | 推理融合 + 反馈闭环 + SelfModel动态化 | ✅ | GAP-B50-12~14: VotingStrategy::Fusion+FusionEngine+Contradiction, Metacognitive→Planner闭环+O(1)索引, SelfModel EMA动态评估+LivePerformance集成 |
| Step 5: 稳健性加固 | 内存淘汰 + 真LRU + O(log n) + 告警历史 | ✅ | GAP-B50-15~19: RateLimit租户淘汰+TokenBucket idle, SemanticCache真LRU+后台TTL, MemoryStore O(log n)优化, AlertManager环形缓冲100条, Chaos锁中毒恢复统一 |
| Step 6: 基准观测 | 端到端流式性能基准 | ✅ | GAP-B50-20: streaming_e2e_benchmark.rs (TTFT/TPS/TTC p50/p95/p99, 3模式, 回归检测) |
| Step 7: 架构整合 | CapabilityBus+Scheduler+HarnessBus+Schema+Audit+TokenCache+FullAuto | ✅ | GAP-B50-21~27: CapBus sense/decide/feedback/evolve接入, Scheduler TOCTOU修复+submit/assign_next, HarnessBus三阶段检查+Drift监控, Schema类型替换ad-hoc, ThreadSafeAuditLog NDJSON+rotation, TokenCache包装AgentRegistry, FullAutoFlow真实注册表 |
| Step 8: 测试防线 | Helpers+Handlers+TransportFactory+Context测试覆盖 | ✅ | GAP-B50-28~30: 8核心helper 3+测试用例, 6 handler pack 2+测试用例, transport_factory+OrchestrationContext行为测试 |
| Step 9: 生产加固 | GracefulShutdown+Tracing+MemHealth+FaultPersist+Prometheus+Auth | ✅ | GAP-B50-31~37: DrainGuard 5段关机, Tracing root/child span, 内存监控+告警, FaultTolerance SQLite持久化, /metrics端点, Auth中间件3模式, Config notify热加载 |
| Step 10: 代码质量 | AcpServer分解+FedRL合并+Wildcard清理+锁统一 | ✅ | GAP-B50-38~41: AcpServer 4组Deps拆分, FederatedRL统一实现, Wildcard→显式导出, tokio::sync::RwLock统一锁策略 |
| **总计** | **10 Step / 41 GAP** | **✅ 100%** | all 41 GAP completed, 0 warnings/errors |

---

## 7. 维度预期提升 v2

| 维度 | BLUE49 | BLUE50 v2 核心提升 | BLUE50 |
|:----:|:------:|:----------------|:------:|
| **架构层** | 10/10 | CapabilityBus+Scheduler+HarnessBus接入 + TaskContext + WebSocketHub + SessionRegistry + AcpServer分解 + Schema启用 | **11/10** |
| **运行层** | 10/10 | 真流式SSE + BrainLoop异步 + DAG投机 + Scheduler调度 + O(log n) + TokenCache | **11/10** |
| **智能层** | 10/10 | CoT推理 + 深度Reflection + 真语义缓存 + 推理融合 + Council辩论 + CapBus演进 + SelfModel动态 | **11/10** |
| **治理层** | 10/10 | HarnessBus三阶段 + ThreadSafeAuditLog + RBAC escalation + Drift修复 + Hardening竞态修复 | **10+/10** |
| **协议层** | 10/10 | WebSocket + 分帧协议 + 双向实时 + Config热加载notify | **10+/10** |
| **韧性层** | 10/10 | FaultTolerance持久化 + GracefulShutdown + Chaos修复 + 内存监控 | **10+/10** |
| **可观测层** | 10/10 | Tracing Span + Prometheus /metrics + AlertManager环形缓冲 + 流式基准CI | **11/10** |
| **内存层** | 10/10 | RateLimit淘汰 + LivePerformance淘汰 + 真LRU + TTL后台清理 | **10+/10** |
| **GUI层** | 10/10 | 真流式SSE + WebSocket推送 + 会话同步 + 流取消 | **11/10** |
| **VSCode层** | 10/10 | 真流式Chat + WebSocket + 分帧协议 + 会话同步 + AbortController | **11/10** |
| **测试层** | 10/10 | 42+Helpers + 18+Handlers + 6+Transport/Context + CI基准 | **11/10** |
| **安全层** | 10/10 | Auth中间件 + RBAC escalation + AuditLog脱敏 | **10+/10** |

---

## 8. 超级智能全能打工王者评估 v2

### 8.1 当前状态（BLUE49 基线 — 代码层面10/10，运行时存在~15K行孤岛代码）

| 能力维度 | 评分 | 关键限制 |
|:---------|:---:|:---------|
| **任务理解** | 9/10 | 无CoT上下文、WorldModel未被查询 |
| **任务执行** | 7/10 | Scheduler未用、BrainLoop同步、FullAuto空注册表 |
| **多Agent协作** | 7/10 | CapabilityBus未运行、无辩论、HarnessBus未集成 |
| **自主学习** | 6/10 | CapabilityBus孤儿导致从未运行 |
| **自我认知** | 8/10 | SelfModel静态，LivePerformance未接入 |
| **错误恢复** | 9/10 | FaultTolerance无持久化 |
| **多端体验** | 7/10 | 非流式、无同步、不可取消 |
| **实时通信** | 5/10 | 仅HTTP，无WebSocket |
| **推理深度** | 6/10 | 无LLM推理，无CoT传播 |
| **速度流畅度** | 6/10 | 非流式、假LRU、无投机 |
| **架构完整性** | 4/10 | ~15K行孤岛代码（7.2k+800+500+...） |
| **测试覆盖** | 4/10 | 36+27文件零测试 |

### 8.2 BLUE50 v2 完成后预期

| 能力维度 | 评分 | 核心变化 |
|:---------|:---:|:---------|
| **任务理解** | **10/10** | CoT传播 + WorldModel查询 + 深度LLM Plan + 真语义 |
| **任务执行** | **10/10** | Scheduler调度 + BrainLoop异步 + DAG投机 + 流式 + FullAuto真实工具 |
| **多Agent协作** | **10/10** | CapBus Sense→Decide→Act→Feedback→Evolve + Council辩论 + 推理融合 + HarnessBus |
| **自主学习** | **10/10** | CapBus evolve + Metacognitive闭环 + SelfModel动态 + Reputation流入Registry |
| **自我认知** | **10/10** | SelfModel动态评估 + LivePerformance驱动 |
| **错误恢复** | **10/10** | FaultTolerance持久化 + GracefulShutdown + Chaos完善 |
| **多端体验** | **10/10** | 真流式 + 三端同步 + 实时推送 + 取消控制 |
| **实时通信** | **10/10** | WebSocket + 分帧 + 心跳 |
| **推理深度** | **10/10** | CoT + 多模型融合 + 深度LLM Reflection |
| **速度流畅度** | **10/10** | 流式SSE + 异步化 + 投机 + O(log n) + TokenCache |
| **架构完整性** | **10/10** | 0行孤岛 — 全部核心模块接入热路径 |
| **测试覆盖** | **9/10** | 42+24+6+测试 + CI基准 |

**最终评价**：BLUE50 v2完成后，系统从"代码完备"（BLUE49: 14个功能维度 10/10）真正进化为"运行卓越"（BLUE50: 12个运行时维度 10/10）的超级智能全能打工王者。

---

## 9. 附录：完整瓶颈索引（62个，41 GAP + 21 P3）

### 速度瓶颈（S1-S10）
| S1🔴 | S2🟠 | S3🟠 | S4🟠 | S5🟡 | S6🟡 | S7🟡 | S8🟡 | S9🟢 | S10🟢 |
→ GAP: B50-03,04,01,02,16,16,—,08,13,—

### 智能瓶颈（I1-I8）
| I1🔴 | I2🟠 | I3🟠 | I4🟡 | I5🟡 | I6🟡 | I7🟢 | I8🟢 |
→ GAP: B50-05,06,07,08,12,13,14,—

### 通信瓶颈（C1-C7）
| C1🔴 | C2🟠 | C3🟡 | C4🟡 | C5🟡 | C6🟢 | C7🟢 |
→ GAP: B50-09,10,11,—,—,—,—

### 内存瓶颈（M1-M5）
→ GAP: B50-15,15,17,18,16

### 孤岛模块瓶颈（O1-O15）
| O1🔴 | O2🔴 | O3🔴 | O4🟠 | O5🟠 | O6🟠 | O7🟠 | O8-O13,O15🟡/🟢 | O14🟡 |
→ GAP: B50-21,22,23,24,25,27,26,—(P3),33

### 测试覆盖瓶颈（T1-T7）
| T1🔴 | T2🔴 | T3🟠 | T4🟠 | T5🟡 | T6🟡 | T7🟢 |
→ GAP: B50-28,29,30,30,20,20,—

### 架构集成瓶颈（A1-A19）
| A1🔴 | A2-A4🟠 | A5-A16🟡 | A17-A19🟢 |
→ GAP: B50-21,06,21,21,—(P3)

### VSCode瓶颈（V1-V6）+ 代码质量（Q1-Q6）
| V1🟠 | V2🟡 | V3-V6🟢 | Q1🟠 | Q2🟡 | Q3🟡 | Q4🟡 | Q5🟢 | Q6🟢 |
→ GAP: B50-02,10,—(P3),38,40,—,41,—,—

> **总计：62个瓶颈 | 41个GAP（10 Step）| 21个P3（未来迭代）| 预计25-32天完成**
