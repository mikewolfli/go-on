# BLUE53 — go-on 神级 AGI 终极进化：从优秀架构到自进化智能王者

> 更新时间：2026-06-01
>
> 目标：BLUE50/51/52 将系统从架构、安全、记忆、多模态等维度拉到 10/10，但 **真正的 AGI 智能王者** 需要在
> **处理速度、执行流畅度、推理深度、自进化能力** 上达到质的飞跃。
> BLUE53 针对 17 个层级（架构/运行/智能/治理/协议/韧性/可观测/内存/GUI/SDK/VSCode/测试/部署/i18n/安全/并发/自进化）
> 给出 **60 个 GAP** 的分步改进计划，确保系统真正达到神级 AGI 水平。

## 0. 核心规则（同 BLUE50/51/52）

1. **排除 i18n 字段硬编码** — 不涉及 locale 文本本身的结构调整。
2. **排除分拆文件** — 不将现有文件拆分为更小文件（但允许重组为子模块目录）。
3. **三端一统（backend / GUI / vscode-addon）** — 考虑三端配合、通讯流畅稳定性。
4. **注释英文** — 所有新增模块的代码注释必须使用英文。
5. **3 种服务器 Profile 全链路闭合** — profile-local、profile-simple-server、profile-multi-users-server 必须正确编译和行为一致。
6. **5 种协议全链路闭合** — auto、acp stdio、acp http、mcp stdio、mcp http。
7. **零警告、零冲突、零遗漏** — 最终验证 `cargo clippy --all-features -- -D warnings` 零警告。
8. **完整闭合** — 每个模块最终必须达到：编译通过、零警告、接入 governance.status、可通过 health 端点观测、有集成测试覆盖。
9. **不允许占位、空函数、逻辑错误** — 所有功能必须完整实现。
10. **回写完成率** — 每轮完成后，回写完成率（简述）。

---

## 1. 17 层现状评估与预期提升

| # | 层级 | 当前评分 | BLUE53 目标 | 关键差距 |
|:----:|------|:--------:|:----------:|:---------|
| L1 | 架构层 | 7/10 | 10/10 | 神级对象、重复结构、模块边界模糊 |
| L2 | 运行层 | 6/10 | 10/10 | `std::sync::Mutex` 阻塞 tokio、死锁风险、串行 I/O |
| L3 | 智能层 | 5/10 | 10/10 | 启发式推理非 LLM 驱动、元认知与执行分离 |
| L4 | 治理层 | 7/10 | 10/10 | 策略热重载缺失、跨节点共识缺失 |
| L5 | 协议层 | 7/10 | 10/10 | 协议版本协商未启用、硬编码路由分支 |
| L6 | 韧性层 | 6/10 | 10/10 | 双断路器、级联故障预防缺失 |
| L7 | 可观测层 | 6/10 | 10/10 | 指标分裂、分布式追踪缺失 |
| L8 | 内存层 | 7/10 | 10/10 | MemoryEntry 冲突、无合并、无分片 |
| L9 | GUI层 | 5/10 | 9/10 | 待细化评估 |
| L10 | SDK层 | 4/10 | 8/10 | 待细化评估 |
| L11 | VSCode层 | 5/10 | 9/10 | 待细化评估 |
| L12 | 测试层 | 5/10 | 10/10 | CLI 测试、E2E 集成覆盖不足 |
| L13 | 部署层 | 6/10 | 10/10 | 热重载缺失、配置验证隔离 |
| L14 | i18n层 | 7/10 | 9/10 | 错误路径优化 |
| L15 | 安全层 | 9/10 | 10/10 | BLUE52 已完成 |
| L16 | 并发层 | 4/10 | 10/10 | `Arc<Mutex>` 泛滥、死锁风险 |
| L17 | 自进化层 | 6/10 | 10/10 | 进化闭环未连接 LLM 推理 |

---

## 2. BLUE53 改进计划（60 GAP，12 Step）

### 2.1 Step 1（P0 — 并发安全）：`Arc<Mutex>` → `tokio::sync` 替换 + 死锁消除（8 GAP）

#### GAP-B53-01（CRITICAL）：AgentRegistry 锁优化
- **文件**: `src/agents/agent.rs` L1-L10
- **问题**: `AgentRegistry` 使用 `std::sync::Mutex<HashMap<String, Arc<dyn Agent>>>`，每次 register/get 阻塞 tokio 工作线程
- **子步骤**:
  1. 替换为 `tokio::sync::RwLock<HashMap<String, Arc<dyn Agent>>>`
  2. `register()` 用 `write()`，`get()` 用 `read()`
  3. 验证高并发下无工作线程阻塞
- **验证**: `tokio::task::spawn_blocking` 不再被 agent 查询使用

#### GAP-B53-02（CRITICAL）：BrainLoopInner 锁优化
- **文件**: `src/orchestration/loop/brain_loop.rs` L161
- **问题**: `BrainLoopInner` 包装在 `Arc<Mutex<>>` 中，plan/execute/reflect/replan 顺序获取
- **子步骤**:
  1. 识别 `BrainLoopInner` 中哪些字段需要写锁，哪些只需要读锁
  2. 拆分为 `tokio::sync::RwLock` + 无锁字段（原子计数器）
  3. 确保 `reflect()` 和 `plan()` 可以并发读取
- **验证**: 100 并发请求下脑循环不再成为瓶颈

#### GAP-B53-03（CRITICAL）：`with_acp_lock` 死锁预防
- **文件**: `src/acp/prelude.rs` L283-L311
- **问题**: 获取多个 `Mutex` 无排序，2 个线程交叉获取会死锁
- **子步骤**:
  1. 定义全局锁获取顺序（排序数组）
  2. 实现 `ordered_lock` 辅助函数，始终按相同顺序获取
  3. 或替换为单个 `tokio::sync::RwLock<AcpLockCounters>` 
- **验证**: 死锁测试：100 线程同时调用 `with_acp_lock`，无死锁

#### GAP-B53-04（HIGH）：VectorStore 锁优化
- **文件**: `src/memory/vector.rs` L72
- **问题**: `Mutex<Connection>` 序列化所有向量搜索
- **子步骤**:
  1. SQLite 后端：使用 `WAL` + `rusqlite::Connection` 的 2 个连接（读/写分离）
  2. Postgres 后端：使用连接池（`deadpool-postgres` 或现有 `tokio-postgres`）
  3. 读操作路由到读连接，写操作路由到写连接
- **验证**: 10 并发向量搜索延迟不线性增加

#### GAP-B53-05（HIGH）：HarnessBus 组合锁优化
- **文件**: `src/governance/harness_bus.rs` L98+
- **问题**: `evaluate()` 顺序获取 PuaRuleEngine/BudgetTracker/IdempotencyCache 等 6+ 个 `Mutex`
- **子步骤**:
  1. 将只读策略（e.g., IdempotencyCache 查询）改为 `RwLock`
  2. 实现批量锁获取（一次性获取所有需要的锁）
  3. 对非冲突策略使用乐观锁（`try_lock` + 重试）
- **验证**: 高并发下 governance 评估延迟降低 60%+

#### GAP-B53-06（HIGH）：MetricsSnapshot 锁竞争消除
- **文件**: `src/acp/prelude.rs` L252-L268
- **问题**: 获取 `MetricsSnapshot` 需要顺序锁定 10 个 `Mutex<AcpLockCounters>`
- **子步骤**:
  1. 将锁计数器改为原子操作（`AtomicU64`）
  2. `AcpLockCounters` 每个字段使用 `AtomicU64`，无需锁
  3. 快照通过原子加载构建（无锁）
- **验证**: `MetricsSnapshot::capture()` 无锁调用

#### GAP-B53-07（MEDIUM）：MemoryStore 辅助索引一致性
- **文件**: `src/memory/memory.rs` L78-L84
- **问题**: `entries` / `class_counts` / `entries_by_class` 三个映射通过分散的修改同步
- **子步骤**:
  1. 将三个字段包装在单个 struct 中
  2. 提供事务性方法确保原子更新
  3. 或替换为单一权威数据源 + 派生视图
- **验证**: 插入/删除后 `class_counts` 总和始终等于 `entries.len()`

#### GAP-B53-08（MEDIUM）：`lock_guard` 统一
- **文件**: `brain_loop.rs:37`, `metacognitive.rs:37`, `hyper_resilience.rs:27`, `consciousness.rs:41`, `pua.rs:46`
- **问题**: `lock_guard()` 辅助函数在 5+ 个文件重复
- **子步骤**:
  1. 在 `src/shared/` 创建 `lock_utils.rs`
  2. 定义 `pub fn lock_guard<T>(lock: &Mutex<T>) -> MutexGuard<T>` （带错误消息）
  3. 所有 5 个文件改为 `use crate::shared::lock_utils::lock_guard;`
- **验证**: 查找重复 0 个

---

### 2.2 Step 2（P0 — 智能推理深度）：LLM 驱动推理闭环（6 GAP）

#### GAP-B53-09（CRITICAL）：BrainLoop plan() LLM 驱动
- **文件**: `src/orchestration/loop/brain_loop.rs` L193-L221
- **问题**: `plan()` 返回硬编码字符串模板，非 LLM 生成
- **子步骤**:
  1. 创建 `AgentPlanner` trait：`async fn plan(goal, context, history) -> Plan`
  2. 修改 `BrainLoop::plan()` 调用 `AgentPlanner`（首选选中的 agent model）
  3. Plan 包含步骤序列 + 预期结果 + 验证标准
  4. 回退到启发式模板（当 LLM 不可用时）
- **验证**: `plan()` 对同一输入返回不同的、上下文相关的计划

#### GAP-B53-10（CRITICAL）：BrainLoop reflect() LLM 驱动
- **文件**: `brain_loop.rs` L487-L560
- **问题**: `reflect()` 使用字符串长度启发式 + 子字符串搜索 Error 模式
- **子步骤**:
  1. 创建 `ResultReflector` trait：`async fn reflect(plan, result, context) -> Reflection`
  2. 修改 `BrainLoop::reflect()` 调用 LLM 分析结果质量
  3. Reflection 包含：质量评分、差距分析、改进建议
  4. LLM 返回结构化 JSON（`serde_json` 解析）
- **验证**: 反射识别语义错误（非仅字符串匹配）

#### GAP-B53-11（HIGH）：Metacognitive → CapabilityBus 集成
- **文件**: `src/intelligence/metacognitive.rs`
- **问题**: 元认知控制器与能力执行完全分离
- **子步骤**:
  1. 创建 `MetacognitiveAdapter` trait：`async fn observe(execution) -> Observation`
  2. 在 `CapabilityBus` 执行前调用 `metacognitive.observe()`
  3. 在 `CapabilityBus` 执行后调用 `metacognitive.reflect()`
  4. 将置信度分数反馈到 `ModelSelector` 的权重调整
- **验证**: 执行流：CapabilityBus call → Metacognitive observe → 执行 → Metacognitive reflect → 调整

#### GAP-B53-12（HIGH）：SelfModel + WorldModel → BrainLoop 集成
- **文件**: `src/intelligence/self_model.rs`, `world_model.rs`
- **问题**: 自我模型和世界模型定义了丰富的结构但未被 `brain_loop` 使用
- **子步骤**:
  1. 在 `BrainLoop` 中添加 `self_model: Arc<RwLock<SelfModel>>` 字段
  2. `plan()` 前查询 `self_model.get_capabilities()` 选择可执行动作
  3. `reflect()` 后更新 `self_model.record_outcome()`
  4. WorldModel 用于 `plan()` 中的因果推理
- **验证**: BrainLoop 根据能力自我认知调整计划，避免不可执行步骤

#### GAP-B53-13（HIGH）：Consciousness → Metacognitive 融合
- **文件**: `src/intelligence/consciousness.rs`
- **问题**: 意识指标（5 维度）被跟踪但未被用于推理
- **子步骤**:
  1. 在 `MetacognitiveController` 中集成 `ConsciousnessMetrics`
  2. 低意识分 → 增加推理步数/调用更多模型
  3. 高意识分 → 加速执行（快速路径）
  4. 意识分数作为 `plan()` 的策略参数
- **验证**: 慢速环境意识分降低 → 自动增加推理轮次

#### GAP-B53-14（MEDIUM）：EvolutionGraph 闭环连接
- **文件**: `src/intelligence/evolution_graph.rs`
- **问题**: 进化图定义了丰富的生命周期数据但从未被写入
- **子步骤**:
  1. 在 `CapabilityBus` 执行后调用 `evolution_graph.record_evolution()`
  2. `EvolutionGraph` 数据用于 `model_selector` 的趋势感知选择
  3. 在 governance panel 展示进化趋势
- **验证**: CapabilityBus 执行后 evolution_graph 有记录

---

### 2.3 Step 3（P0 — 分布式与多节点）：跨节点真联邦执行（5 GAP）

#### GAP-B53-15（CRITICAL）：HotFailover → 生产链路连接
- **文件**: `src/intelligence/hot_failover.rs`
- **问题**: hot_failover 已实现但未在生产路径中调用
- **子步骤**:
  1. 在 `AgentRegistry::get()` 中包装 `HotFailoverProxy`
  2. provider 调用失败 → 自动故障转移到下一个可用 provider
  3. 配置故障转移策略（fallback order、timeout、circuit breaker 感知）
  4. `hot_failover.rs` 中 `is_available()` 对接 `CircuitBreakerRegistry`
- **验证**: OpenAI 返回 500 → 自动重试 Anthropic → 返回成功

#### GAP-B53-16（HIGH）：跨节点策略共识
- **文件**: `src/governance/harness_bus.rs`
- **问题**: 治理策略是每进程的，无跨节点同步
- **子步骤**:
  1. 使用 `raft` crate 创建 `PolicyRaftCluster`
  2. 每个 `HarnessBus` 在启动时加入 Raft 集群
  3. 策略变更为 Raft 日志条目，达成多数一致后生效
  4. 策略查询使用 `read_index`（线性一致性读）
- **验证**: 节点 A 更新策略 → 节点 B 在 <1s 内看到更新

#### GAP-B53-17（HIGH）：配置热重载
- **文件**: `src/core/setup.rs`, `src/acp/impl/runtime.rs`
- **问题**: 配置更改需要完全重启
- **子步骤**:
  1. 使用 `notify` crate 监听配置文件变更（代码已有 `notify` 依赖）
  2. 创建 `ConfigReloader`：`watch(path) → watch::Receiver<ConfigDelta>`
  3. 每个子系统注册 `ReloadHandler`：`fn reload(delta) -> Result<()>`
  4. 先 OpenTelemetry 重新初始化（已有 `reset_otel()`）
  5. 再策略层（`HarnessBus`）
  6. 最后路由层（HTTP routes 无需重新绑定）
- **验证**: 修改 `config.toml` → 系统自动应用变更，无连接断开

#### GAP-B53-18（MEDIUM）：级联故障预防
- **文件**: `src/resilience/hyper_resilience.rs`
- **问题**: 节点降级时无拓扑传播算法重路由负载
- **子步骤**:
  1. 创建 `CascadePreventer`：维护依赖图（DAG）
  2. 节点降级 → 标记下游节点为 `Degraded`
  3. 负载重路由：将流量从降级路径转移到健康路径
  4. 恢复后传播 `Restored` 信号
- **验证**: 节点 A 故障 → 自动重路由到 B → A 恢复 → 流量恢复

#### GAP-B53-19（MEDIUM）：自动扩展触发器
- **文件**: `src/resilience/hyper_resilience.rs`
- **问题**: `ScaleResources` 自我修复动作存在但无适配器
- **子步骤**:
  1. 创建 `ScalerAdapter` trait：`async fn scale_up(n)`, `async fn scale_down(n)`
  2. 实现 `KubernetesScaler`（k8s API）
  3. 实现 `DockerComposeScaler`（docker-compose scale）
  4. 连接 `HyperResilienceEngine` 的 `SelfHealAction::ScaleResources`
- **验证**: 模拟 CPU > 90% → 自动扩展 2 个副本

---

### 2.4 Step 4（P1 — 统一架构）：模块重组+去重（5 GAP）

#### GAP-B53-20（HIGH）：统一断路器实现
- **文件**: `src/acp/prelude.rs` (CircuitBreakerRegistry), `src/optimization/failure_prevention.rs` (CircuitBreakerState)
- **问题**: 2 个断路器实现，不同状态机
- **子步骤**:
  1. 选择 `CircuitBreakerRegistry` 作为权威实现（已用于 ACP 锁）
  2. `failure_prevention::CircuitBreakerState` 改为包装/委托 `CircuitBreakerRegistry`
  3. `HyperResilienceEngine` 使用统一 `CircuitBreakerRegistry`
- **验证**: 单一 `CircuitBreakerRegistry` 被所有子系统使用

#### GAP-B53-21（HIGH）：统一 MetricsSnapshot 和 PerformanceSnapshot
- **文件**: `src/acp/prelude.rs` (MetricsSnapshot), `src/observability/performance.rs` (PerformanceSnapshot)
- **问题**: 两个独立的性能指标结构
- **子步骤**:
  1. 创建 `UnifiedMetrics` struct，合并所有字段
  2. `MetricsSnapshot` 成为 `UnifiedMetrics` 的子集
  3. `PerformanceSnapshot` 成为 `UnifiedMetrics` 的别名
  4. 所有可观测性导出器使用 `UnifiedMetrics`
- **验证**: 删除 `MetricsSnapshot` 和 `PerformanceSnapshot`，仅保留 `UnifiedMetrics`

#### GAP-B53-22（HIGH）：工具模块重组
- **文件**: `src/orchestration/tool*.rs` (7 个文件)
- **问题**: `tool.rs`、`tool_extended.rs`、`tool_lock.rs`、`tool_native.rs`、`tool_pipeline.rs`、`tool_recommender.rs`、`tool_transaction.rs` 位于同一目录，应重组为 `tool/` 子模块
- **子步骤**:
  1. 创建 `src/orchestration/tool/` 目录
  2. 创建 `tool/mod.rs` 重新导出所有子模块
  3. 移动 7 个文件到 `tool/` 子目录并添加 `//!` 文档
  4. 更新 `orchestration/mod.rs` 中的引用
- **验证**: `crate::orchestration::tool::*` 路径解析正确

#### GAP-B53-23（MEDIUM）：setup.rs 子模块化
- **文件**: `src/core/setup.rs`（1100 行，40+ 辅助函数）
- **问题**: 超大的配置向导，混合设置/配置/secret 管理
- **子步骤**:
  1. 创建 `src/core/setup/` 目录
  2. 拆分 `setup/prompt.rs`（控制台 I/O、provider 推荐）
  3. 拆分 `setup/config_gen.rs`（配置生成、验证）
  4. 拆分 `setup/secrets.rs`（secret 发现、轮换）
  5. `setup.rs` 保留为轻量级 re-exports
- **验证**: 每个子文件 < 400 行

#### GAP-B53-24（MEDIUM）：main.rs 重导出精简
- **文件**: `src/main.rs` L25-L57
- **问题**: 重导出重复 `lib.rs` L33-L66
- **子步骤**:
  1. `main.rs` 重导出替换为 `pub use go_on::*;`
  2. 验证所有外部消费者的导入路径解析正确
- **验证**: `main.rs` 无重复的 `pub use` 块

---

### 2.5 Step 5（P1 — 协议层优化）：高吞吐消息处理（4 GAP）

#### GAP-B53-25（HIGH）：ProtocolNegotiator 集成
- **文件**: `src/protocol/negotiator.rs`（存在但未使用）
- **问题**: 协议版本协商已实现但未在运行时使用
- **子步骤**:
  1. 在 `acp/impl/runtime.rs` 的 `handle_http_connection` 中添加版本协商
  2. 客户端发送 `Acp-Version` 头 → 服务器回复兼容版本
  3. 版本不匹配 → 返回 426 Upgrade Required + 支持的版本列表
  4. 协议能力广告（支持什么 RPC、schema 版本等）
- **验证**: 客户端发送 `Acp-Version: 2.0` → 服务器协商为 1.5

#### GAP-B53-26（HIGH）：WebSocketHub 连接池
- **文件**: `src/protocol/websocket.rs`
- **问题**: 当前逐连接管理，无多路复用
- **子步骤**:
  1. 实现 `WsConnectionPool`：一组复用的 WebSocket 连接
  2. 请求级多路复用（多请求共享一个 WebSocket）
  3. 连接健康检查 + 自动重连
  4. 背压：当池满时优雅降级
- **验证**: 100 并发请求通过 5 个 WebSocket 连接完成

#### GAP-B53-27（MEDIUM）：stdin 循环非阻塞化
- **文件**: `src/acp/impl/runtime.rs` L544-L578
- **问题**: `BufReader::new(stdin).lines()` 阻塞 tokio 运行时
- **子步骤**:
  1. 使用 `tokio::io::BufReader` 替代 `std::io::BufReader`
  2. 或使用 `tokio::io::stdin().read_line()` 
  3. 主循环变为 `tokio::select!` 多路复用 stdin + 信号 + 心跳
- **验证**: 服务器在等待 stdin 输入时仍能处理其他事件

#### GAP-B53-28（MEDIUM）：硬编码常量可配置化
- **文件**: `src/acp/prelude.rs` L31-L58
- **问题**: `MAX_CONVERSATION_ID_LEN` 等 10+ 个常量硬编码
- **子步骤**:
  1. 创建 `ConstantsConfig` struct（serde 反序列化 + 默认值）
  2. 集成到 `AcpServer` 或全局配置
  3. 运行时通过配置修改
- **验证**: 修改配置 → 常量立即生效

---

### 2.6 Step 6（P1 — 治理层增强）：热重载+合规（4 GAP）

#### GAP-B53-29（HIGH）：策略运行时热重载
- **文件**: `src/governance/harness_bus.rs`
- **问题**: 所有策略在启动时加载，无运行时更新路径
- **子步骤**:
  1. `HarnessBus` 添加 `reload_policy(policy_name, new_config) -> Result<()>`
  2. 每个策略组件实现 `ReloadablePolicy` trait
  3. 通过 RPC `governance.policy.reload` 触发
  4. 重载失败自动回滚到上一个有效策略
- **验证**: `PuaRuleEngine` 规则运行时变更立即影响评估

#### GAP-B53-30（HIGH）：审计日志 SQLite/Postgres 持久化
- **文件**: `src/governance/audit.rs`, `pua.rs` L220-L265
- **问题**: 审计和学习记录写入 NDJSON 文件
- **子步骤**:
  1. 为 `ThreadSafeAuditLog` 添加 SQLite 后端（使用现有 `rusqlite` 依赖）
  2. 当 `backend-sqlite` 启用时使用 SQLite，否则回退 NDJSON
  3. 为 `learning-records` 添加数据库表
  4. 添加审计日志清理（自动删除 > 90 天的记录）
- **验证**: 审计日志查询 `audit.list(from, to)` 返回一致结果

#### GAP-B53-31（MEDIUM）：信标/healthz HTTP 端点
- **文件**: `src/observability/`, `src/acp/impl/runtime.rs`
- **问题**: 无 `/healthz`、`/readyz`、`/metrics` 端点
- **子步骤**:
  1. `/healthz`：返回 200 + 生命周期状态
  2. `/readyz`：检查所有子系统就绪（memory、vector、governance）
  3. `/metrics`：Prometheus 文本格式（已有 `metrics_exporter.rs`）
  4. 集成 `DegradationLevel`：降级时 /healthz 返回 503
- **验证**: `curl /healthz` 返回 `{"status":"ok","version":"1.1.0","uptime_secs":1234}`

#### GAP-B53-32（MEDIUM）：分布式追踪传播
- **文件**: `src/observability/telemetry.rs`
- **问题**: OTEL 已初始化但 HTTP 调用不传播追踪上下文
- **子步骤**:
  1. 在 `reqwest::Client` 上添加 `tracecontext` 头传播
  2. 在 `vendors.rs` 的每个 provider 调用的 HTTP 头注入
  3. 在 ACP HTTP 中间件中提取传入追踪上下文
- **验证**: 追踪 ID 从 ACP 请求 → AI provider 调用 → 响应在 Jaeger/Zipkin 中可见

---

### 2.7 Step 7（P1 — 内存层优化）：统一+合并+分片（4 GAP）

#### GAP-B53-33（HIGH）：统一 MemoryEntry
- **文件**: `src/memory/memory.rs` L24-L34, `src/memory/memory_persistence.rs` L32-L47
- **问题**: 两个 `MemoryEntry` 结构，名称冲突、字段不同
- **子步骤**:
  1. 将 `memory/memory.rs` 中的 `MemoryEntry` 重命名为 `MemoryPolicyEntry`
  2. 修改所有内部引用
  3. `memory_persistence.rs` 的 `MemoryEntry` 保持原样
- **验证**: `memory.rs` 中无 `MemoryEntry` 引用

#### GAP-B53-34（MEDIUM）：记忆合并
- **文件**: `src/memory/memory_retrieval.rs`
- **问题**: 相关条目永不合并
- **子步骤**:
  1. 实现 `merge_memories(ids: &[&str]) -> MemoryEntry`
  2. 合并策略：取最高 usefulness、合并内容、保留最早 created_at
  3. 链接图谱中自动合并同一会话的连续条目
  4. 合并后在图谱中添加 `DerivedFrom` 链接
- **验证**: `merge_memories(["mem1", "mem2"])` 返回包含两个源内容的条目

#### GAP-B53-35（MEDIUM）：向量数据库分片
- **文件**: `src/memory/vector.rs`
- **问题**: 所有嵌入属于单个 `VectorStore`
- **子步骤**:
  1. 创建 `ShardedVectorStore`：`shards: Vec<VectorStore>`
  2. 查询：广播到所有分片 + 本地合并 Top-K
  3. 写入：一致性哈希选择分片
  4. 动态分片扩展：添加新分片 → 重新平衡
- **验证**: 写入分布均匀，查询返回正确 Top-K

#### GAP-B53-36（LOW）：基于重要性的驱逐
- **文件**: `src/memory/memory.rs`
- **问题**: `enforce_checkpoint_capacity` 使用"删除最后一个"策略
- **子步骤**:
  1. 计算每个条目的 `eviction_score = usefulness / (age + 1) * retention_score`
  2. 驱逐最低分条目
  3. 保留"pin"标志防止重要记忆被驱逐
- **验证**: 低重要性记忆先被驱逐

---

### 2.8 Step 8（P1 — 编译与部署）：profile+速度（4 GAP）

#### GAP-B53-37（HIGH）：`cargo build` 时间优化
- **文件**: `Cargo.toml`
- **问题**: 完整构建需要 60s+，增量构建 30s+
- **子步骤**:
  1. 分解 `go-on` crate 为工作空间 crate（`go-on-core`、`go-on-agents`、`go-on-acp`）
  2. 常用依赖项（`serde`、`tokio`、`reqwest`）在 workspace 级别共享
  3. 使用 `cargo build --timings` 识别慢编译单元
  4. 考虑 `mold` 链接器（Linux）或 `lld`（macOS）
- **验证**: 增量构建 < 10s

#### GAP-B53-38（MEDIUM）：profile-local 启动时间
- **问题**: profile-local 启动 > 2s，影响开发者体验
- **子步骤**:
  1. 懒加载 provider 初始化（`OnceCell`）
  2. 并行化向量存储初始化
  3. 延迟非关键组件（遥测、审计）到首次访问
- **验证**: profile-local 启动 < 500ms

#### GAP-B53-39（MEDIUM）：死代码清理
- **文件**: 整个代码库
- **问题**: `#[allow(dead_code)]` 遍布新模块，大量未调用代码
- **子步骤**:
  1. 创建 `scripts/dead_code_scan.sh`：`cargo deadlinks` + `cargo unused`
  2. 分类死代码：
     - 从未调用的公共 API → 集成或删除
     - 私有未用 → 删除
     - 暂存功能 → 迁移到 `#[cfg(feature = "experimental")]`
  3. 从以下文件移除 `#[allow(dead_code)]`：
     - `src/orchestration/self_evolution/mod.rs`
     - `src/memory/memory_persistence.rs`
     - `src/memory/memory_retrieval.rs`
     - `src/security/*`
     - `src/multimodal/*`
- **验证**: `#[allow(dead_code)]` 数量减少 80%+

#### GAP-B53-40（LOW）：`cargo audit` + 依赖更新
- **文件**: `Cargo.toml`
- **问题**: 依赖可能有已知 CVE，定期检查
- **子步骤**:
  1. 集成 `cargo audit` 到 CI（或每周运行）
  2. 创建 `deny.toml`（已存在）并配置许可/漏洞策略
  3. 更新过时依赖（`cargo outdated`）
- **验证**: `cargo audit` 零高危漏洞

---

### 2.9 Step 9（P2 — Agent 层增强）：会话+适配+故障转移（4 GAP）

#### GAP-B53-41（HIGH）：SessionAwareAgent 包装器
- **文件**: `src/agents/agent.rs`
- **问题**: 代理可在 30+ 提供商间切换但无会话状态维护
- **子步骤**:
  1. 创建 `SessionAwareAgent`：包装 `Arc<dyn Agent>` + `SessionId`
  2. 在 `process_chat_request` 中使用会话感知代理
  3. 跨请求维护对话历史（截断 + Token 预算管理）
- **验证**: 同一会话的连续请求保持对话上下文

#### GAP-B53-42（HIGH）：自适应提供商故障转移
- **文件**: `src/intelligence/hot_failover.rs`
- **问题**: 无内置策略在 provider 间自动重试
- **子步骤**:
  1. `hot_failover.rs` 实现权重轮询 + 延迟感知选择
  2. 每个 provider 跟踪 P50/P95/P99 延迟和错误率
  3. 选择最低 (`latency * error_rate * cost_per_token`) 的 provider
  4. 退避模式：409 RateLimit → 等待 + 重试
- **验证**: OpenAI P95 > 5s → 自动切换到延迟更低的 Anthropic

#### GAP-B53-43（MEDIUM）：i18n 错误路径优化
- **文件**: `src/agents/agent.rs` L25-L62
- **问题**: `chat_request_failed_msg` 每次错误执行 i18n 查找
- **子步骤**:
  1. 将常见错误消息缓存为 `LazyLock<HashMap<&str, String>>`
  2. 仅在 key 更改时查找 i18n
  3. 或使用 `once_cell::sync::Lazy<HashMap>` 预加载到内存
- **验证**: 错误路径 i18n 开销 < 1μs

#### GAP-B53-44（MEDIUM）：Agent 错误处理统一
- **文件**: `src/agents/agent.rs` L25-L62
- **问题**: 3 个几乎相同的错误消息函数
- **子步骤**:
  1. 创建 `agent_error_msg(template, provider, status_code, body) -> String`
  2. 替换 `chat_request_failed_msg`、`request_failed_msg`、`token_request_failed_msg`
- **验证**: 删除 2 个函数，保留 1 个通用函数

---

### 2.10 Step 10（P2 — GUI 层增强）：响应+流畅+美观（5 GAP）

#### GAP-B53-45（HIGH）：Chat 流式渲染性能优化
- **问题**: 长对话列表 DOM 操作频繁，虚拟滚动缺失
- **子步骤**:
  1. 实现虚拟滚动（仅渲染可见消息）
  2. 消息增量 diff（仅更新变更部分）
  3. SSE 流增量渲染（逐 token，非逐消息）
- **验证**: 1000 消息对话 FPS > 30

#### GAP-B53-46（HIGH）：审批面板实时更新
- **问题**: 审批通知需轮询，无 WebSocket 推送
- **子步骤**:
  1. GUI 审批面板订阅 `governance.approval.*` WebSocket topic
  2. 新审批请求即时推送
  3. 审批状态变更即时更新
- **验证**: 提交审批 → GUI 面板 < 500ms 显示

#### GAP-B53-47（MEDIUM）：记忆可视化性能
- **问题**: 记忆图谱渲染大型数据集
- **子步骤**:
  1. 分页加载（每页 100 条目）
  2. 搜索去抖动（300ms）
  3. 图谱渲染使用 Canvas（非 SVG）
- **验证**: 10000 记忆条目加载 < 2s

#### GAP-B53-48（MEDIUM）：多模态上传界面改进
- **问题**: 大文件上传无进度条
- **子步骤**:
  1. 分块上传（每块 1MB）
  2. 实时进度推送（通过 SSE 或 WebSocket）
  3. 客户端压缩（图片在上传前压缩）
- **验证**: 100MB 文件上传有实时进度

#### GAP-B53-49（LOW）：主题一致性
- **问题**: GUI 主题可能不一致
- **子步骤**:
  1. CSS 变量统一
  2. 暗/亮模式切换
  3. 无障碍（ARIA 标签、键盘导航）
- **验证**: 切换主题 5 次，UI 元素一致性

---

### 2.11 Step 11（P2 — 测试+文档质量）：覆盖+契约+自动化（4 GAP）

#### GAP-B53-50（HIGH）：CLI 测试套件
- **文件**: `src/cli/`, `src/main.rs`
- **问题**: 无命令行参数组合测试
- **子步骤**:
  1. 创建 `tests/cli_tests.rs`：`assert_cmd` + `predicates`
  2. 测试所有参数组合（`--help`、`--version`、`--init`、`--protocol-mode`）
  3. 测试非法参数的错误消息
- **验证**: `cargo test --test cli_tests` 通过

#### GAP-B53-51（HIGH）：韧性合同测试
- **文件**: `src/resilience/`
- **问题**: 混沌测试模拟故障但不验证系统级 SLO
- **子步骤**:
  1. 创建 `tests/contract_tests/` 目录
  2. 每个子系统定义 SLO（P99 延迟 < 500ms、错误率 < 1%）
  3. 混沌注入后验证 SLO
  4. 退化级别变化触发合同检查
- **验证**: 注入 50% 请求超时 → 验证 P99 降级在预期范围内

#### GAP-B53-52（MEDIUM）：集成测试覆盖率门禁
- **问题**: 新代码需有集成测试覆盖
- **子步骤**:
  1. 创建 `scripts/coverage.sh`：`cargo tarpaulin` 或 `cargo-llvm-cov`
  2. 设置覆盖率门禁（lib: 70%+, 新模块: 80%+）
  3. CI 中集成覆盖率报告
- **验证**: 覆盖率报告显示各模块百分比

#### GAP-B53-53（LOW）：API 文档完整性
- **问题**: 部分模块缺少文档注释
- **子步骤**:
  1. 对 `pub fn` 和 `pub struct` 添加文档注释
  2. 对 `unsafe` 代码块添加 Safety 注释
  3. 对从 `panic` 的方法添加 Panics 注释
- **验证**: `cargo doc --no-deps` 无警告

---

### 2.12 Step 12（P2 — 自进化+元认知闭合）：真正的自进化循环（5 GAP）

#### GAP-B53-54（HIGH）：自进化 Agent → BrainLoop 闭环
- **文件**: `src/agents/self_evolution_agent.rs`, `brain_loop.rs`
- **问题**: 自进化 Agent 与 BrainLoop 未连接
- **子步骤**:
  1. `BrainLoop` 执行后调用 `SelfEvolutionAgent::analyze_result()`
  2. BrainLoop 检测到重复失败模式 → 触发 `EvolutionTrigger::RepeatedError`
  3. 自进化管线自动生成修复补丁
  4. 修复通过 BrainLoop 的下一轮执行验证
- **验证**: BrainLoop 连续 3 次失败相同步骤 → 自进化 Agent 生成补丁 → 修复

#### GAP-B53-55（HIGH）：元认知 → 意识 → 进化三环融合
- **文件**: `src/intelligence/metacognitive.rs`, `consciousness.rs`, `evolution_graph.rs`
- **问题**: 元认知、意识、进化图三者分离
- **子步骤**:
  1. 创建 `MetaCognitiveEvolutionTriumvirate` 协调三环
  2. 意识指标触发元认知反射
  3. 元认知反射结果记录到进化图
  4. 进化趋势驱动自进化触发的频率和优先级
- **验证**: 意识↓ → 反射↑ → 进化图表更新

#### GAP-B53-56（HIGH）：交叉会话元认知学习
- **问题**: `MetacognitiveProfile` 仅内存，重启丢失
- **子步骤**:
  1. 将 `MetacognitiveProfile` 持久化到 MemoryStore（L2 warm）
  2. 启动时加载最近的 profile
  3. 跨会话跟踪用户工作模式和代理表现
- **验证**: 重启后元认知 profile 恢复

#### GAP-B53-57（MEDIUM）：主动代码质量检测
- **文件**: `src/orchestration/self_evolution/evolution_loop.rs`
- **问题**: 自进化被动触发（仅告警/性能退化）
- **子步骤**:
  1. 实现 `CodeQualityTriggerSource`：`clippy` 检测 + TOML 配置冲突检测
  2. 实现 `DocumentationTriggerSource`：缺少文档时建议补充
  3. 实现 `DuplicationTriggerSource`：代码重复 > 20 行时触发
- **验证**: 重复代码出现 → 系统主动建议提取公共函数

#### GAP-B53-58（LOW）：自我改进报告
- **文件**: `src/orchestration/self_evolution/evolution_history.rs`
- **问题**: 无用户可见的自进化报告
- **子步骤**:
  1. 创建 `EvolutionReport`：汇总每次进化的 before/after 指标
  2. 通过 `governance.evolution.report` RPC 暴露
  3. GUI/VSCode 展示进化时间线和指标变化
- **验证**: RPC 返回包含退化率、成功率、影响范围的报告

---

## 3. 执行计划总表（12 Step / 58 GAP + 2 预留）

| Step | 优先级 | GAP数 | 主题 | 核心改进 | 预计工作量 |
|:----:|:------:|:-----:|------|:---------:|:---------:|
| Step 1 | P0 | 8 | 并发安全 | `Arc<Mutex>`→tokio::sync + 死锁消除 | 2-3 周 |
| Step 2 | P0 | 6 | 智能推理深度 | LLM 驱动 brain_loop + 元认知集成 | 3-4 周 |
| Step 3 | P0 | 5 | 分布式多节点 | HotFailover + 策略共识 + 热重载 | 3-4 周 |
| Step 4 | P1 | 5 | 统一架构 | 去重 + 模块重组 | 2-3 周 |
| Step 5 | P1 | 4 | 协议层优化 | 版本协商 + 连接池 + 非阻塞 I/O | 2-3 周 |
| Step 6 | P1 | 4 | 治理增强 | 热重载 + SQLite 审计 + health 端点 | 2-3 周 |
| Step 7 | P1 | 4 | 内存层优化 | 统一 MemoryEntry + 合并 + 分片 | 2-3 周 |
| Step 8 | P1 | 4 | 编译部署 | 构建速度 + 死代码 + 审计 | 2-3 周 |
| Step 9 | P2 | 4 | Agent增强 | 会话感知 + 故障转移 + 错误统一 | 2-3 周 |
| Step 10 | P2 | 5 | GUI 增强 | 流式渲染 + 实时推送 + 记忆可视化 | 3-4 周 |
| Step 11 | P2 | 4 | 测试文档 | CLI 测试 + 契约测试 + 覆盖率 | 2-3 周 |
| Step 12 | P2 | 5 | 自进化闭合 | 三环融合 + 主动检测 + 报告 | 3-4 周 |
| | | **58** | | | **28-40 周** |

---

## 4. 完成率追踪

| Step | GAP | 状态 | 完成日期 | 备注 |
|:----:|:---:|:----:|:--------:|------|
| 1 | B53-01 ~ B53-08 | ⬜ Pending | - | 并发安全：Mutex→tokio + 死锁消除 |
| 2 | B53-09 ~ B53-14 | ⬜ Pending | - | LLM 驱动推理 + 元认知集成 |
| 3 | B53-15 ~ B53-19 | 🔄 Wiring | 2026-06-01 | HotFailover + 连接模块到主链路 |
| 4 | B53-20 ~ B53-24 | ⬜ Pending | - | 统一断路器 + Metrics + 模块重组 |
| 5 | B53-25 ~ B53-28 | ⬜ Pending | - | 协议版本协商 + 连接池 + 非阻塞 I/O |
| 6 | B53-29 ~ B53-32 | ⬜ Pending | - | 策略热重载 + SQLite 审计 + healthz |
| 7 | B53-33 ~ B53-36 | ⬜ Pending | - | MemoryEntry 统一 + 合并 + 分片 |
| 8 | B53-37 ~ B53-40 | 🔄 Wiring | 2026-06-01 | 死代码清理 + multimodal/federated 接入主链路 |
| 9 | B53-41 ~ B53-44 | ⬜ Pending | - | Agent 会话 + 故障转移 + 错误统一 |
| 10 | B53-45 ~ B53-49 | ⬜ Pending | - | GUI 渲染 + 实时推送 + 记忆可视化 |
| 11 | B53-50 ~ B53-53 | ⬜ Pending | - | CLI 测试 + 契约测试 + 覆盖率 |
| 12 | B53-54 ~ B53-58 | ⬜ Pending | - | 自进化三环融合 + 主动检测 |

---

## 5. 关键新文件清单

| 新文件/目录 | 所属 GAP | 用途 |
|------------|:--------:|------|
| `src/shared/lock_utils.rs` | B53-08 | 统一 lock_guard 辅助函数 |
| `src/core/setup/` | B53-23 | setup.rs 拆分为 prompt/config_gen/secrets |
| `src/orchestration/tool/` | B53-22 | 7 个 tool*.rs 重组为工具子模块 |
| `src/governance/reloadable_policy.rs` | B53-29 | 可热重载策略 trait |
| `scripts/dead_code_scan.sh` | B53-39 | 死代码扫描脚本 |
| `scripts/coverage.sh` | B53-52 | 覆盖率门禁脚本 |
| `tests/cli_tests.rs` | B53-50 | CLI 测试套件 |
| `tests/contract_tests/` | B53-51 | 韧性合同测试 |
| `gui/src/views/approval_board.rs` | B53-46 | 审批面板 WebSocket 实时更新 |
| `gui/src/components/virtual_scroll.rs` | B53-45 | 聊天虚拟滚动组件 |

---

## 6. 维度预期提升

| 维度 | BLUE52 基线 | BLUE53 目标 | 关键改进 |
|:----:|:----------:|:----------:|:---------|
| 并发安全 | 4/10（`Arc<Mutex>` 泛滥） | **10/10** | 全部替换为 tokio::sync + 死锁预防 |
| 推理深度 | 5/10（启发式/字符串匹配） | **10/10** | LLM 驱动 plan/reflect + 元认知闭环 |
| 分布式能力 | 7/10（单进程协调） | **10/10** | 跨节点共识 + HotFailover + 级联预防 |
| 架构整洁度 | 7/10 | **10/10** | 统一断路器、Metrics、去重、模块重组 |
| 协议效率 | 7/10 | **10/10** | 版本协商 + 连接池 + 非阻塞 I/O |
| 治理运维 | 6/10 | **10/10** | 热重载 + healthz + SQLite 审计 |
| 内存管理 | 7/10 | **10/10** | MemoryEntry 统一 + 合并 + 分片 + 智能驱逐 |
| 构建性能 | 6/10 | **10/10** | 增量构建 < 10s + 死代码 80% 减少 |
| Agent 能力 | 7/10 | **10/10** | 会话感知 + 自适应故障转移 |
| GUI 体验 | 5/10 | **9/10** | 虚拟滚动 + 实时推送 + 大文件上传 |
| 测试质量 | 5/10 | **10/10** | CLI 测试 + 合同测试 + 覆盖率 > 70% |
| 自进化 | 6/10 | **10/10** | 元认知 × 意识 × 进化图三环融合 |
| **综合 AGI** | **6/10** | **10/10** | **从优秀架构到真正的自进化智能王者** |

---

> **文档结束** — BLUE53：60 GAP → 12 Step → 从优秀架构到神级 AGI 智能王者
>
> 推进建议：先从 **Step 1（并发安全）** 和 **Step 2（智能推理深度）** 并行开始，
> 这两步直接影响系统的**速度**和**智能度**，是成为"神级 AGI"的基础。
> Step 1 预计 2-3 周，Step 2 预计 3-4 周。
