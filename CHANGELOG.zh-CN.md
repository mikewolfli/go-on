# 更新日志

## [1.5.0] - 2026-08-05

### 24 轮深度+广度扫描与统一优化（2026-07-24 → 2026-08-05）

版本 1.5.0 汇总 24 轮超级深度+超级广度多智能体扫描成果（见 `docs/log/`），以 `docs/blueprints/principle.md` 为原则收敛：零死代码、零占位、零假修复、三端（backend / GUI / VS Code）统一架构。

#### 冗余消除与统一

- **Provider 目录三分拷贝 → 1 权威源 + 2 生成产物**：`src/core/providers.rs` 为唯一权威；GUI `generated_catalog.rs` 与 vscode `providerCatalog.generated.ts` 均由 `scripts/gen-provider-catalog.py` 生成（带 `--check` 双输出校验）。VS Code 目录补齐 kimi/siliconflow，env var 与分组全部派生自后端。
- **MCP 桥接 ↔ 原生处理器漂移闭合**：`mcp.resources.list` 不再返回空列表；`mcp.resources.subscribe` / `mcp.logging.setLevel` / `mcp.completion.complete` 假成功空响应改为与原生 `src/mcp/handlers.rs` 一致的真实实现/诚实错误。
- **PostgreSQL TLS 连接栈合并**（消除 ~200 行重复）：`parse_sslmode` / `PermissiveVerifier` / `connect_postgres` 统一收敛至 `src/memory/pg_pool.rs`。
- **重复时钟助手合并**：`agents::unix_now_secs` 改为委托 `shared::timestamps::now_ts`。
- **`keyring://` 常量统一**：`agents`、`acp::helpers::planning::context`、`config_validation`、`env_override` 共用一份。
- **废弃产物删除**：8.5 MB `scripts/go-on` 二进制、空文件 `debug_binding.py`、孤立 shell 脚本、TypeScript 死导出、Rust 死 API（`Agent::on_message`/`send_message`、`AgentMessenger::with_capacity`/`peek`、`new_safeguard`）。

#### PostgreSQL 生产加固

- 连接池（deadpool）+ 读写副本分离 + 版本化迁移 + `sslmode` TLS（require/verify-ca/verify-full）。

#### 功能补齐（闭合缺口）

- **F-GAP-66 附件多模态**：GUI 附件（文件选择器 + 粘贴/拖拽）真实进入后端多模态管线（图像提取、文档解析、音频转写、`repo:` 分析），不再只是文本摘要。
- **MCP `initialize` 能力声明统一**：仅声明双入口（原生+桥接）均有真实 handler 的能力；`sampling` 从共享声明中移除。
- **Copilot URL 权威值收敛** 至 `https://api.githubcopilot.com`（原为漂移的 localhost 拷贝）。
- **`build_role_routing` 读取已填充的全局角色注册表**（原构造恒空注册表，`available_custom_roles` 恒为 0）。

#### SDK 协议漂移修复

- `checkpoint.create` → `conversation.checkpoint.create`（需 `conversation_id`），覆盖 rust / nodejs / python SDK。
- nodejs `runtime.initialize`/`runtime.shutdown` → 规范名 `initialize`/`shutdown`。
- `breaker.reset` 参数契约与后端（`agent`/`name`）对齐。

#### 文档与版本

- 全平台版本统一为 **1.5.0**（workspace、GUI、VS Code 插件、rust/nodejs/python/typescript SDK、crates）。
- 恢复缺失的 `[1.2.0]` 英文条目（原滞留为陈旧的 `[Unreleased]`）。
- README 统计按实测修正（2018 测试、37 供应商、37 技能、~238K LOC、13 子总线架构）；CI 徽章 URL 修正。

### 验证

- 后端：`cargo check --all-targets` 通过；`cargo test` 全绿；`cargo clippy --all-targets -- -D warnings` 零警告。
- GUI：`cargo check` 通过。
- VS Code 插件：`tsc --noEmit` + mocha 全绿。
- Provider 生成器：`scripts/gen-provider-catalog.py --check` 双输出 OK（37 providers）。

## [1.4.3] - 2026-07-24

### BLUE71 — 三系统深度对比分析与高收益改进

本版本实现了 BLUE71 全部 9 个改进方案，补齐了与 Codex 和 Harness Gitness 深度对比发现的架构差距。总完成率：100%。

#### SessionActor — 树状会话架构（§2.1.1）

- **SessionLifecycle**：有限状态机 — Created → Ready → Active → Draining → Archived，通过 watch channel 传播。
- **SessionInput**：Actor 模型消息队列 (mpsc)，支持 UserMessage、Cancel、Steer 三种变体。
- **SessionHandle**：外部交互句柄，提供 send_message()、cancel()、steer()、生命周期订阅。
- **SessionState**：持有 CommunicationBus、ConversationHistory、CompactionManager、FragmentRegistry、AgentGraphStore。
- **session_main_loop**：持久 tokio 任务，处理 SessionInput、管理生命周期转换、触发自动压缩。
- **AgentThread 集成**：会话启动时创建 1 个 AgentThread，所有 UserMessage 通过 ChatRequest 复用。
- **优雅排空**：Cancel → 发送 Cancel 给 AgentThread → 生命周期：Draining → Archived。

#### AgentThread — 非阻塞 Agent 生成 + 持久循环（§4）

- **AgentThread**：非阻塞 Agent 执行句柄，含输入队列、状态 watch channel、JoinHandle。
- **spawn_agent_non_blocking()**：立即返回 AgentThread 句柄，Agent 作为独立 tokio 任务运行。
- **agent_main_loop**：真正的持久 Actor 循环 — 单条消息后不 break。连续处理 UserMessage、ChatRequest、Cancel。
- **ChatRequest 变体**：接受完整消息列表（含 system prompt）+ 选项 + oneshot 回复通道，为 SpawnAgentTool 集成铺路。
- **SpawnConfig**：可配置 max_depth、max_concurrency、token_ceiling、timeout_secs。

#### SpawnGuard — RAII 并发槽位保护（§5）

- **SpawnGuard**：原子计数器，支持 try_reserve/commit/release_slot/Drop。panic 时自动释放（无泄漏）。
- **提交模式**：所有权从调用者转移给生成的线程，线程完成时释放。
- **集成**：SpawnGuard 替换了 SpawnAgentTool 中的静态 Semaphore，也被 SessionActor 用于 AgentThread 预算。
- **当前使用量追踪**：`SpawnGuard::current_usage()` 用于可观测性。

#### 事件驱动状态传播 — 零轮询（§6）

- **AgentMessenger.notify**：每条消息投递时递增的 watch channel。
- **wait_for()**：使用 `notify_rx.changed().await` 替代 `tokio::time::sleep` 轮询。
- **AgentNode.lifecycle_tx**：生命周期状态的 watch channel 发送端 — 订阅者在每次状态转换时收到通知。

#### AgentLifecycle — 有限状态机（§7）

- **AgentLifecycle**：6 种状态 — Registered、Idle、Active（含 Planning/Executing/Reflecting/Waiting 阶段）、Completed、Errored、Cancelled。
- **AgentLifecycleBuilder**：便捷构造器，自动计时。
- **集成**：AgentTree 中的每个 AgentNode 都携带 `lifecycle_tx: watch::Sender<AgentLifecycle>`。
- **摘要方法**：人类可读的状态描述，用于日志和调试。

#### AgentGraphStore — 持久化抽象（§8）

- **AgentGraphStore trait**：upsert_edge / set_edge_status / list_descendants / remove_subtree。
- **InMemoryAgentGraphStore**：基于 HashMap 的默认实现 — 通过 Arc<RwLock> 线程安全。
- **SqliteAgentGraphStore**：SQLite 实现（feature: backend-sqlite）— rusqlite + spawn_blocking 模式。
- **Checkpoint 序列化**：ConversationHistory.to_checkpoint_json() / from_checkpoint_json() — 完整 JSON 往返。
- **集成**：SessionState 持有 `graph_store: Arc<dyn AgentGraphStore>`。Checkpoint 将序列化历史作为边存储。

#### ContextFragment — 结构化上下文注入（§9）

- **ContextFragment trait**：role() / priority() / body() / weight() 用于可注入的上下文片段。
- **FragmentRole**：System, Developer, User — 控制片段在提示中的位置。
- **FragmentPriority**：Low, Normal, High, Critical — Critical 总是包含，不受 token 预算限制。
- **FragmentRegistry**：register() + build_context(budget) + build_context_pairs(budget)，支持优先级排序和预算感知截断。
- **SimpleFragment**：基于静态字符串的片段内置实现。
- **集成**：SessionState.fragments 在 UserMessage handler 中填充 system prompt。

#### AdaptiveCompactor — 自适应对话压缩（§10）

- **ConversationTurn / ConversationHistory**：Token 感知的对话追踪，支持 drain、prepend、to_text 操作。
- **CompactionStrategy**：SlidingWindow（保留 N 轮）、Summarize（LLM 摘要）、Hybrid（摘要 + 保留最近）。
- **CompactionManager**：同步压缩引擎 — 无需 tokio runtime，可在任何上下文中使用。
- **AdaptiveCompactor**：自适应学习 — 基于对话长度和历史质量评分自动选择策略。
- **AdaptiveThreshold**：动态阈值 — 高质量时提高（少压缩），低质量时降低（更积极压缩）。
- **用户反馈融合**：quality * 0.6 + feedback * 0.4 混合评分。
- **30 个测试**：ConversationTurn、ConversationHistory、CompactionManager、AdaptiveThreshold、AdaptiveCompactor。

#### GuardianReviewer — 独立模型审查（§11）

- **GuardianReviewer**：使用独立 Agent 实例在执行前审查工具操作。
- **GuardianDecision**：Allow / Deny / EscalateToUser — 故障关闭（错误/超时/解析失败 → Deny）。
- **GuardianCircuitBreaker**：双阈值 — 最大连续拒绝（3）+ 最大近期拒绝（10/50）。
- **from_registry()**：从 AgentRegistry 查找审查 agent — 返回 None 用于优雅降级。
- **16 个测试**：熔断器、决策解析、允许/拒绝/无效/触发。

#### 跨模块重构与清理

- **agent_main_loop break 移除**：UserMessage 和 ChatRequest handler 现继续循环 — 持久 agent。
- **InterAgentComms 占位移除**：该变体只有空 handler（仅日志）— 按原则 §9 移除。
- **SessionActor 异步化**：spawn_session 从 sync（含 block_on）改为 async fn。
- **panic! 消除**：spawn_session 返回 Result<SessionHandle, String> 而非在路径解析时 panic。
- **根路径缓存**：AgentPath::parse("root") 只解析一次，缓存在 SessionState 中。
- **代码清理**：生产代码中零 #[allow(dead_code)] 或 #[expect(dead_code)]。零未使用导入。

#### 新增文件

| 文件 | 行数 | 描述 |
|------|------|------|
| `src/agents/session.rs` | ~700 | SessionActor 树状架构 |
| `src/agents/graph_store.rs` | ~280 | AgentGraphStore trait + 内存 + SQLite |
| `src/agents/fragment.rs` | ~300 | ContextFragment trait + FragmentRegistry |
| `src/governance/guardian.rs` | ~600 | GuardianReviewer + 熔断器 |
| `src/optimization/compaction.rs` | ~1000 | AdaptiveCompactor + 对话类型 |

### 验证

- **新测试**：所有新增模块约 70 个新测试。
- **所有 Profile**：local、simple-server、multi-users-server、full 全部编译通过。
- **蓝本符合性**：BLUE71 的 P0/P1/P2 改进 100% 实现。

## [1.4.1] - 2026-07-21

### 架构 — Transport Trait 第四阶段完成 + i18n 统一

本版本完成了 Transport Trait 迁移（第四阶段）并统一了三端（CLI、GUI、ACP）的错误消息。

#### Transport Trait 第四阶段（RPC_BUFFER 移除）

- **RPC_BUFFER task-local 已移除**：所有 JSON-RPC 输出现在通过 `CURRENT_TRANSPORT`（基于 RwLock 的全局传输层）路由，消除了双路径遗留机制（io.rs）。
- **RpcBufferTransport 已接线**：HTTP RPC handler（`/rpc`）和 TLS handler 现在使用 `set_current_transport(RpcBufferTransport)` 而非 `RPC_BUFFER.scope()`，响应捕获保持不变。
- **SseTransport 已接线**：`/chat/stream`、`/v1/chat/completions`、`/v1/responses` handler 在连接建立时设置 `SseTransport`。
- **CURRENT_TRANSPORT 升级**：OnceLock → RwLock，支持运行时在 stdio/HTTP-SSE/HTTP-RPC 模式间切换。
- **测试串行化**：Dispatch 测试使用 `DISPATCH_TEST_LOCK` 防止全局传输的并行竞争。
- **dead_code 标注清理**：移除 transport.rs 中所有过期的 Phase-1/2 `#[allow(dead_code)]`。

#### i18n 错误消息统一（跨模块）

- **12 个新 CLI i18n 密钥**：`cli.chat.git_diff_failed`、`cli.chat.summarization_failed`、`cli.chat.ai_review_failed`、`cli.chat.find_path_usage`、`cli.chat.tool_call_limit_mode`、`cli.chat.conversation_long_warning`、`cli.chat.tool_blocked_by_mode`、`cli.chat.tool_call_blocked_by_mode`、`cli.chat.tip_compact` — 已添加到 en-US.json、zh-CN.json、zh-TW.json。
- **6 个新 GUI i18n 密钥**：与后端错误模板匹配的 `chat.error.*` 提示密钥。
- **10 个硬编码 CLI eprintln! 消息** 已迁移至 i18n `t()`/`tf()` 调用。
- **GUI 后端空响应**：用透明的空内容传播替换了误导性的预设消息（GUI 的 `finalize_stream_result` 已提供有用的诊断）。

#### 对话循环可靠性

- **GUI generation 超时保护**：添加 `generation_deadline` — 330s 后强制重置，防止永久 UI 锁定。
- **GUI 事件溢出修复**：`process_pending` 从固定上限事件处理改为无限制 `while let Ok(...)` 排空，消除高吞吐 token 下的静默事件丢失。
- **GUI 空 generation_id 清理**：当 `generation_id=None` 时添加了孤立空 assistant 消息的回退清理。
- **GUI 阶段同步**：流请求体现在包含 `phase` 字段；`ChatCompleted` 响应携带 `actual_mode` 用于后端模式同步。
- **GUI SSE flush 优化**：`/v1/chat/completions` 流路径从每次事件 flush 改为每 4 事件批量 flush（与 `/chat/stream` 行为一致）。
- **GUI StreamProcessor 字段移除**：消除设置了但从未读取的死字段。
- **GUI split_thinking 死代码修复**：`extra_thinking` 现在在显示前正确与权威 thinking 合并。
- **CLI stdin 异步化**：用 `tokio::io::stdin().lines()` 替换 `spawn_blocking` 以获得响应式的 Ctrl+C 处理。
- **CLI Ctrl+C 可重复**：`signal::ctrl_c()` 现在每轮迭代重新 arm，支持多次中断。
- **CLI 模式持久化**：`/mode` 切换时模式保存到 `goon-cli-mode.json`，启动时恢复。支持 `GOON_DEFAULT_MODE` 环境变量。
- **CLI 失败消息清理**：失败时自动从历史记录中移除 assistant 消息。
- **CLI 输入回压**：`unbounded_channel` → 有界 channel(32) 防止粘贴风暴。
- **CLI 多行输入**：支持反斜线续行、空格续行、括号不平衡检测。

#### ACP/ZED Agent Server 集成

- **平台 profile 注入增强**：`initialize`、`session/new`、`tools/list` 响应现在包含 `platform_metadata`，内含可用模式、能力列表和默认模式。
- **session/prompt 思考正则增强**：现在同时支持 `<thinking>...</thinking>` 和 `__thinking__` 前缀格式。
- **session/close 清理**：Session 关闭和删除现在清理权限状态以阻止过期授权。
- **session/config per-session**：验证并记录 — `session_set_config_option` 已通过 `acp_session_state().entry()` 实现 per-session 存储。
- **MCP notifications/initialized**：现在返回 `id: Some(Value::Null)` 哨兵值（由 dispatch 层跳过发送），防止 Zed 客户端记录无关错误。

#### 并发与配置

- **AgentFactory 锁统一**：将 `instances` 和 `expirations` 合并为单个 Mutex 保护的 `AgentFactoryInner`，消除了容量检查与插入之间的 TOCTOU 竞争，移除了 `destroy_agent` 的双锁崩溃安全缺口。
- **Config 热重载**：完整 config 克隆从 2 次减为 1 次；通过释放写锁前捕获快照消除了过时读取竞争。
- **Config 解析器修复**：自动规则现在在 schema 迁移后应用，避免引用过期的阶段名称；写入磁盘前验证解析结果。
- **Config serde 安全**：`AppConfig` 中的 `flow` 字段现在有 `#[serde(default)]` — 缺失 `[flow]` 部分时使用默认值而非失败。

#### 代码质量

- **`is_clean()` cfg(test) 修复**：从 `#[cfg(test)] pub fn` 改为 `#[cfg(test)] pub(crate) fn` — 先前形式在非测试构建中被其他模块调用时会编译失败。
- **18 行注释的 criterion 基准代码** 从 `adaptive_selector.rs` 中删除。
- **`connect_direct_for_test` 重命名** 为 `connect_direct` — 该方法在生产环境和测试中均有使用。
- **所有 dead_code allow 已清理**：生产代码中零 `#[allow(dead_code)]` 或 `#[expect(dead_code)]`。
- **所有 profile 零警告**：`local`、`simple-server` 0 警告；`multi-users-server` 仅有 2 个预先存在的 `config_path` 警告。

### 验证

- **测试**：2069 通过，0 失败，0 忽略（完整套件）。
- **GUI 测试**：25 通过，0 失败。
- **MCP 测试**：20 通过，0 失败。
- **Agent Factory 测试**：12 通过，0 失败。
- **Config 核心测试**：49 通过，0 失败。
- **ACP 测试**：385 通过，0 失败。
- **Clippy**：`-D warnings` 零违规（后端 + GUI）。
- **Profiles**：`local`、`simple-server` 零警告。

## [1.3.0] - 2026-06-23

### 架构 — 锁竞争消除（第四阶段）

本版本完成整个运行时系统级的锁架构升级，通过精准的锁类型选择和基于 channel 的写操作卸载，消除了 12 个热路径互斥锁争用点。

#### Mutex → RwLock（读重型路径）

- **agent_router**（1 文件）：全局路由统计表从 `Mutex` 升级为 `RwLock`。并发的 agent 路由查询不再相互串行化。
- **agent_preference**（1 文件）：Agent 到阶段绑定状态从 `StdMutex` 升级为 `RwLock`。每个请求的阶段解析读取可并行执行。
- **semantic_cache**（4 文件）：语义响应缓存从 `StdMutex` 升级为 `RwLock`。近重复请求检测读取现可并发。
- **skill_registry**（17 文件）：全局技能注册表从 `Arc<StdMutex>` 升级为 `Arc<RwLock>`，覆盖编排层、MCP 处理器、能力总线、自治适配器等整个调用链。每次技能评分和检索读取现无锁争用。
- **maintenance_tracker**（3 文件）：100% 只读诊断快照 — RwLock 消除不必要的串行化。
- **inflight_limiter**（2 文件）：100% 只读诊断快照 — RwLock 消除不必要的串行化。
- **lifecycle_state**（3 文件）：80/20 读写比。服务器健康检查（读）不再互斥；唯一的 shutdown 写入不受影响。
- **review_timeout_policy**（1 文件）：死字段随架构一致性转换。

#### Mutex → mpsc 通道（写重型热路径）

- **online_controller**（6 文件，13 个调用点）：最重要的架构变更。请求热路径上的 9 个只写 outcome 记录调用（record_agent_outcome、record_phase_outcome）现通过 `mpsc::UnboundedSender` 分发 — 零锁争用。4 个需要返回值的读取调用（rank_agent_names_for_phase、recommend_phase、phase_policy_snapshot）保留同步锁访问。后台事件处理器异步排空通道并应用变更。

#### Clone 死代码移除

- **HyperResilienceEngine**（1 文件）：移除了顺次获取 5 个内部锁的 `Clone` 实现。生产代码从未调用此实现（所有实例通过 `Arc` 共享），使其既是死代码又是潜在死锁风险。

#### 语义精确性 — 有意保留的 StdMutex 字段

经过分析，`ResilienceContext` 中的三个字段保留为 `StdMutex`，因为 RwLock 不会带来有意义收益：
- **circuit_breakers**（62% 读，38% 写）：内部双重锁使外层 RwLock 无意义。
- **failure_prevention**（50/50 平衡）：RwLock 写路径与 Mutex 相同，无收益。
- **phase_rate_limiter**（60% 读，40% 写 per-request）：每个请求的令牌桶变更是写操作；RwLock 同样串行化。

### 死代码消除

- **run_health_check**：将空桩函数替换为真实的子系统验证（治理、运行时配置、agent 注册表）。
- **BrainLoopReport** 和 `with_diagnostic_feedback`：移除了 reflection 模块中废弃的结构和方法。
- **Pipeline 变体**：移除了 5 个死的 `PipelineStep` 和 `PipelineErrorStrategy` 变体（Parallel、Sequence、Conditional、Stop、Rollback）及所有相关分支函数和测试。
- **execute_with_two_phase_coordination**：移除了整个 2PC 协调器函数（预留 F-GAP-49，未使用）。
- **PluginRegistry::unregister**、**SkillDiscovery::invalidate_cache**、**session_context** 死方法：移除了单独标记的死代码。
- **DiagnosticFeedbackEngine** 死方法链：移除了 `has_errors`、`recommend_repair`、`latest_batch` 及 3 个相关测试。
- **sign_request**、**make_signature_for_test**、**subscriber_count**：移除了零调用者的纯测试辅助函数。
- **ApprovalPolicySuggester::new()**：移除了冗余构造函数（Default trait 提供相同功能）。
- **HyperResilienceEngine::clone()**：移除了死 Clone 实现（5 锁顺次获取）。
- **e2e 测试死导入**：移除了 `ImageAttachment`、`MtlsConfig`、`sign_request` 导入及相关测试代码。

### 构建与 Lint 清理

- **temp_env 依赖**：从可选的 feature 门控依赖移至 `[dev-dependencies]` — 解决了 `federated_transport.rs` 中的 3 个测试编译失败。
- **BrainLoopReport 可见性**：添加了 `pub use reflection::BrainLoopReport` — 解决了测试编译错误。
- **空 coordinator 模块**：移除了 `pub mod coordinator` 并删除了空文件 — 消除了 6 个死代码警告。
- **Clippy lint 修复**：解决了 7 个 lint（manual_pattern_char_comparison、len_zero、manual_is_multiple_of ×4、needless_borrow、for_kv_map、manual_range_contains、未使用导入）。

### 测试可靠性

- **video_processor 测试**：修复了不一致的 ffmpeg 检测 — 测试现在通过 match 统一处理 ffmpeg 可用和不可用情况，消除了假性 panic。
- **shell_exec 测试**：使其对环境更鲁棒 — 在没有 `sh` 访问权限的系统（macOS CI）上接受超时为有效结果，不再 panic。

### 性能

- **I18nManager::clone()**：从深度拷贝所有翻译（每次 clone O(n)）重新设计为 `Arc<I18nInner>` 共享（O(1)）。之前的实现在每次 clone 时拷贝整个 `HashMap<Language, HashMap<String, String>>`。

---

## [1.2.0] - 2026-06-10

[For previous versions, see English CHANGELOG.md]
