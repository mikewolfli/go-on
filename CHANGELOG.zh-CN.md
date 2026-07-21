# 更新日志

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
