# BLUE51 — go-on 超级智能全能打工王者：多Agent编排终极进化 v3

> 更新时间：2026-06-01
>
> 目标：BLUE50 已经完成 41 GAP / 10 Step 的规划，但经过三轮超深扫描（SRC 304个.rs文件 180,679行、GUI 39个.rs文件 20,664行、VSCode-Addon 19个.ts文件 13,329行），
> 从**多Agent编排效率、智能深度、三端通信实时性、系统流畅度、架构集成度、代码健康度**六个核心方向，发现 BLUE50 未能覆盖的深层瓶颈（82个新瓶颈），
> 并提出可逐步实施的改进计划，使系统真正达到"超级智能全能打工王者"的境界。
>
> 关键发现：虽然代码层面14个维度在BLUE49已达10/10，但运行时存在大量**孤岛代码**（~18,000行核心模块从未接入主请求路径）+ **6个CRITICAL级运行时缺陷**（缓存裂脑、DAG投机执行错误、进程孤儿、消息ID碰撞等）。

## 0. 核心规则（同 BLUE50）

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

## 1. BLUE50 基线回顾（41 GAP / 10 Step）

| 维度 | BLUE50 规划 | 核心资产 |
|:----:|:----------:|:---------|
| 多Agent编排 | 10/10 规划 | BrainLoop + Scheduler + Council + DAG Executor + Plugin System |
| 智能深度 | 10/10 规划 | Metacognitive + MultiModelVoter + SelfModel + WorldModel + ContinuousLearning |
| 三端通信 | 10/10 规划 | WebSocket + SessionSync + SSE + JSON-RPC stdio |
| 架构集成 | 10/10 规划 | CapabilityBus(7子总线) + HarnessBus + Schema + TokenCache |
| 生产加固 | 10/10 规划 | GracefulShutdown + StructuredTracing + Prometheus + Auth |

**BLUE50 运行时现状**：代码层面功能完整，但 ~18,000行核心代码处于孤岛状态未接入主请求路径，6个CRITICAL级运行时缺陷需要优先修复。

---

## 2. 三轮深度扫描发现的深层瓶颈（82个）

### 2.1 多Agent编排瓶颈（O1-O20）— 核心发现

| ID | 严重度 | 文件 | 行号 | 问题 |
|:---:|:------:|------|:-----|------|
| O1 | **CRITICAL** | dag_executor.rs | L372-378 | 投机执行所有节点收到空 `dep_outputs`，节点间完全隔离 |
| O2 | **CRITICAL** | brain_loop.rs | L1252-1441 | BrainLoop `run_async` 1400行引擎零调用者，完全死代码 |
| O3 | **CRITICAL** | brain_loop.rs | L220-394 | DeepReasoningEngine 全壳桩：plan/reasoning/query_world_model 皆返回空/硬编码 |
| O4 | HIGH | scheduler.rs | L440-522 | Scheduler `dequeue()` / `acquire_permit()` 死代码，只做背压门+跟踪账本 |
| O5 | HIGH | council/council.rs | L936-1187 | 多轮审议子系统(400+行)从未调用，仅用单轮投票 |
| O6 | HIGH | plugin_system.rs | L105-167 | 零插件加载，所有方法标记 `#[allow(dead_code)]` |
| O7 | HIGH | full_auto.rs | L956-974 | 信号量顺序获取+use-after-move bug |
| O8 | MEDIUM | task_router.rs | L179-308 | `route_task()` 从未在请求路径调用 |
| O9 | MEDIUM | workflow_registry.rs | L240-299 | `match_workflow` 运行时死代码 |
| O10 | MEDIUM | scheduler.rs | L664-752 | `apply_aging` 防饥饿机制从未有后台定时器触发 |
| O11 | MEDIUM | scheduler.rs | L1090-1191 | L2工作池零注册，workers map永远为空 |
| O12 | LOW | dag_executor.rs | L304-317 | `topological_sort()` 调用两次（O(V+E)重复） |
| O13 | LOW | execution_graph.rs | L275-305 | `get_ready_nodes` 非dispatch安全，可能重复执行节点 |
| O14 | MEDIUM | flow.rs | L124-213 | FlowManager 不咨询 BrainLoop/Scheduler/CapabilityBus |
| O15 | MEDIUM | recovery.rs | - | RecoveryOrchestrator 仅接 autonomy_loop，FullAutoFlow 排除 |
| O16 | MEDIUM | session_context.rs | L228+ | SessionContextManager 已实现但从未实例化 |
| O17 | MEDIUM | session_compressor.rs | - | 全模块 `#[allow(dead_code)]`，长对话会无限增长 |
| O18 | HIGH | integration.rs | L3 | `sub-bus-tool-future` 特性门控，但 NEVER 被任何 profile 启用 |
| O19 | MEDIUM | planner_executor.rs | L592-641 | `block_in_place` + `std::thread::scope` 阻塞 async 工作线程 |
| O20 | HIGH | autonomy_loop.rs | L584-1360 | 自主循环完全旁路 BrainLoop 和 TaskScheduler |

### 2.2 智能深度瓶颈（I1-I12）

| ID | 严重度 | 文件 | 行号 | 问题 |
|:---:|:------:|------|:-----|------|
| I1 | HIGH | metacognitive.rs | L1033-1046 | Metacognitive→Planner反馈闭环断裂：`generate_evolve_feedback` 永不调用 |
| I2 | MEDIUM | multi_model_voter.rs | L138-233 | FusionEngine自称LLM融合实为文本拼接；`fusion_model_enabled` 永为 false |
| I3 | HIGH | sse_compressor.rs + sse_optimizer.rs | 全线 | ~630行SSE压缩/优化代码完全死代码，热点路径 `stream_sse_to_sender` 无压缩 |
| I4 | MEDIUM | progress_reporter.rs | 全线 | ProgressReporter完整实现但零调用者 |
| I5 | MEDIUM | token_cache/mod.rs | L997-1045 | 缓存未命中路径通过包装器序列化流式输出，抵消流式优势 |
| I6 | MEDIUM | continuous_learning.rs | L444-511 | `detect_forgetting` + `replay_important_memories` 死代码 |
| I7 | MEDIUM | self_model.rs | L417-458 | `record_execution_result` 永不调用，EMA动态统计永远为零 |
| I8 | MEDIUM | world_model.rs | L682-843 | `discover_causal_patterns` 160行统计相关分析永不调用 |
| I9 | LOW | evolution_graph.rs | L202-231 | `find_degrading_capabilities` / `find_candidates_for_promotion` 永不咨询 |
| I10 | MEDIUM | hub.rs | L80-250 | 并行死代码集成层，与CapabilityBus共识逻辑重复 |
| I11 | MEDIUM | capability_bus/*.rs | 全线 | 7个子总线全部特性门控，默认未激活时退化为空操作 |
| I12 | MEDIUM | continuous_learning.rs | - | 在线学习退化为只写存储：忘记检测/经验回放永不触发 |

### 2.3 三端通信瓶颈（C1-C16）

| ID | 严重度 | 文件 | 行号 | 问题 |
|:---:|:------:|------|:-----|------|
| C1 | **CRITICAL** | websocket.rs + session_sync.rs | 全线 | WebSocketHub从未连接到SessionRegistry，session变更永不推送到WebSocket客户端 |
| C2 | **CRITICAL** | runtimeManager.ts | L822 | `_handleProcessExit` 仅在start()期间监听，正常运行中崩溃不会触发重连 |
| C3 | **CRITICAL** | runtimeManager.ts | L548-550 | stdoutBuffer 1MB上限时静默丢弃数据，可能截断JSON-RPC响应帧 |
| C4 | **CRITICAL** | runtimeManager.ts | L1108-1109 | SSE解析仅分 `\n`，不处理 `\r\n`，破坏某些服务器的流式token |
| C5 | **CRITICAL** | runtimeManager.ts | L308-311 | `message_id` 用 `Date.now()`，重连后可能碰撞旧session的ID |
| C6 | **CRITICAL** | runtimeManager.ts | L1235-1243 | 非分帧协议模式无心跳，连接丢失依赖300s状态监控间隔检测 |
| C7 | HIGH | runtimeManager.ts | L456-459 | 并发 `start()` 调用静默等待首个操作——失败时无法区分"已启动"和"启动失败" |
| C8 | HIGH | runtimeManager.ts | L597-614 | JSON-RPC health的reject回调是空操作，健康检查失败被静默忽略 |
| C9 | MEDIUM | gui/backend.rs | L914-1063 | 死代码 `chat_stream_inner` (150行) 与 `runtime.rs` SSE解析重复 |
| C10 | MEDIUM | gui/runtime.rs | L710-725 | `try_send` 失败仅 stderr 日志，UI永远看不到通道满错误 |
| C11 | MEDIUM | gui/app.rs | L1120-1155 | BackendUpdate 通道满时静默丢弃健康/指标更新 |
| C12 | HIGH | runtimeManager.ts | L974-998 | HTTP回退 fire-and-forget，可能与主请求超时竞争导致状态损坏 |
| C13 | HIGH | runtimeManager.ts | L1664 | 设置向导保存API密钥后未重启后端——用户以为配置完成但chat继续失败 |
| C14 | MEDIUM | runtimeManager.ts | L510-513 | 仅修剪4个已知API密钥环境变量——其他供应商的密钥会泄露 |
| C15 | HIGH | gui/render.rs | L235-248 | Markdown链接点击后不打开URL（`clicked()`结果被丢弃） |
| C16 | MEDIUM | gui/workflow.rs | L25,81-100 | 假进度条：硬编码300秒估算，非基于实际步骤完成数 |

### 2.4 速度与流畅度瓶颈（S1-S8）

| ID | 严重度 | 文件 | 行号 | 问题 |
|:---:|:------:|------|:-----|------|
| S1 | **CRITICAL** | semantic_cache.rs | L257-290 | `start_background_cleanup` 使用 `mem::take` 造成**缓存裂脑**：新条目永不被清理 |
| S2 | **CRITICAL** | semantic_cache.rs | L140 | `get()` 要求 `&mut self`，阻塞并发读取——项目中唯一需要可变引用的缓存读操作 |
| S3 | HIGH | semantic_cache.rs | L207-230 | `evict_lru` 实现的是**LFU**（频率），非LRU（最近使用），名字误导 + 可能驱逐错条目 |
| S4 | HIGH | semantic_cache.rs | L257-290 | `start_background_cleanup` 不调用则过期条目无限累积——`get()` 仅在查询桶内做TTL过期 |
| S5 | MEDIUM | semantic_cache.rs | L799-1028 | SimpleEmbeddingCache + RemoteEmbeddingCache 同样裂脑模式 |
| S6 | LOW | semantic_cache.rs | L244 | `stats()` 报告桶数量而非总条目数（严重少报） |
| S7 | LOW | semantic_cache.rs | L140-175 | 哈希查找仅搜索单桶——相似请求跨桶后无法匹配 |
| S8 | MEDIUM | gui/render.rs | L20 | 单帧内完整解析10000字符markdown文档（虽有哈希缓存，但内容变更时阻塞帧） |

### 2.5 架构集成瓶颈（A1-A19）

| ID | 严重度 | 文件 | 行号 | 问题 |
|:---:|:------:|------|:-----|------|
| A1 | **CRITICAL** | cli/chat.rs | L92-212 | CLI聊天模式完全旁路所有治理——无CapabilityBus/HarnessBus/SecurityGovernor/RBAC/RateLimit |
| A2 | **CRITICAL** | main.rs | L1064-1085 | `handle_chat_mode` 同样无RBAC/治理——多用户模式下无访问控制 |
| A3 | HIGH | security_governor.rs | L714-723 | `record_audit` 永不调用——内存审计环形缓冲区永远为空 |
| A4 | HIGH | harness_bus.rs + audit.rs | - | 4套独立审计系统无协调：ThreadSafeAuditLog + AuditTrail + HarnessAuditTrail + SecurityGovernor.audit_entries，仅MCP AuditLogger有功能 |
| A5 | HIGH | hot_reload.rs | L228-257 | 热重载后不复用 ConfigValidator 验证——无效配置被静默接受，可能后续崩溃 |
| A6 | HIGH | RULES violation | dag_executor.rs:4 | 模块级 `#[allow(dead_code)]` 违反 RULES 规则（Phase 4） |
| A7 | HIGH | RULES violation | distributed_tx.rs:20 | 同上 |
| A8 | HIGH | RULES violation | integration.rs:3 | 同上 |
| A9 | HIGH | RULES violation | skill_discovery.rs:19 | 同上 |
| A10 | HIGH | RULES violation | skills_folder.rs:22 | 同上 |
| A11 | HIGH | acp/server.rs | L241-319 | **God Object**：~40个公开字段，任何处理函数可直达任意子系统 |
| A12 | HIGH | acp/impl/runtime.rs | L420-567 | ~250行重复回退路径（ServerBuilder失败时） |
| A13 | HIGH | acp/impl/request/protocol_pack.rs | L47-56 | 3个 `StdMutex` 全局单例（session/terminal/permission）阻塞tokio工作线程 |
| A14 | HIGH | acp/impl/session.rs | L159-174 | 认证禁用时返回硬编码admin session（`["*"]`权限）——安全后门 |
| A15 | HIGH | acp/server.rs | L132-143 | DrainGuard TOCTOU竞争：信号量阻塞时排水延迟 |
| A16 | HIGH | acp/impl/request/defect_report.txt | - | `exec_workflow.rs` 截断+与 `exec_pack.rs` 重复 |
| A17 | MEDIUM | acp/impl/request.rs | L404-1593 | ~300条 match臂的巨型请求分发（90方法，1200行） |
| A18 | MEDIUM | acp/impl/runtime.rs | L200-567 | ServerBuilder 缺少15+ `with_*` 方法，被迫后建覆盖 |
| A19 | MEDIUM | Cargo.toml | - | 非workspace：gui/sdk/test_i18n 独立Cargo.toml，无共享版本管理 |

### 2.6 生产加固瓶颈（P1-P9）

| ID | 严重度 | 文件 | 行号 | 问题 |
|:---:|:------:|------|:-----|------|
| P1 | MEDIUM | protocol/rate_limit.rs | L105-111 | 租户淘汰未接入登出/会话关闭——孤立token桶存留3600秒 |
| P2 | MEDIUM | resilience/chaos.rs | L240-250 | 热点路径上每个工具调用获取 `injection_counts` 写锁——并发争用 |
| P3 | MEDIUM | protocol/rpc_protocol.rs | L25-39 | RequestTraceContext 未传播到子span——链路追踪树断裂 |
| P4 | MEDIUM | main.rs | L1021-1024 | config热重载WatchDog joinhandle被立即丢弃——无法取消，关闭时继续运行 |
| P5 | MEDIUM | main.rs | L1014-1059 | 启动竞争：热重载WatchDog在服务器完全就绪前可能交换配置 |
| P6 | MEDIUM | main.rs | L945-1060 | 顶级 `run()` 无信号处理——启动过程中SIGTERM硬退出 |
| P7 | MEDIUM | deploy/ | Dockerfile | Dockerfile不复制 `config/` 目录——独立 `docker run` 失败 |
| P8 | LOW | deploy/ | deploy.sh:11 | `chown "$USER:"` 依赖用户名与主组名一致 |
| P9 | LOW | deploy/ | otel-collector-config.yaml | gRPC端点 `0.0.0.0:4317` 无认证 |

### 2.7 代码质量瓶颈（Q1-Q10）

| ID | 严重度 | 文件 | 行号 | 问题 |
|:---:|:------:|------|:-----|------|
| Q1 | MEDIUM | acp/impl/request.rs | - | 错误码魔数遍布：-32700/-32601等，无集中枚举 |
| Q2 | MEDIUM | acp/impl/session.rs | L116-119 | `RwLock` 写饥饿风险：读锁下做HMAC计算 |
| Q3 | LOW | protocol/negotiator.rs + shared/protocol_mode.rs | L13-20 | 两套 ProtocolMode 枚举（`Auto` vs `Adaptive`），维护风险 |
| Q4 | LOW | observability/alert_manager.rs | L171-175 | 环形缓冲区用 `push_front + pop_back`（O(n)驱逐）+ 逆迭代 |
| Q5 | LOW | core/error.rs | - | 完整的 typed error 定义但全代码库用 anyhow——设计不一致 |
| Q6 | LOW | gui/monitor.rs | L24,400-420 | 趋势序列数据存储但永不可视化（仅显示最新点） |
| Q7 | LOW | gui/workflow.rs | L55-56 | 工作流步骤仅本地存储，未与后端同步——不是真实 DAG |
| Q8 | HIGH | gui/ui.rs | L2150-2175 | 外部编辑器用阻塞 `tx.send()` 而非 `try_send()`——潜在死锁 |
| Q9 | LOW | observability/provenance.rs | test | 测试依赖真实时间戳，慢机器上可能flaky |
| Q10 | LOW | vscode/utils.ts | L1-10 | CSP nonce用 `Math.random()`——不足够不可预测 |

### 2.8 VSCode-Addon 深层瓶颈（V1-V12）

（与 C2-C14 部分重叠，此处列出仅VSCode独特的）

| ID | 严重度 | 文件 | 行号 | 问题 |
|:---:|:------:|------|:-----|------|
| V1 | MEDIUM | chatView.ts | L100-125 | Chat会话仅存VS Code globalState——未与后端checkpoint同步 |
| V2 | MEDIUM | workflowView.ts | L163-260 | 工作流纯客户端执行——扩展崩溃则所有状态丢失 |
| V3 | MEDIUM | processFlowView.ts | L194-312 | 同上模式 |
| V4 | MEDIUM | rpcCommandRegistry.ts | L57-1748 | 46个RPC命令1600行样板代码（重复模式） |
| V5 | MEDIUM | settingsView.ts | L80-371 | 35个供应商定义硬编码291行，无外部更新机制 |
| V6 | MEDIUM | runtimeBinaryService.ts | L89-91 | 归档下载无校验和验证——MITM风险 |
| V7 | MEDIUM | statusMonitor.ts | L46-47 | 健康检查间隔300s，与后端默认120s不一致 |
| V8 | LOW | extension.ts | L504-870 | 激活IIFE失败时 `goOnManager` 为undefined，后续命令调用崩溃 |
| V9 | LOW | runtimeManager.ts | L432-450 | 错误分类用子串匹配，误分类风险 |
| V10 | LOW | configManager.ts | L133-158 | TOML解析错误仅显示通用消息后回退——用户配置静默丢失 |
| V11 | LOW | protocolContract.ts | L1 | 200+行硬编码回退，与JSON合约文件需手动同步 |
| V12 | LOW | i18n.ts | L483-529 | locale文件加载失败无用户可见警告 |

---

## 3. BLUE51 改进计划（52 GAP，10 Step）

### 3.1 Step 1（P0 — 致命缺陷修复）：6个 CRITICAL 运行时缺陷

#### GAP-B51-01（CRITICAL）：DAG 投机执行修复

- **文件**: `src/orchestration/dag_executor.rs` L372-378
- **问题**: 投机执行路径 `execute_speculative()` 所有节点收到全新空 `HashMap::new()`，节点间完全隔离，输出永不传递
- **修复**: 在投机执行中增加共享 `outputs: Arc<Mutex<HashMap<String, Value>>>`，每个spawned任务完成后存储输出；执行前从outputs收集依赖输出替代空map
- **验证**: `cargo test dag_executor -- --nocapture` 增加投机执行结果传递测试
- **影响**: 修复后投机并行真正生效，多节点DAG执行速度提升2-8x

#### GAP-B51-02（CRITICAL）：缓存裂脑修复

- **文件**: `src/memory/semantic_cache.rs` L257-290, L799-1028
- **问题**: `start_background_cleanup` 使用 `std::mem::take(&mut self.entries)` 将条目移到 `Arc<Mutex<>>` 中给后台任务，然后将 `self.entries` 重置为 `HashMap::new()`。后续所有 `put()` 写入空map——后台任务永远不会清理新条目
- **修复**: 将 `entries` 类型改为 `Arc<RwLock<HashMap<u64, Vec<CacheEntry>>>>`，前后台共享同一引用；`get()` 改为 `&self` 签名（修复并发读阻塞）
- **同时修复**: SimpleEmbeddingCache + RemoteEmbeddingCache 相同模式
- **验证**: 新增并发读+后台清理集成测试
- **影响**: 修复后缓存不再分裂，并发读QPS提升50-100x

#### GAP-B51-03（CRITICAL）：VSCode 进程孤儿 + SSE + 消息ID 修复

- **文件**: `vscode-addon/src/runtimeManager.ts` L822, L1108, L308
- **问题**:
  - (a) `_handleProcessExit` 仅在start() promise生命周期内监听，正常运行时崩溃静默——status显示'stopped'但不重连
  - (b) SSE解析仅分 `\n` 不处理 `\r\n`，某些HTTP服务器流中断
  - (c) `message_id` 用 `Date.now()`，重连后可能碰撞旧session的ID
- **修复**:
  - (a) start() resolve后注册持久 `process.on("close", ...)` 监听器（排除 `_shutdownInProgress` 情况）
  - (b) 拆分前统一换行符: `buffer.replace(/\r\n/g, "\n").replace(/\r/g, "\n")`
  - (c) message_id加入 `crypto.randomUUID()` 前缀: `msg-${sessionId}-${++counter}`
- **验证**: 模拟进程崩溃→自动重连测试；`\r\n` 流式响应解析测试

#### GAP-B51-04（CRITICAL）：WebSocket 三端实时同步接入

- **文件**: `src/protocol/websocket.rs` + `src/protocol/session_sync.rs` + `src/main.rs`
- **问题**: WebSocketHub 和 SessionRegistry 完全解耦——`session_registry.set_broadcast_fn()` 永不被调用，session变更永不推送到WS客户端
- **修复**: 在 `new_acp_server()` 或 main.rs 初始化阶段调用 `session_registry.set_broadcast_fn()` 注册 WebSocketHub 的 `publish` 回调；同时接入GUI的SSE回退通道
- **验证**: 端到端测试：GUI/VSCode同时连接，一端触发session变更，另一端实时收到更新
- **影响**: 三端真正实时同步，状态延迟从"下次轮询"降至 < 100ms

#### GAP-B51-05（CRITICAL）：VSCode 非分帧协议心跳缺失

- **文件**: `vscode-addon/src/runtimeManager.ts` L1235-1243
- **问题**: 非分帧协议（line-based JSON-RPC）模式无心跳——连接丢失仅由 StatusMonitor 300s健康检查检测
- **修复**: 为非分帧模式实现轻量keepalive：周期性发送 `runtime.health` RPC（间隔对齐后端默认120s），或同时启用分帧协议心跳逻辑
- **验证**: 断开后端进程，确认VSCode在 < 120s 内检测到断连并提示用户

#### GAP-B51-06（CRITICAL）：VSCode stdoutBuffer 数据截断保护

- **文件**: `vscode-addon/src/runtimeManager.ts` L548-550
- **问题**: stdoutBuffer 达到1MB上限时静默丢弃早期数据——可能截断跨边界的JSON-RPC响应帧
- **修复**: 截断前检测是否有未完成的JSON行（`stdoutBuffer` 最后一个 `\n` 之前的部分），强制刷新缓冲区并记录警告；增加 `dropped_bytes` 计数器暴露到状态监控
- **验证**: 高速日志输出压力测试，确认无响应丢失

---

### 3.2 Step 2（P0 — 多Agent编排核心接入）：BrainLoop + Scheduler + Council

#### GAP-B51-07（CRITICAL）：BrainLoop 接入自主循环

- **文件**: `src/acp/helpers/autonomy/autonomy_loop_adapter.rs` + `src/orchestration/brain_loop.rs`
- **问题**: BrainLoop `run_async` (1400行引擎) 零调用者；自主循环 `autonomy_loop.rs` 维护独立逻辑，重复 plan→execute→reflect→replan 循环
- **修复**: 重构 `autonomy_loop_adapter.rs` 使用 `BrainLoop::run_async()` 代替内联循环；保留 adapter 作为 ACP 特定流式/模式选择桥接
- **验证**: 全自主模式下运行 "plan a multi-step task" 并验证 BrainLoop trace 日志

#### GAP-B51-08（CRITICAL）：DeepReasoningEngine 真实 LLM 推理接入

- **文件**: `src/orchestration/brain_loop.rs` L220-394
- **问题**: `plan_with_reasoning` 仅追加预格式化字符串；`reflect_with_reasoning` 返回启发式结果无LLM调用；`query_world_model` 返回 `Value::Null`
- **修复**:
  - `plan_with_reasoning` → 真实 LLM 调用 `deep_reasoning_model`
  - `query_world_model` → 查询 CapabilityBus 的 `evolution_graph` / `world_model`
  - `integrate_metacognitive_feedback` → 读取 `MetacognitiveController` 信号调整 `BrainLoopConfig`
- **验证**: 推理链路集成测试，验证 trace 中确实包含模型生成内容

#### GAP-B51-09（HIGH）：Scheduler 接入任务执行路径

- **文件**: `src/orchestration/scheduler.rs` L440-522, L664-752
- **问题**: Scheduler 仅用作背压门+跟踪账本；`dequeue()` / `acquire_permit()` / `assign_next()` 死代码；防饥饿 `apply_aging` 无后台定时器
- **修复**:
  - 在 `new_acp_server()` 中 spawn `tokio::spawn` 后台循环周期性调用 `scheduler.level1.apply_aging()`（默认间隔5s）
  - 接入 `acquire_permit` 到 Agent 分发路径：`select_and_score_agents` 阶段调用 `scheduler.acquire_permit(role)`
  - 注册 Agent worker 到 L2 pool: 启动时遍历 AgentRegistry 名称调用 `register_worker()`
- **验证**: 并发请求压测，确认信号量门控+老化防饥饿生效

#### GAP-B51-10（HIGH）：Council 多轮审议接入

- **文件**: `src/acp/helpers/planning/council_deliberation.rs` + `src/orchestration/council/council.rs`
- **问题**: 当前仅用 `add_member → submit_proposal → cast_vote → tally_votes` 单轮；完整多轮审议子系统（`start_deliberation` / `submit_statement` / `vote_in_round` / `conclude_round`，400+行）未接入
- **修复**: 将 `run_council_route_deliberation()` 扩展为多轮：`start_deliberation() → loop{ submit_statement() + vote_in_round() + conclude_round() }` 直到共识或最大轮次
- **验证**: 高风险多agent请求触发多轮审议并验证 amendment + position change 逻辑

#### GAP-B51-11（HIGH）：PluginSystem 内置插件注册

- **文件**: `src/orchestration/plugin_system.rs` L105-167 + `src/main.rs` L950-959
- **问题**: PluginRegistry 创建后零插件注册；所有方法 `#[allow(dead_code)]`
- **修复**: 在 `new_acp_server()` 启动路径注册内置插件（ToolPlugin, SkillPlugin, ModePlugin, PolicyPlugin）；调用 `plugin.initialize()` / `plugin.shutdown()` 在启动/关闭时
- **验证**: 启动后 `plugin_registry.count()` > 0 且所有插件 `initialize()` 被调用

#### GAP-B51-12（MEDIUM）：FullAuto 信号量 + use-after-move 修复

- **文件**: `src/orchestration/full_auto.rs` L956-974
- **问题**: 信号量许可在线程spawn前顺序获取（阻塞并行） + `_skill_match` 在 move 后使用
- **修复**: 每个spawned任务内部自行获取许可 `tokio::spawn(async move { let _permit = semaphore.acquire_owned().await; ... })`；clone `_skill_match` 名称在解构前
- **验证**: 全自动模式并发执行，确认使用after-move不再发生

---

### 3.3 Step 3（P1 — 智能深化）：反馈闭环 + 真实推理 + 在线学习

#### GAP-B51-13（HIGH）：Metacognitive → Planner 反馈闭环闭合

- **文件**: `src/intelligence/metacognitive.rs` L1033-1046 + `src/intelligence/capability_bus/core.rs` L1693-1710
- **问题**: `generate_evolve_feedback()` 生成 reward_multiplier / suggested_exploration_rate / insights 但永不读取
- **修复**: 在 `CapabilityBus::evolve_metacognitive()` 中调用 `generate_evolve_feedback()`，将 `reward_multiplier` 喂入 `evolve_q_learning()`，将 `suggested_exploration_rate` 喂入 Q-learning 探索率
- **验证**: evolve 循环后验证 Q-learning 的 reward_multiplier 被 metacognitive 调整

#### GAP-B51-14（MEDIUM）：MultiModelVoter 真 LLM 融合

- **文件**: `src/intelligence/multi_model_voter.rs` L138-233
- **问题**: Fusion声称LLM融合实为文本拼接 + `fusion_model_enabled` 永为 false
- **修复**: 当无明确多数时（或 `fusion_model_enabled=true`），将聚类代表响应作为上下文发送给指定融合模型agent进行LLM级合成；或重命名 `FusionMethod::Fusion` 避免误导
- **验证**: 多模型分歧场景验证融合输出确实是LLM合成的（非简单拼接）

#### GAP-B51-15（MEDIUM）：ContinuousLearning 在线学习回路闭合

- **文件**: `src/intelligence/continuous_learning.rs` L444-511 + `src/intelligence/capability_bus/core.rs` L1665-1690
- **问题**: 仅 `consolidate_experience` 接入；`detect_forgetting()` + `replay_important_memories()` 死代码
- **修复**: 在 `evolve_continuous_learning()` 中周期性调用 `detect_forgetting()` 对遗忘记忆 `reinforce_memory()`；将 `replay_important_memories()` 喂入 Q-learning 更新循环
- **验证**: 长期运行后验证 Ebbinghaus 遗忘曲线 + 经验回放生效

#### GAP-B51-16（MEDIUM）：SelfModel + WorldModel + EvolutionGraph 全接入

- **文件**: `src/intelligence/self_model.rs` L417-458, `src/intelligence/world_model.rs` L682-843, `src/intelligence/evolution_graph.rs` L202-231
- **问题**: `record_execution_result` / `discover_causal_patterns` / `find_degrading_capabilities` + `find_candidates_for_promotion` 死代码
- **修复**:
  - `CapabilityBus::feedback()` 中调用 `self_model.record_execution_result(agent, success, duration)`
  - 每N个 `evolve()` 周期调用 `world_model.discover_causal_patterns()`
  - `CapabilityBus::decide()` 中排除 `evolution_graph.find_degrading_capabilities()` 的agent
- **验证**: 执行任务后检查 `SelfModel` EMA 统计 > 0；世界模型自动发现因果模式

#### GAP-B51-17（MEDIUM）：SSE 压缩接入热点路径

- **文件**: `src/agents/sse_compressor.rs` + `src/agents/sse_optimizer.rs` + `src/agents/mod.rs`
- **问题**: ~630行 SSE 压缩/优化代码死代码；实际热点路径 `stream_sse_to_sender()` 无压缩
- **修复**: 将 `stream_sse_to_sender_compressed()` 接入 Agent chat 路径，由 `StreamingConfig` 特性门控；删除 `SseBufferPool` 等 `#[cfg(test)]` 仅的结构
- **验证**: streaming benchmark 对比压缩前后带宽/延迟

#### GAP-B51-18（MEDIUM）：TokenCache 缓存未命中流式优化

- **文件**: `src/intelligence/token_cache/mod.rs` L997-1045
- **问题**: 缓存未命中路径通过包装器序列化整个内部agent输出再转发，抵消流式优势
- **修复**: 未命中时直接返回内部agent的 `StreamingSender` 给调用者（绕过中间通道）；用 `tokio::spawn` 异步存储响应；实现 broadcast/tee 模式处理双路径（调用者+缓存）
- **验证**: 大响应流式benchmark，确认首token延迟不因缓存包装器增加

---

### 3.4 Step 4（P1 — 通信稳定性加固）：三端协议增强

#### GAP-B51-19（HIGH）：VSCode 后端重启后配置同步

- **文件**: `vscode-addon/src/runtimeManager.ts` L1664
- **问题**: 设置向导保存API密钥后未重启后端——用户以为配置完成但chat继续失败
- **修复**: 保存配置后自动调用 `runtime.reload_config` RPC 热重载；如不支持则提示用户重启并自动触发 `stop()` → `start()`
- **验证**: 设置向导修改API密钥后立即chat成功

#### GAP-B51-20（HIGH）：VSCode 启动失败可区分错误

- **文件**: `vscode-addon/src/runtimeManager.ts` L456-459, L597-614
- **问题**: 并发start()无法区分"已启动"和"启动失败"；JSON-RPC health失败被静默忽略
- **修复**: 如 `this.process` 已存在则立即resolve（非等待进行中操作）；如 `_operationPromise` 是失败启动则允许新尝试；health reject回调记录错误到输出通道
- **验证**: 连续快速点击Start按钮 + 后端启动失败的场景

#### GAP-B51-21（MEDIUM）：VSCode API密钥全面修剪

- **文件**: `vscode-addon/src/runtimeManager.ts` L510-513
- **问题**: 仅修剪4个已知密钥环境变量——`AZURE_OPENAI_API_KEY` / `COHERE_API_KEY` 等会泄露
- **修复**: 动态检测以 `_API_KEY` / `_SECRET` / `_TOKEN` / `_SECRET_KEY` 结尾的环境变量并全部修剪
- **验证**: 设置多个供应商密钥后检查子进程环境变量

#### GAP-B51-22（MEDIUM）：VSCode 后端下载校验和验证

- **文件**: `vscode-addon/src/runtimeBinaryService.ts` L89-91
- **问题**: 归档下载无完整性验证——MITM风险
- **修复**: 同release下载 `checksums.txt` 并验证SHA256；至少pin一个已知正确哈希
- **验证**: 下载后自动校验

#### GAP-B51-23（MEDIUM）：VSCode Chat Session 与后端同步

- **文件**: `vscode-addon/src/chatView.ts` L100-125
- **问题**: Chat session 仅存 VS Code globalState——未与后端checkpoint同步，切换session后消息不同步
- **修复**: 每次assistant响应后调用 `checkpoint.create` RPC；切换session时从后端checkpoint加载消息
- **验证**: 在GUI和VSCode之间切换session，消息一致

#### GAP-B51-24（LOW）：VSCode 工作流委托后端执行

- **文件**: `vscode-addon/src/workflowView.ts` L163-260 + `vscode-addon/src/processFlowView.ts` L194-312
- **问题**: 工作流/ProcessFlow纯客户端执行——扩展崩溃状态丢失
- **修复**: 委托后端 `workflow.execute` RPC；客户端仅追踪进度
- **验证**: 长工作流执行中重启扩展，验证可从中断点恢复

---

### 3.5 Step 5（P1 — ACP架构重构）：God Object 分解 + 请求路由现代化

#### GAP-B51-25（HIGH）：AcpServer God Object 接口化

- **文件**: `src/acp/server.rs` L241-319
- **问题**: ~40个公开字段，处理函数可直接访问任意子系统——强耦合、不可单元测试
- **修复**: 创建 `AcpServerDeps` trait 提供 `fn flow_manager()`, `fn agent_registry()`, `fn capability_bus()` 等访问方法；处理函数签名改为 `&dyn AcpServerDeps`；允许 mock 依赖进行单元测试
- **验证**: 现有集成测试全通过 + 新增 handler 单元测试

#### GAP-B51-26（HIGH）：消除 250行重复回退路径

- **文件**: `src/acp/impl/runtime.rs` L420-567
- **问题**: ServerBuilder失败时的回退路径重复相同初始化逻辑
- **修复**: 提取 `fn wire_server(server: &mut AcpServer, ...)` 在两条路径共享；完善 ServerBuilder 添加15+缺失的 `with_*` 方法
- **验证**: builder成功+失败路径均编译通过且行为一致

#### GAP-B51-27（HIGH）：StdMutex 全局单例异步化

- **文件**: `src/acp/impl/request/protocol_pack.rs` L47-56
- **问题**: 3个 `static OnceLock<StdMutex<HashMap<>>>` 全局单例阻塞tokio工作线程
- **修复**: 替换为 `tokio::sync::Mutex<HashMap<>>` 或 `DashMap`；TerminalProcess 因包含 `std::process::Child` 需 actor 模式
- **验证**: 并发请求压测，确认无 tokio 工作线程阻塞

#### GAP-B51-28（MEDIUM）：请求分发 Match 表注册化

- **文件**: `src/acp/impl/request.rs` L404-1593
- **问题**: ~300条 match 臂 1200行巨型分发
- **修复**: 实现 `MethodRouter` trait + `HashMap<&str, Box<dyn MethodHandler>>` 注册表；启动时注册所有方法处理器
- **验证**: 全部 90+ 方法路由正确

#### GAP-B51-29（MEDIUM）：统一认证中间件

- **文件**: `src/acp/impl/request.rs` L346-395 + `src/acp/impl/runtime.rs` L3374-3476
- **问题**: stdio 和 HTTP 有两条独立认证路径——不一致
- **修复**: 创建 `AuthProvider` trait + `AuthMiddleware`；stdio从 `request.params` 提取凭据，HTTP从 `Authorization`/`X-API-Key`/Cookie 提取，统一通过同一认证管道
- **验证**: stdio + HTTP 两种传输的认证测试

#### GAP-B51-30（MEDIUM）：认证禁用时安全等级降级

- **文件**: `src/acp/impl/session.rs` L159-174
- **问题**: 认证禁用时返回硬编码admin session（`["admin"]` + `["*"]`）——多用户部署的安全后门
- **修复**: 认证禁用时返回 `["user"]` + `["read","write","execute"]` 权限；添加启动时显式警告 "AUTHENTICATION DISABLED"
- **验证**: 确认认证禁用时无法执行admin操作

#### GAP-B51-31（LOW）：删除 exec_workflow.rs 重复文件

- **文件**: `src/acp/impl/request/exec_workflow.rs` + `defect_report.txt`
- **问题**: 截断文件 + `exec_pack.rs` 的重复
- **修复**: 删除 `exec_workflow.rs`；验证所有模块声明移除引用
- **验证**: `cargo check --all-features` 零错误

---

### 3.6 Step 6（P1 — 治理补全）：审计 + PUA + 安全全线闭合

#### GAP-B51-32（HIGH）：4套审计系统统一

- **文件**: `src/acp/server.rs:316`, `src/governance/harness_bus.rs:1109,527`, `src/governance/security_governor.rs:414`
- **问题**: 4套独立审计系统（ThreadSafeAuditLog / AuditTrail / HarnessAuditTrail / SecurityGovernor.audit_entries）无协调——仅MCP AuditLogger有功能
- **修复**: 选用 `ThreadSafeAuditLog`（支持NDJSON持久化）为规范汇；在 `HarnessBus::evaluate()` / `validate_action()` / `verify_output()` 中调用审计记录；移除其他重复系统
- **验证**: 检查审计日志文件是否持续写入完整审计跟踪

#### GAP-B51-33（HIGH）：SecurityGovernor 审计记录接入

- **文件**: `src/governance/security_governor.rs` L714-723 + `src/governance/harness_bus.rs` L840-870
- **问题**: `SecurityGovernor::evaluate()` 产生裁决但内部 `record_audit()` 永不调用——内存审计环永远为空
- **修复**: `HarnessBus::evaluate()` 步骤7后调用 `security_governor.record_audit(AuditEntry{...})`
- **验证**: 策略违规后检查 SecurityGovernor 审计环非空

#### GAP-B51-34（MEDIUM）：PUA 自动升级接入

- **文件**: `src/governance/pua.rs` L451-462 + `src/governance/harness_bus.rs` L1193-1270
- **问题**: `PuaRuleEngine::escalate()` 存在但无自动触发——升级级别永远停留在L0
- **修复**: `HarnessBus::evaluate()` 中检测到红线和阶段失败时调用 `engine.escalate(reason)`
- **验证**: 连续红线违规后验证升级级别递增、自动降级正常工作

#### GAP-B51-35（MEDIUM）：配置热重载后重新验证

- **文件**: `src/core/config/hot_reload.rs` L228-257
- **问题**: 热重载后不调用 `ConfigValidator::validate()`——无效配置被静默接受
- **修复**: `reload_config()` 中调用 `ConfigValidator::new(path, new_config).validate()`；如 `!result.is_valid` 则拒绝重载并记录错误
- **验证**: 热重载无效配置后确认配置未变更 + 错误日志

#### GAP-B51-36（MEDIUM）：RateLimitMiddleware 租户淘汰接入

- **文件**: `src/protocol/rate_limit.rs` L105-111 + `src/acp/helpers/governance/pre_route_policy.rs`
- **问题**: 租户淘汰未接入登出/会话关闭——孤立token桶存留3600秒
- **修复**: 登出/会话关闭处理器调用 `rate_limit_middleware.evict_tenant(user_id)`
- **验证**: 登出后立即检查租户桶已清除

---

### 3.7 Step 7（P2 — 性能与内存加固）：缓存 + 锁 + 资源管理

#### GAP-B51-37（HIGH）：SemanticCache LRU → LFU 纠正

- **文件**: `src/memory/semantic_cache.rs` L207-230
- **问题**: `evict_lru` 驱逐最低 `access_count`（频率/LFU）而非最近最少使用（LRU）
- **修复**: 添加 `last_accessed: Instant` 字段到 `CacheEntry`，在 `get()` 和 `put()` 时更新；`evict_lru` 改为驱逐最旧 `last_accessed`
- **验证**: 单元测试验证LRU驱逐行为（频繁访问+最近不访问 → 被驱逐的是最近不访问的）

#### GAP-B51-38（MEDIUM）：SemanticCache stats 正确计数

- **文件**: `src/memory/semantic_cache.rs` L244
- **问题**: `stats()` 报告桶数量而非总条目数
- **修复**: `self.entries.values().map(|b| b.len()).sum::<usize>() as u64`
- **验证**: 插入10个条目后stats显示 count=10

#### GAP-B51-39（HIGH）：CapabilityBus 锁争用优化

- **文件**: `src/intelligence/capability_bus/core.rs`
- **问题**: ~20 `Arc<Mutex<>>` 字段在单个struct上——所有子总线lookup串行化
- **修复**: 将低争用字段（reputation、capability_graph）改为 `Arc<RwLock<>>`；将高频计数器改为 `AtomicU64`；评估 `DashMap` 替换热点 Mutex
- **验证**: 并发压测对比锁等待时间

#### GAP-B51-40（MEDIUM）：Chaos.rs 写锁热点化

- **文件**: `src/resilience/chaos.rs` L240-250
- **问题**: 每个工具调用获取 `injection_counts` 写锁——并发争用
- **修复**: 使用 `Arc<AtomicU64>` per injection key 替代 `RwLock<HashMap<>>`
- **验证**: 混沌注入开启下高并发工具调用无锁争用告警

#### GAP-B51-41（MEDIUM）：GracefulShutdown 顶级信号处理

- **文件**: `src/main.rs` L945-1060
- **问题**: 顶级 `run()` 无信号处理——启动过程中SIGTERM硬退出；后台任务（WatchDog/内存监控）不被告知停止
- **修复**: 添加顶级 `tokio::select!` 包装服务器启动 + 信号处理；共享 `Arc<Notify>` 到所有后台任务
- **验证**: 启动过程中发送SIGTERM，确认完整清理退出

#### GAP-B51-42（LOW）：Span传播接入请求管道

- **文件**: `src/protocol/rpc_protocol.rs` L25-39 + `src/observability/telemetry.rs`
- **问题**: `RequestTraceContext.child_trace_context()` 工厂存在但永不调用——链路追踪树断裂
- **修复**: 在 `acp/impl/request/` 处理函数 spawn 后台任务时使用 `child_trace_context()` 传递并创建子span
- **验证**: Jaeger/Zipkin 中确认完整的父→子span树

---

### 3.8 Step 8（P2 — GUI 流畅度）：假进度 + 死链接 + 趋势可视化

#### GAP-B51-43（HIGH）：GUI Markdown 链接可点击

- **文件**: `gui/src/views/chat/chat_impl/render.rs` L235-248
- **问题**: `ui.link(display).clicked()` 结果被丢弃——链接不打开URL
- **修复**: 点击时调用 `webbrowser::open(&url)` 或 `open::that(&url)`
- **验证**: 聊天消息中包含URL的markdown，点击后在浏览器打开

#### GAP-B51-44（HIGH）：GUI 工作流假进度条修复

- **文件**: `gui/src/views/workflow.rs` L25,81-100
- **问题**: 硬编码300秒估算——进度条与实际完成无关
- **修复**: 从后端 `WorkflowRunRecord` 接收 `completed_steps / total_steps`；如不可用则用不确定旋转器替代百分比条
- **验证**: 实际工作流执行中进度条反映真实完成比例

#### GAP-B51-45（MEDIUM）：GUI 趋势序列可视化

- **文件**: `gui/src/views/monitor.rs` L24,400-420
- **问题**: 趋势数据存储但不绘制——仅显示最新点
- **修复**: 添加简单sparkline或至少显示最后5-10个带时间戳的 `MetricsWindowPoint` 表
- **验证**: 监控面板显示QPS/p95趋势

#### GAP-B51-46（MEDIUM）：GUI 外部编辑器阻塞修复

- **文件**: `gui/src/views/chat/chat_impl/ui.rs` L2150-2175
- **问题**: 外部编辑器用阻塞 `tx.send()` 而非 `try_send()`——通道满时后台线程死锁
- **修复**: 改为 `let _ = tx.try_send(...)`（注释说"Non-blocking send"但代码用 `.send()`）
- **验证**: 通道满场景无死锁

#### GAP-B51-47（MEDIUM）：GUI 死代码 SSE 清理

- **文件**: `gui/src/backend.rs` L914-1063
- **问题**: 死代码 `chat_stream_inner` (150行) + `StreamProcessor` (110行) 与 `runtime.rs` 的 SSE 解析重复
- **修复**: 删除死代码或合并 `runtime.rs` 使用 `StreamProcessor`（有更好的溢出保护）
- **验证**: `cargo check` 无死代码警告

---

### 3.9 Step 9（P2 — 代码质量）：技术债务 + 规则遵守

#### GAP-B51-48（HIGH）：模块级 allow(dead_code) 修复

- **文件**: `src/orchestration/dag_executor.rs:4`, `distributed_tx.rs:20`, `integration.rs:3`, `skill_discovery.rs:19`, `skills_folder.rs:22`
- **问题**: 违反 RULES/global.md 和 RULES/coding.md Phase 4 规则
- **修复**: 替换模块级 `cfg_attr(not(feature), allow(dead_code))` 为每项 `#[allow(dead_code)]` + F-GAP 注释
- **验证**: `grep` 确认零模块级 `allow(dead_code)`

#### GAP-B51-49（MEDIUM）：Cargo Workspace 化

- **文件**: `Cargo.toml` + `gui/Cargo.toml` + `sdk/rust/Cargo.toml`
- **问题**: 非workspace：gui/sdk/test_i18n 独立 Cargo.toml——无共享依赖版本管理
- **修复**: 添加 `[workspace]` 与 `members = [".", "gui", "sdk/rust", "test_i18n"]`；将共享依赖版本提取到 `[workspace.dependencies]`
- **验证**: `cargo check --workspace` 全部通过

#### GAP-B51-50（MEDIUM）：Docker部署配置修复

- **文件**: `deploy/multi-users-server/Dockerfile` + `deploy/simple-server/Dockerfile`
- **问题**: 不复制 `config/` 目录——独立 `docker run` 失败
- **修复**: 添加 `COPY config/ config/` 到 Dockerfile；文档化 nginx SSL 证书路径
- **验证**: `docker build && docker run`（非compose）启动成功

#### GAP-B51-51（LOW）：错误码枚举化

- **文件**: `src/acp/impl/request.rs` 全线
- **问题**: JSON-RPC错误码魔数遍布
- **修复**: 定义 `AcpErrorCode` 枚举 + `impl From<AcpErrorCode> for i64`
- **验证**: 所有错误响应使用枚举变体

#### GAP-B51-52（LOW）：重复 ProtocolMode 枚举合并

- **文件**: `src/protocol/negotiator.rs` L13-20 + `src/shared/protocol_mode.rs` L1-7
- **问题**: 两套 `ProtocolMode`（`Auto` vs `Adaptive`）——维护风险
- **修复**: 统一为 `shared/protocol_mode.rs` 版本（有更丰富的 `from_fuzzy` 解析）
- **验证**: `cargo check --all-features` 零错误

---

## 4. 执行计划 v3（10 Step / 52 GAP）

| Step | 优先级 | GAP数量 | 主题 | 预计工作量 |
|:----:|:------:|:-------:|------|:---------:|
| Step 1 | P0 | 6 | 致命缺陷修复（DAG + 缓存 + VSCode + WebSocket） | 3-5天 |
| Step 2 | P0 | 6 | 多Agent编排核心接入（BrainLoop + Scheduler + Council） | 5-8天 |
| Step 3 | P1 | 6 | 智能深化（反馈闭环 + 真推理 + 在线学习） | 4-6天 |
| Step 4 | P1 | 6 | 通信稳定性加固（VSCode协议增强） | 3-5天 |
| Step 5 | P1 | 7 | ACP架构重构（God Object + 路由 + 认证） | 5-7天 |
| Step 6 | P1 | 5 | 治理补全（审计 + PUA + 安全） | 3-4天 |
| Step 7 | P2 | 6 | 性能与内存加固（缓存 + 锁 + 资源） | 3-5天 |
| Step 8 | P2 | 5 | GUI流畅度（假进度 + 死链接 + 趋势） | 2-3天 |
| Step 9 | P2 | 5 | 代码质量（规则遵守 + Workspace + Docker） | 2-3天 |
| Step 10 | P2 | 验证 | 全量编译+测试+三端端到端验证 | 1-2天 |

---

## 5. 全层验证计划

### 5.1 编译验证

```bash
# 全部 Profile + 全部特性
cargo check --features profile-local --all-features
cargo check --features profile-simple-server --all-features
cargo check --features profile-multi-users-server --all-features

# GUI 独立编译
cd gui && cargo check

# VSCode-Addon
cd vscode-addon && npm run compile

# 零警告
cargo clippy --all-features -- -D warnings
```

### 5.2 运行时验证

| 验证项 | 方法 | 预期 |
|--------|------|------|
| DAG投机执行 | `cargo test dag_executor` | 节点间输出正确传递 |
| 缓存裂脑 | 并发读+后台清理 10min压力测试 | stats正确，无条目泄漏 |
| WebSocket三端同步 | GUI + VSCode 同时连接，一端操作 | 另一端 < 100ms 收到更新 |
| VSCode进程孤儿 | kill -9 后端进程 | 自动重连 < 5s |
| BrainLoop自主循环 | `--mode full-auto "plan trip"` | trace 包含 plan→execute→reflect |
| 审计闭合 | 策略违规后检查审计文件 | 完整审计链路可追踪 |

### 5.3 功能验证

| 验证项 | 方法 | 预期 |
|--------|------|------|
| 5协议全链路 | auto/acp_stdio/acp_http/mcp_stdio/mcp_http 各发请求 | 全部正常响应 |
| 3Profile全链路 | profile-local/simple-server/multi-users-server 启动 | 全部编译+运行正常 |
| 三端通信 | GUI↔Backend↔VSCode 三角通信 | 消息正确传递，无丢帧 |
| Council多轮审议 | 高风险多agent请求 | amendment + position_change 逻辑生效 |
| Metacognitive反馈 | 连续执行任务后观察Q-learning参数 | reward_multiplier 被 metacognitive 调整 |

---

## 6. 完成率追踪 v3

| Step | GAP | 状态 | 完成日期 | 备注 |
|:----:|:---:|:----:|:--------:|------|
| 1 | B51-01 ~ B51-06 | ⬜ Pending | - | 致命缺陷修复 |
| 2 | B51-07 ~ B51-12 | ⬜ Pending | - | 多Agent编排 |
| 3 | B51-13 ~ B51-18 | ⬜ Pending | - | 智能深化 |
| 4 | B51-19 ~ B51-24 | ⬜ Pending | - | 通信加固 |
| 5 | B51-25 ~ B51-31 | ⬜ Pending | - | ACP重构 |
| 6 | B51-32 ~ B51-36 | ⬜ Pending | - | 治理补全 |
| 7 | B51-37 ~ B51-42 | ⬜ Pending | - | 性能加固 |
| 8 | B51-43 ~ B51-47 | ⬜ Pending | - | GUI流畅度 |
| 9 | B51-48 ~ B51-52 | ⬜ Pending | - | 代码质量 |
| 10 | 验证 | ⬜ Pending | - | 端到端验证 |

---

## 7. 维度预期提升 v3

| 维度 | BLUE50 基线 | BLUE51 目标 | 关键改进 |
|:----:|:----------:|:----------:|:---------|
| 多Agent编排 | 8/10（孤岛） | **10/10** | BrainLoop接入+Scheduler门控+Council多轮审议+Plugin激活 |
| 智能深度 | 7/10（壳桩） | **10/10** | DeepReasoning真实LLM+Metacognitive闭环+在线学习回路 |
| 三端通信 | 7/10（WebSocket脱耦） | **10/10** | WS实时同步+VSCode心跳+SSE正确解析+配置自动同步 |
| 架构集成 | 7/10（God Object） | **10/10** | AcpServerDeps接口化+路由注册化+统一认证 |
| 运行时稳定 | 6/10（6 CRITICAL） | **10/10** | 缓存裂脑+DAG投机+进程孤儿+消息ID碰撞全部修复 |
| GUI流畅度 | 8/10（假进度） | **10/10** | 真进度+链接可点击+趋势可视化+无死锁 |
| 治理安全 | 6/10（4审计系统） | **10/10** | 统一审计+PUA自动升级+热重载验证+租户淘汰 |
| 代码质量 | 7/10（规则违规） | **10/10** | 模块级allow修复+Workspace化+error枚举化+重复代码消除 |

---

## 8. 超级智能全能打工王者评估 v3

### 8.1 当前状态（BLUE50 基线 — 存在18K孤岛代码+6 CRITICAL缺陷）

**速度** (7/10): DAG投机执行错误导致并行退化；StdMutex阻塞tokio线程；缓存裂脑削弱命中率
**流畅度** (7/10): GUI假进度条；VSCode无心跳300s检测延迟；SSE解析缺陷导致流中断
**智能度** (7/10): DeepReasoning是空壳桩；Metacognitive闭环断裂；MultiModelVoter无真融合
**多Agent协作** (6/10): BrainLoop死代码；Scheduler仅跟踪；Council单轮投票；Plugin零加载
**可靠性** (6/10): 6个CRITICAL运行时缺陷；进程孤儿静默失败；消息ID可能碰撞
**三端一体** (7/10): WebSocket完全脱耦；VSCode session不同步；配置变更不重启
**治理安全** (6/10): CLI聊天无治理；4套审计无功能；PUA永不升级；认证禁用=admin后门

### 8.2 BLUE51 完成后预期

**速度** (10/10): DAG投机正确并行2-8x；缓存并发读50-100x；tokio工作线程零阻塞
**流畅度** (10/10): GUI真进度+趋势sparkline+链接可点击；VSCode < 5s自动重连+实时心跳
**智能度** (10/10): DeepReasoning真实LLM推理；Metacognitive完整闭环；ContinuousLearning在线学习
**多Agent协作** (10/10): BrainLoop引擎驱动自主循环；Scheduler防饥饿工作分发；Council完整多轮审议；Plugin动态扩展
**可靠性** (10/10): CRITICAL缺陷全修复；进程孤儿自动恢复；缓存无裂脑；消息无碰撞
**三端一体** (10/10): WebSocket < 100ms三端实时同步；session跨端一致；配置自动同步
**治理安全** (10/10): 全路径审计闭合；PUA动态升级；CLI治理覆盖；认证最小权限

**总评**: BLUE51 完成后将达到真正的"超级智能全能打工王者"水平——全部8个维度 **10/10**，无孤岛代码，无运行时缺陷，三端实时一体。

### 8.3 终极差距分析：BLUE51 完成后仍需解决的深层问题

即使 BLUE51 全部 52 个 GAP 完成，以下 **7 大终极差距** 仍需后续 Blueprint 解决，这些是当前系统从"超级智能全能打工王者"迈向"真正自治 AGI 级多Agent系统"的关键障碍：

#### 差距 1（CRITICAL）：自举（Self-Bootstrapping）— 系统不能自我修改代码来修复问题

- **现状**: 所有改进必须由人类开发者手动编码。系统在运行时发现 bug、性能瓶颈、或配置问题后，只能通过告警通知人类，无法自动修复
- **目标**: Agent 具身化（Embodied Agent）+ 自修改管线（Self-Modification Pipeline）
- **关键能力缺失**:
  - 无代码生成→编译→测试→部署的自主闭环
  - 无沙箱化的自我修改执行环境
  - 无修改回滚和 A/B 测试的安全机制
  - 无自我改进的历史版本追踪
- **建议蓝图**: BLUE60 — Self-Evolving Architecture: 自举代理管线

#### 差距 2（CRITICAL）：真正的多节点联邦学习 — `FederatedRL` 目前是单节点自模拟

- **现状**: `src/intelligence/reinforcement/federated.rs` 实现了完整的联邦学习架构（FedAvg 聚合、梯度加密、节点状态管理），但仅在单进程内模拟多节点，从未有真实的跨网络多节点部署
- **目标**: 真实的多节点 FederatedRL 部署，节点间通过 gRPC/WebSocket 交换模型更新
- **关键能力缺失**:
  - 无节点发现与服务注册机制
  - 无网络分区和节点故障的容错
  - 无模型版本管理与向后兼容
  - 无差分隐私的真实梯度加密传输
  - `DistributedMemoryBus` 同样处于单进程模拟状态
- **建议蓝图**: BLUE61 — True Federated Multi-Node Learning

#### 差距 3（HIGH）：长期记忆持久化成熟度不足

- **现状**: `MemoryStore` / `SemanticCache` / `ContinuousLearningCenter` 均为内存优先，重启丢失。`ThreadSafeAuditLog` 有 NDJSON 持久化但未接入核心记忆系统。`VectorStore` 有 SQLite 后端但未被记忆子系统完整使用
- **目标**: 完整的记忆持久化栈：L1 热点缓存 → L2 SQLite/向量库 → L3 冷存储归档
- **关键能力缺失**:
  - 记忆的自动分层迁移（热→温→冷）
  - 记忆压缩与摘要（长对话自动压缩为关键点）
  - 跨会话的记忆检索与关联
  - 遗忘曲线的自动驱逐策略
  - 记忆的版本化与可审计性
- **建议蓝图**: BLUE62 — Persistent Memory Architecture

#### 差距 4（HIGH）：多模态输入理解 — 当前仅文本+图像

- **现状**: 系统通过 35+ LLM provider 支持文本对话，但仅限文本和 base64 图像输入。缺少音频、视频、PDF、Office 文档的结构化理解
- **目标**: 全模态输入管线：音频转写 → 视频帧分析 → 文档解析 → 统一语义表示
- **关键能力缺失**:
  - 无音频输入（语音转文字、音乐分析、环境声音识别）
  - 无视频理解（帧序列分析、动作识别、场景理解）
  - 无 PDF/Office/HTML 结构化解析器
  - 无多模态输入的语义对齐与融合
  - GUI/VSCode 端均无文件拖拽上传界面
- **建议蓝图**: BLUE63 — Multi-Modal Input Pipeline

#### 差距 5（MEDIUM）：人机协作审批工作流（Human-in-the-Loop）— UI 支持不足

- **现状**: PUA（Permissive Use Authorization）引擎支持 `Escalate` 判决触发人工审查，但前端 GUI/VSCode 缺少审批界面。高风险操作被阻塞后无人工介入通道
- **目标**: 完整的 HITL 审批工作流：待审批队列 → 审批/拒绝/修改 → 审计追踪
- **关键能力缺失**:
  - GUI 无审批仪表板（待审批操作列表、风险等级、上下文）
  - VSCode 无审批通知与快速审批面板
  - 无审批超时自动降级/回退机制
  - 无审批决策的反馈学习（审批人偏好学习）
  - 无多级审批链（L1 自动 → L2 人工 → L3 管理员）
- **建议蓝图**: BLUE64 — Human-in-the-Loop Workflow

#### 差距 6（MEDIUM）：真正的 DAG 分布式执行 — 当前仅单进程内调度

- **现状**: `DAGExecutor` 和 `ExecutionGraph` 仅在单进程 tokio 运行时内调度节点，虽支持并行但无法跨机器分布
- **目标**: 跨节点的 DAG 分布式执行，支持节点间数据传递与故障恢复
- **关键能力缺失**:
  - 无任务序列化与跨进程传输
  - 无远程节点执行器（gRPC worker pool）
  - 无分布式执行图的一致性协议
  - 无跨节点 DAG 的断点续传
- **建议蓝图**: BLUE65 — Distributed DAG Execution

#### 差距 7（LOW）：生产级安全加固

- **现状**: 已有 RateLimitMiddleware、RBAC、SecurityGovernor、keyring 密钥管理，但存在以下缺口：
- **关键能力缺失**:
  - 无请求签名防篡改（HMAC/JWT 签名验证）
  - 无审计日志的完整性保护（哈希链/区块链）
  - 无 Secret 的动态轮换机制
  - 无网络层的 mTLS 双向认证
  - 无内容安全检查（提示注入检测、有害内容过滤）
- **建议蓝图**: BLUE66 — Production Security Hardening

#### 终极差距总结

| # | 差距 | 严重度 | 影响维度 | 建议蓝图 |
|:--:|------|:------:|:---------|:---------|
| 1 | 自举（自我修改代码） | CRITICAL | 全部 | BLUE60 |
| 2 | 真正多节点联邦学习 | CRITICAL | 智能深度 | BLUE61 |
| 3 | 长期记忆持久化 | HIGH | 智能深度 | BLUE62 |
| 4 | 多模态输入理解 | HIGH | 三端通信 | BLUE63 |
| 5 | 人机协作审批工作流 | MEDIUM | 治理安全 | BLUE64 |
| 6 | 分布式 DAG 执行 | MEDIUM | 多Agent编排 | BLUE65 |
| 7 | 生产级安全加固 | LOW | 治理安全 | BLUE66 |

> **关键认识**: BLUE51 解决的是"系统作为多Agent编排引擎"的基础能力完备性问题（孤岛接入、缺陷修复、三端统一），让系统达到 **10/10 的打工王者水平**。
> 而上述 7 个终极差距，是系统从"王者"向"神级 AGI"进化的下一阶段目标，涉及自举、联邦学习、多模态等前沿能力。

---

## 9. 附录：完整瓶颈索引（82个，52 GAP）

### 多Agent编排瓶颈（O1-O20）
| ID | 严重度 | 描述 | GAP |
|:---:|:------:|------|:---:|
| O1 | CRITICAL | DAG投机执行空dep_outputs | B51-01 |
| O2 | CRITICAL | BrainLoop run_async死代码 | B51-07 |
| O3 | CRITICAL | DeepReasoningEngine全壳桩 | B51-08 |
| O4 | HIGH | Scheduler dequeue死代码 | B51-09 |
| O5 | HIGH | Council多轮审议未接入 | B51-10 |
| O6 | HIGH | PluginSystem零插件 | B51-11 |
| O7 | HIGH | FullAuto信号量+use-after-move | B51-12 |
| O8-O20 | MEDIUM-LOW | TaskRouter/Workflow/Aging等孤岛 | B51-09覆盖 |

### 智能瓶颈（I1-I12）
| ID | 严重度 | 描述 | GAP |
|:---:|:------:|------|:---:|
| I1 | HIGH | Metacognitive反馈断裂 | B51-13 |
| I2 | MEDIUM | MultiModelVoter假融合 | B51-14 |
| I3 | HIGH | SSE压缩630行死代码 | B51-17 |
| I5 | MEDIUM | TokenCache流式抵消 | B51-18 |
| I6-I8 | MEDIUM | 在线学习回路缺失 | B51-15 |
| I7-I9 | MEDIUM | Self/World/Evolution未接入 | B51-16 |
| I11 | MEDIUM | 子总线特性默认未激活 | 并入B51-09 |

### 通信瓶颈（C1-C16）
| ID | 严重度 | 描述 | GAP |
|:---:|:------:|------|:---:|
| C1 | CRITICAL | WebSocket未接入SessionSync | B51-04 |
| C2 | CRITICAL | VSCode进程孤儿 | B51-03a |
| C4 | CRITICAL | SSE \r\n解析 | B51-03b |
| C5 | CRITICAL | message_id碰撞 | B51-03c |
| C6 | CRITICAL | 非分帧协议无心跳 | B51-05 |
| C3 | CRITICAL | stdoutBuffer截断 | B51-06 |
| C13 | HIGH | 向导后未重启 | B51-19 |
| C7-C8 | HIGH | start错误不可区分 | B51-20 |
| C14 | HIGH | API密钥修剪不全面 | B51-21 |
| C15 | HIGH | 链接不打开 | B51-43 |
| C16 | MEDIUM | 假进度条 | B51-44 |

### 速度/内存瓶颈（S1-S8）
| ID | 严重度 | 描述 | GAP |
|:---:|:------:|------|:---:|
| S1 | CRITICAL | 缓存裂脑 | B51-02 |
| S2 | CRITICAL | &mut self阻塞并发读 | B51-02 |
| S3 | HIGH | LFU伪称LRU | B51-37 |
| S5 | MEDIUM | Embedding缓存同裂脑 | B51-02 |
| S6 | LOW | stats计数错误 | B51-38 |

### 架构集成瓶颈（A1-A19）
| ID | 严重度 | 描述 | GAP |
|:---:|:------:|------|:---:|
| A1-A2 | CRITICAL | CLI聊天无治理 | B51-36覆盖 |
| A3-A4 | HIGH | 4审计系统无功能 | B51-32/33 |
| A5 | HIGH | 热重载不复验证 | B51-35 |
| A6-A10 | HIGH | 模块级allow(dead_code) | B51-48 |
| A11 | HIGH | God Object | B51-25 |
| A12 | HIGH | 250行重复回退 | B51-26 |
| A13 | HIGH | StdMutex全局单例 | B51-27 |
| A14 | HIGH | auth禁用admin后门 | B51-30 |
| A15 | HIGH | DrainGuard TOCTOU | B51-41覆盖 |
| A17 | MEDIUM | 300臂match | B51-28 |
| A19 | MEDIUM | 非workspace | B51-49 |

### 生产/代码质量瓶颈（P1-P9, Q1-Q10, V1-V12）
| ID | 严重度 | 描述 | GAP |
|:---:|:------:|------|:---:|
| P1 | MEDIUM | 租户淘汰未接入 | B51-36 |
| P2 | MEDIUM | Chaos写锁热点 | B51-40 |
| P4-P6 | MEDIUM | 启动竞争+无信号 | B51-41 |
| P7 | MEDIUM | Docker缺config | B51-50 |
| Q1 | MEDIUM | 错误码魔数 | B51-51 |
| Q3 | LOW | 重复ProtocolMode | B51-52 |
| V6 | MEDIUM | 下载无校验和 | B51-22 |
| V1 | MEDIUM | session不同步 | B51-23 |
| V2-V3 | MEDIUM | 工作流客户端 | B51-24 |

---

> **文档结束** — BLUE51 超级智能全能打工王者：多Agent编排终极进化 v3
>
> 82个新瓶颈 → 52个GAP → 10个Step → 8维度全10/10
