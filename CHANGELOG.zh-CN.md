# 更新日志

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
