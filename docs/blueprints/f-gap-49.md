# F-GAP-49 通用预留 300 项特征对照表

> 更新时间：2026-06-03  
> 类型：通用预留（最大群体）—— 全部 `#[allow(dead_code)]` 标注  
> **总计：300 项（历史记录）** | ✅ 已激活：0 项（2026-08-09 起全部已删除，见下方状态同步） | 激活率：0%

> ## ⚠️ 2026-08-09 状态同步（BLUE72）
>
> 经全项目扫描（blue72.md §1）核实：此表所列 300 项预留特征**已在历轮清理中全部删除**，
> 当前代码中不存在这些 `#[allow(dead_code)]` 预留项。全项目实测 `#[allow(dead_code)]`
> 仅剩 4 处且全部合法（skill_market 的 serde 反序列化字段、self_evolution 的字符串模板）。
> 本表保留作为历史记录，不再作为待激活清单使用；后续以代码实际状态为准。

---

## 使用说明

- ✅ = 已激活（注释更新、且已在热路径中接入主链路闭合）
- ⏳ = 待激活（仍在 `#[allow(dead_code)]` 保护下；对应 feature 就绪后切换）
- 🔄 = 检查中（正在确认是否已意外激活或可删除）

---

## A. ACP Prelude 层（40 项）

| # | 特征 | 位置 | 状态 | 备注 |
|---|------|------|:----:|------|
| 1 | `DEFAULT_BREAKER_FAILURE_THRESHOLD` | `src/acp/prelude.rs` | ⏳ | 断路器默认阈值 |
| 2 | `DEFAULT_BREAKER_OPEN_SECONDS` | `src/acp/prelude.rs` | ⏳ | 断路器开启时间 |
| 3 | `MAX_CONVERSATION_ID_LEN` | `src/acp/prelude.rs` | ⏳ | 会话长度限制 |
| 4 | `MAX_BRANCH_ID_LEN` | `src/acp/prelude.rs` | ⏳ | 分支长度限制 |
| 5 | `MAX_CHECKPOINT_ID_LEN` | `src/acp/prelude.rs` | ⏳ | 检查点长度限制 |
| 6 | `MAX_CHECKPOINTS_PER_CONVERSATION` | `src/acp/prelude.rs` | ⏳ | 检查点数量限制 |
| 7 | `MAX_CHECKPOINT_MESSAGE_CHARS` | `src/acp/prelude.rs` | ⏳ | 检查点消息限制 |
| 8 | `MAX_CONVERSATIONS_TRACKED` | `src/acp/prelude.rs` | ⏳ | 会话追踪上限 |
| 9 | `MAX_STREAM_CHUNKS` | `src/acp/prelude.rs` | ⏳ | 流块上限 |
| 10 | `MAX_STREAM_CHARS` | `src/acp/prelude.rs` | ⏳ | 流字符上限 |
| 11 | `HISTOGRAM_BUCKETS_SECONDS` | `src/acp/prelude.rs` | ⏳ | 直方图桶定义 |
| 12 | `DEFAULT_CACHE_TTL` | `src/acp/prelude.rs` | ⏳ | 默认缓存TTL |
| 13 | `MAX_CACHE_ENTRIES` | `src/acp/prelude.rs` | ⏳ | 缓存条目上限 |
| 14 | `DEFAULT_VECTOR_TOP_K` | `src/acp/prelude.rs` | ⏳ | 默认向量检索数 |
| 15 | `DEFAULT_VECTOR_MIN_SCORE` | `src/acp/prelude.rs` | ⏳ | 默认向量最低分 |
| 16 | `REPUTATION_DECAY_FACTOR` | `src/acp/prelude.rs` | ⏳ | 信誉衰减因子 |
| 17 | `REPUTATION_INITIAL_SCORE` | `src/acp/prelude.rs` | ⏳ | 信誉初始分 |
| 18 | `MAX_VOTE_AGENTS` | `src/acp/prelude.rs` | ⏳ | 最大投票智能体数 |
| 19 | `MIN_VOTE_AGENTS` | `src/acp/prelude.rs` | ⏳ | 最小投票智能体数 |
| 20 | `VOTE_TIMEOUT_SECONDS` | `src/acp/prelude.rs` | ⏳ | 投票超时秒数 |
| 21 | `MAX_RETRY_ATTEMPTS` | `src/acp/prelude.rs` | ⏳ | 最大重试次数 |
| 22 | `RETRY_BASE_DELAY_MS` | `src/acp/prelude.rs` | ⏳ | 重试基础延迟 |
| 23 | `MAX_BACKOFF_DELAY_MS` | `src/acp/prelude.rs` | ⏳ | 最大退避延迟 |
| 24 | `DEFAULT_PLANNER_TIMEOUT` | `src/acp/prelude.rs` | ⏳ | 规划器超时 |
| 25 | `MAX_PLAN_STEPS` | `src/acp/prelude.rs` | ⏳ | 最大计划步骤 |
| 26 | `DEFAULT_BUDGET_QUOTA` | `src/acp/prelude.rs` | ⏳ | 默认预算配额 |
| 27 | `BUDGET_REFILL_RATE` | `src/acp/prelude.rs` | ⏳ | 预算补充率 |
| 28 | `BUDGET_REFILL_INTERVAL` | `src/acp/prelude.rs` | ⏳ | 预算补充间隔 |
| 29 | `MAX_TENANTS` | `src/acp/prelude.rs` | ⏳ | 最大租户数 |
| 30 | `DEFAULT_TENANT` | `src/acp/prelude.rs` | ⏳ | 默认租户 |
| 31 | `LOCK_ACQUIRE_TIMEOUT` | `src/acp/prelude.rs` | ⏳ | 锁获取超时 |
| 32 | `SESSION_IDLE_TIMEOUT` | `src/acp/prelude.rs` | ⏳ | 会话空闲超时 |
| 33 | `MAX_CONCURRENT_SESSIONS` | `src/acp/prelude.rs` | ⏳ | 最大并发会话 |
| 34 | `MetricsSnapshot.request_count` | `src/acp/prelude.rs` | ⏳ | 指标快照字段 |
| 35 | `MetricsSnapshot.error_count` | `src/acp/prelude.rs` | ⏳ | 指标快照字段 |
| 36 | `MetricsSnapshot.latency_p50` | `src/acp/prelude.rs` | ⏳ | 指标快照字段 |
| 37 | `MetricsSnapshot.latency_p95` | `src/acp/prelude.rs` | ⏳ | 指标快照字段 |
| 38 | `MetricsSnapshot.latency_p99` | `src/acp/prelude.rs` | ⏳ | 指标快照字段 |
| 39 | `MetricsSnapshot.cache_hits` | `src/acp/prelude.rs` | ⏳ | 指标快照字段 |
| 40 | `MetricsSnapshot.cache_misses` | `src/acp/prelude.rs` | ⏳ | 指标快照字段 |

## B. ACP Conversation 层（15 项）

| # | 特征 | 位置 | 状态 | 备注 |
|---|------|------|:----:|------|
| 41 | `build_conversation_tree()` | `src/acp/helpers/conversation.rs` | ⏳ | 构建会话树 |
| 42 | `prune_branch()` | `src/acp/helpers/conversation.rs` | ⏳ | 修剪分支 |
| 43 | `merge_branch()` | `src/acp/helpers/conversation.rs` | ⏳ | 合并分支 |
| 44 | `diff_conversations()` | `src/acp/helpers/conversation.rs` | ⏳ | 会话差异对比 |
| 45 | `compact_conversation()` | `src/acp/helpers/conversation.rs` | ⏳ | 会话压缩 |
| 46 | `split_conversation()` | `src/acp/helpers/conversation.rs` | ⏳ | 会话拆分 |
| 47 | `rebuild_checkpoint()` | `src/acp/helpers/conversation.rs` | ⏳ | 重建检查点 |
| 48 | `replay_from_checkpoint()` | `src/acp/helpers/conversation.rs` | ⏳ | 从检查点回放 |
| 49 | `checkpoint_summary()` | `src/acp/helpers/conversation.rs` | ⏳ | 检查点摘要 |
| 50 | `conversation_health()` | `src/acp/helpers/conversation.rs` | ⏳ | 会话健康检查 |
| 51 | `resolve_conflicts()` | `src/acp/helpers/conversation.rs` | ⏳ | 解决冲突 |
| 52 | `apply_merge_strategy()` | `src/acp/helpers/conversation.rs` | ⏳ | 应用合并策略 |
| 53 | `fetch_full_branch()` | `src/acp/helpers/conversation.rs` | ⏳ | 获取完整分支 |
| 54 | `load_checkpoint_state()` | `src/acp/helpers/conversation.rs` | ⏳ | 加载检查点状态 |
| 55 | `estimate_conversation_depth()` | `src/acp/helpers/conversation.rs` | ⏳ | 估计会话深度 |

## C. ACP Metrics 层（40 项）

| # | 特征 | 位置 | 状态 | 备注 |
|---|------|------|:----:|------|
| 56 | `MetricsExporter.format_prometheus()` | `src/acp/helpers/metrics.rs` | ⏳ | Prometheus 格式导出 |
| 57 | `MetricsExporter.format_json()` | `src/acp/helpers/metrics.rs` | ⏳ | JSON 格式导出 |
| 58 | `MetricsExporter.format_opentelemetry()` | `src/acp/helpers/metrics.rs` | ⏳ | OTLP 格式导出 |
| 59 | `MetricsExporter.export_interval()` | `src/acp/helpers/metrics.rs` | ⏳ | 导出间隔配置 |
| 60 | `request_duration_histogram` | `src/acp/helpers/metrics.rs` | ⏳ | 请求时长直方图 |
| 61 | `request_size_histogram` | `src/acp/helpers/metrics.rs` | ⏳ | 请求大小直方图 |
| 62 | `response_size_histogram` | `src/acp/helpers/metrics.rs` | ⏳ | 响应大小直方图 |
| 63 | `token_usage_histogram` | `src/acp/helpers/metrics.rs` | ⏳ | Token用量直方图 |
| 64 | `agent_latency_histogram` | `src/acp/helpers/metrics.rs` | ⏳ | 智能体延迟 |
| 65 | `phase_duration_histogram` | `src/acp/helpers/metrics.rs` | ⏳ | 阶段时长直方图 |
| 66 | `memory_usage_gauge` | `src/acp/helpers/metrics.rs` | ⏳ | 内存用量仪表 |
| 67 | `concurrent_requests_gauge` | `src/acp/helpers/metrics.rs` | ⏳ | 并发请求仪表 |
| 68 | `queue_depth_gauge` | `src/acp/helpers/metrics.rs` | ⏳ | 队列深度仪表 |
| 69 | `circuit_breaker_status` | `src/acp/helpers/metrics.rs` | ⏳ | 断路器状态 |
| 70 | `rate_limiter_status` | `src/acp/helpers/metrics.rs` | ⏳ | 限流器状态 |
| 71 | `cache_size_gauge` | `src/acp/helpers/metrics.rs` | ⏳ | 缓存大小仪表 |
| 72 | `cache_eviction_counter` | `src/acp/helpers/metrics.rs` | ⏳ | 缓存淘汰计数 |
| 73 | `db_connection_pool_size` | `src/acp/helpers/metrics.rs` | ⏳ | DB连接池大小 |
| 74 | `db_query_latency` | `src/acp/helpers/metrics.rs` | ⏳ | DB查询延迟 |
| 75 | `db_query_count` | `src/acp/helpers/metrics.rs` | ⏳ | DB查询计数 |
| 76 | `vector_search_latency` | `src/acp/helpers/metrics.rs` | ⏳ | 向量搜索延迟 |
| 77 | `vector_index_size` | `src/acp/helpers/metrics.rs` | ⏳ | 向量索引大小 |
| 78 | `embedding_latency` | `src/acp/helpers/metrics.rs` | ⏳ | 嵌入生成延迟 |
| 79 | `embedding_count` | `src/acp/helpers/metrics.rs` | ⏳ | 嵌入计数 |
| 80 | `agent_switch_counter` | `src/acp/helpers/metrics.rs` | ⏳ | 智能体切换计数 |
| 81 | `fallback_counter` | `src/acp/helpers/metrics.rs` | ⏳ | 回退计数 |
| 82 | `retry_counter` | `src/acp/helpers/metrics.rs` | ⏳ | 重试计数 |
| 83 | `timeout_counter` | `src/acp/helpers/metrics.rs` | ⏳ | 超时计数 |
| 84 | `quota_exceeded_counter` | `src/acp/helpers/metrics.rs` | ⏳ | 配额超限计数 |
| 85 | `policy_denial_counter` | `src/acp/helpers/metrics.rs` | ⏳ | 策略拒绝计数 |
| 86 | `approval_request_counter` | `src/acp/helpers/metrics.rs` | ⏳ | 审批请求计数 |
| 87 | `vote_counter` | `src/acp/helpers/metrics.rs` | ⏳ | 投票计数 |
| 88 | `consensus_reached_counter` | `src/acp/helpers/metrics.rs` | ⏳ | 共识达成计数 |
| 89 | `human_intervention_counter` | `src/acp/helpers/metrics.rs` | ⏳ | 人工干预计数 |
| 90 | `session_count` | `src/acp/helpers/metrics.rs` | ⏳ | 会话计数 |
| 91 | `checkpoint_count` | `src/acp/helpers/metrics.rs` | ⏳ | 检查点计数 |
| 92 | `fork_count` | `src/acp/helpers/metrics.rs` | ⏳ | 分支计数 |
| 93 | `merge_count` | `src/acp/helpers/metrics.rs` | ⏳ | 合并计数 |
| 94 | `conflict_count` | `src/acp/helpers/metrics.rs` | ⏳ | 冲突计数 |
| 95 | `runtime_gauge_cpu_usage` | `src/acp/helpers/metrics.rs` | ⏳ | CPU使用率 |

## D. ACP Misc 层（10 项）

| # | 特征 | 位置 | 状态 | 备注 |
|---|------|------|:----:|------|
| 96 | `resolve_redundant_tools()` | `src/acp/helpers/misc.rs` | ⏳ | 解析冗余工具 |
| 97 | `deduplicate_tools()` | `src/acp/helpers/misc.rs` | ⏳ | 工具去重 |
| 98 | `score_tool_relevance()` | `src/acp/helpers/misc.rs` | ⏳ | 工具相关性评分 |
| 99 | `rank_tools_by_relevance()` | `src/acp/helpers/misc.rs` | ⏳ | 工具相关性排序 |
| 100 | `filter_incompatible_tools()` | `src/acp/helpers/misc.rs` | ⏳ | 过滤不兼容工具 |
| 101 | `validate_tool_constraints()` | `src/acp/helpers/misc.rs` | ⏳ | 验证工具约束 |
| 102 | `tool_conflict_detection()` | `src/acp/helpers/misc.rs` | ⏳ | 工具冲突检测 |
| 103 | `resolve_tool_conflicts()` | `src/acp/helpers/misc.rs` | ⏳ | 解决工具冲突 |
| 104 | `infer_tool_dependencies()` | `src/acp/helpers/misc.rs` | ⏳ | 推断工具依赖 |
| 105 | `build_tool_execution_graph()` | `src/acp/helpers/misc.rs` | ⏳ | 工具执行图构建 |

## E. ACP Request 层（10 项）

| # | 特征 | 位置 | 状态 | 备注 |
|---|------|------|:----:|------|
| 106 | `AcpRequest.approval_context` | `src/acp/impl/request/` | ⏳ | 审批上下文字段 |
| 107 | `AcpRequest.audit_trail` | `src/acp/impl/request/` | ⏳ | 审计追踪字段 |
| 108 | `AcpRequest.risk_assessment` | `src/acp/impl/request/` | ⏳ | 风险评估字段 |
| 109 | `AcpRequest.budget_allocation` | `src/acp/impl/request/` | ⏳ | 预算分配字段 |
| 110 | `AcpRequest.provenance_chain` | `src/acp/impl/request/` | ⏳ | 溯源链字段 |
| 111 | `parallel_request_dispatcher` | `src/acp/impl/request/` | ⏳ | 并行请求分发器 |
| 112 | `request_prioritization` | `src/acp/impl/request/` | ⏳ | 请求优先级排序 |
| 113 | `request_batching` | `src/acp/impl/request/` | ⏳ | 请求批处理 |
| 114 | `request_deduplication` | `src/acp/impl/request/` | ⏳ | 请求去重 |
| 115 | `rate_limiting_per_user` | `src/acp/impl/request/` | ⏳ | 用户级限流 |

## F. ACP Runtime 层（8 项）

| # | 特征 | 位置 | 状态 | 备注 |
|---|------|------|:----:|------|
| 116 | `RuntimeGaugeSnapshot.cpu_percent` | `src/acp/impl/runtime.rs` | ⏳ | CPU百分比 |
| 117 | `RuntimeGaugeSnapshot.memory_percent` | `src/acp/impl/runtime.rs` | ⏳ | 内存百分比 |
| 118 | `RuntimeGaugeSnapshot.disk_io` | `src/acp/impl/runtime.rs` | ⏳ | 磁盘IO |
| 119 | `RuntimeGaugeSnapshot.network_io` | `src/acp/impl/runtime.rs` | ⏳ | 网络IO |
| 120 | `RuntimeGaugeSnapshot.goroutines` | `src/acp/impl/runtime.rs` | ⏳ | Goroutine数 |
| 121 | `RuntimeGaugeSnapshot.open_fds` | `src/acp/impl/runtime.rs` | ⏳ | 打开文件数 |
| 122 | `RuntimeGaugeSnapshot.heap_alloc` | `src/acp/impl/runtime.rs` | ⏳ | 堆分配 |
| 123 | `RuntimeGaugeSnapshot.gc_pause` | `src/acp/impl/runtime.rs` | ⏳ | GC暂停时间 |

## G. 治理层（3 项）

| # | 特征 | 位置 | 状态 | 备注 |
|---|------|------|:----:|------|
| 124 | `PolicyEngine.audit_logger` | `src/governance/` | ⏳ | 审计日志器接线 |
| 125 | `PolicyEngine.rollback_handler` | `src/governance/` | ⏳ | 回滚处理器接线 |
| 126 | `SecurityGovernor.auto_remediation` | `src/governance/` | ⏳ | 自动修复接线 |

## H. 可观测层（8 项）

| # | 特征 | 位置 | 状态 | 备注 |
|---|------|------|:----:|------|
| 127 | `AlertManager.slack_webhook` | `src/observability/` | ⏳ | Slack 通知 |
| 128 | `AlertManager.pagerduty_integration` | `src/observability/` | ⏳ | PagerDuty集成 |
| 129 | `AlertManager.email_alert` | `src/observability/` | ⏳ | 邮件告警 |
| 130 | `ObservabilityLayer.trace_sampling` | `src/observability/` | ⏳ | 追踪采样 |
| 131 | `ObservabilityLayer.log_aggregation` | `src/observability/` | ⏳ | 日志聚合 |
| 132 | `ObservabilityLayer.dashboard_export` | `src/observability/` | ⏳ | 仪表盘导出 |
| 133 | `ObservabilityLayer.health_check_gc` | `src/observability/` | ⏳ | GC健康检查 |
| 134 | `ProvenanceLedger.export_chain` | `src/observability/` | ⏳ | 溯源链导出 |

## I. 共享层（5 项）

| # | 特征 | 位置 | 状态 | 备注 |
|---|------|------|:----:|------|
| 135 | `HttpClient.retry_strategy` | `src/shared/http_client.rs` | ⏳ | 重试策略 |
| 136 | `HttpClient.circuit_breaker` | `src/shared/http_client.rs` | ⏳ | 断路器 |
| 137 | `SecretOverride.encryption_algo` | `src/shared/secret_override.rs` | ⏳ | 加密算法 |
| 138 | `ToolDescriptors.fuzzy_match` | `src/shared/tool_descriptors.rs` | ⏳ | 模糊匹配 |
| 139 | `ProtocolMode.auto_negotiate` | `src/shared/protocol_mode.rs` | ⏳ | 自动协商 |

## J. 协议层（5 项）

| # | 特征 | 位置 | 状态 | 备注 |
|---|------|------|:----:|------|
| 140 | `WebSocket.auto_ping_pong` | `src/protocol/websocket.rs` | ⏳ | 自动心跳 |
| 141 | `WebSocket.message_compression` | `src/protocol/websocket.rs` | ⏳ | 消息压缩 |
| 142 | `Transport.tls_renegotiation` | `src/protocol/transport.rs` | ⏳ | TLS重协商 |
| 143 | `RateLimiter.distributed_counter` | `src/protocol/rate_limit.rs` | ⏳ | 分布式计数 |
| 144 | `SessionSync.conflict_resolution` | `src/protocol/session_sync.rs` | ⏳ | 冲突解决 |

## K. 架构层（10 项）

| # | 特征 | 位置 | 状态 | 备注 |
|---|------|------|:----:|------|
| 145 | `OrchestrationProvider.dynamic_registration` | `src/core/provider.rs` | ⏳ | 动态注册 |
| 146 | `OrchestrationProvider.health_check` | `src/core/provider.rs` | ⏳ | 健康检查 |
| 147 | `OrchestrationProvider.failover` | `src/core/provider.rs` | ⏳ | 故障转移 |
| 148 | `OrchestrationProvider.load_balance` | `src/core/provider.rs` | ⏳ | 负载均衡 |
| 149 | `OrchestrationProvider.version_pinning` | `src/core/provider.rs` | ⏳ | 版本锁定 |
| 150 | `Bootstrap.containerized_mode` | `src/core/bootstrap.rs` | ⏳ | 容器模式 |
| 151 | `Bootstrap.secret_auto_init` | `src/core/bootstrap.rs` | ⏳ | 密钥自动初始化 |
| 152 | `Context.warm_start` | `src/core/context.rs` | ⏳ | 热启动 |
| 153 | `ConfigValidation.cross_field` | `src/core/config_validation.rs` | ⏳ | 跨字段校验 |
| 154 | `Error.severity_classification` | `src/core/error.rs` | ⏳ | 错误严重级别 |

## L. 内存层（10 项）

| # | 特征 | 位置 | 状态 | 备注 |
|---|------|------|:----:|------|
| 155 | `MemoryBridge.bidirectional_sync` | `src/memory/memory_bridge.rs` | ⏳ | 双向同步 |
| 156 | `MemoryBridge.conflict_merge` | `src/memory/memory_bridge.rs` | ⏳ | 冲突合并 |
| 157 | `MemoryPersistence.compaction` | `src/memory/memory_persistence.rs` | ⏳ | 压缩合并 |
| 158 | `MemoryRetrieval.semantic_rerank` | `src/memory/memory_retrieval.rs` | ⏳ | 语义重排序 |
| 159 | `MemoryRetrieval.hybrid_search` | `src/memory/memory_retrieval.rs` | ⏳ | 混合搜索 |
| 160 | `SemanticCache.eviction_policy` | `src/memory/semantic_cache.rs` | ⏳ | 驱逐策略 |
| 161 | `SemanticCache.temperature_aware` | `src/memory/semantic_cache.rs` | ⏳ | 温度感知 |
| 162 | `VectorStore.index_optimization` | `src/memory/vector.rs` | ⏳ | 索引优化 |
| 163 | `VectorStore.shard_management` | `src/memory/vector.rs` | ⏳ | 分片管理 |
| 164 | `AgentMemoryBus.auto_evolve` | `src/memory/agent_memory_bus.rs` | ⏳ | 自动进化 |

## M. 智能层（20 项）

| # | 特征 | 位置 | 状态 | 备注 |
|---|------|------|:----:|------|
| 165 | `AdaptiveSelector.genetic_optimizer` | `src/intelligence/adaptive_selector.rs` | ⏳ | 遗传优化器 |
| 166 | `AdaptiveSelector.bayesian_search` | `src/intelligence/adaptive_selector.rs` | ⏳ | 贝叶斯搜索 |
| 167 | `CapabilityGraph.auto_discovery` | `src/intelligence/capability_graph.rs` | ⏳ | 自动发现 |
| 168 | `CapabilityGraph.dependency_analysis` | `src/intelligence/capability_graph.rs` | ⏳ | 依赖分析 |
| 169 | `ContinuousLearning.curriculum` | `src/intelligence/continuous_learning.rs` | ⏳ | 课程学习 |
| 170 | `ContinuousLearning.transfer` | `src/intelligence/continuous_learning.rs` | ⏳ | 迁移学习 |
| 171 | `Consensus.supermajority` | `src/intelligence/consensus.rs` | ⏳ | 绝对多数 |
| 172 | `Consensus.weighted_voting` | `src/intelligence/consensus.rs` | ⏳ | 加权投票 |
| 173 | `Discovery.network_scan` | `src/intelligence/discovery.rs` | ⏳ | 网络扫描 |
| 174 | `Discovery.capability_query` | `src/intelligence/discovery.rs` | ⏳ | 能力查询 |
| 175 | `Evaluation.scoring_rubric` | `src/intelligence/evaluation.rs` | ⏳ | 评分细则 |
| 176 | `Evaluation.automated_regression` | `src/intelligence/evaluation.rs` | ⏳ | 自动化回归 |
| 177 | `HotFailover.automatic_detection` | `src/intelligence/hot_failover.rs` | ⏳ | 自动检测 |
| 178 | `HotFailover.graceful_degradation` | `src/intelligence/hot_failover.rs` | ⏳ | 优雅降级 |
| 179 | `Matcher.semantic_similarity` | `src/intelligence/matcher.rs` | ⏳ | 语义相似度 |
| 180 | `Metacognitive.self_debug` | `src/intelligence/metacognitive.rs` | ⏳ | 自调试 |
| 181 | `MetacognitivePersistence.thought_log` | `src/intelligence/metacognitive_persistence.rs` | ⏳ | 思考日志 |
| 182 | `ModelSelector.cost_aware` | `src/intelligence/model_selector.rs` | ⏳ | 成本感知 |
| 183 | `Reinforcement.differential_privacy` | `src/intelligence/reinforcement/` | ⏳ | 差分隐私 |
| 184 | `WorldModel.update_strategy` | `src/intelligence/world_model.rs` | ⏳ | 更新策略 |

## M2. 智能层子模块 — CapabilityBus（8 项）

| # | 特征 | 位置 | 状态 | 备注 |
|---|------|------|:----:|------|
| 185 | `CapabilityBus.cross_node_sync` | `src/intelligence/capability_bus/core.rs` | ⏳ | 跨节点同步 |
| 186 | `DistributedMemoryBus.partition` | `src/intelligence/capability_bus/distributed_memory_bus.rs` | ⏳ | 分区管理 |
| 187 | `MemoryBus.ttl_policy` | `src/intelligence/capability_bus/memory_bus.rs` | ⏳ | TTL策略 |
| 188 | `ObservabilityBus.metrics_export` | `src/intelligence/capability_bus/observability_bus.rs` | ⏳ | 指标导出 |
| 189 | `OptimizationBus.auto_tuning` | `src/intelligence/capability_bus/optimization_bus.rs` | ⏳ | 自动调优 |
| 190 | `OrchestrationBus.parallel_exec` | `src/intelligence/capability_bus/orchestration_bus.rs` | ⏳ | 并行执行 |
| 191 | `ProtocolBus.version_negotiation` | `src/intelligence/capability_bus/protocol_bus.rs` | ⏳ | 版本协商 |
| 192 | `ToolBus.tool_caching` | `src/intelligence/capability_bus/tool_bus.rs` | ⏳ | 工具缓存 |

## N. 编排层（20 项）

| # | 特征 | 位置 | 状态 | 备注 |
|---|------|------|:----:|------|
| 193 | `DagDriver.snapshot_recovery` | `src/orchestration/dag_driver.rs` | ⏳ | 快照恢复 |
| 194 | `DagDriver.parallel_fork` | `src/orchestration/dag_driver.rs` | ⏳ | 并行分支 |
| 195 | `DagExecutor.retry_policy` | `src/orchestration/dag_executor.rs` | ⏳ | 重试策略 |
| 196 | `ExecutionGraph.subgraph_isolation` | `src/orchestration/execution_graph.rs` | ⏳ | 子图隔离 |
| 197 | `TaskGraph.auto_partition` | `src/orchestration/task_graph.rs` | ⏳ | 自动分区 |
| 198 | `TaskGraph.dynamic_rebalance` | `src/orchestration/task_graph.rs` | ⏳ | 动态再平衡 |
| 199 | `TaskRouter.affinity_routing` | `src/orchestration/task_router.rs` | ⏳ | 亲和路由 |
| 200 | `TaskRouter.geo_routing` | `src/orchestration/task_router.rs` | ⏳ | 地理路由 |
| 201 | `FlowWithModels.auto_rollback` | `src/orchestration/flow_with_models.rs` | ⏳ | 自动回滚 |
| 202 | `FlowWithModels.version_pinning` | `src/orchestration/flow_with_models.rs` | ⏳ | 版本固定 |
| 203 | `Scheduler.priority_queue` | `src/orchestration/scheduler.rs` | ⏳ | 优先级队列 |
| 204 | `Scheduler.deadline_aware` | `src/orchestration/scheduler.rs` | ⏳ | 截止时间感知 |
| 205 | `WorkflowOptimizer.auto_parallelize` | `src/orchestration/workflow_optimizer.rs` | ⏳ | 自动并行化 |
| 206 | `WorkflowOptimizer.memory_tradeoff` | `src/orchestration/workflow_optimizer.rs` | ⏳ | 内存折衷 |
| 207 | `Council.cross_session` | `src/orchestration/council/council.rs` | ⏳ | 跨会话 |
| 208 | `Council.reputation_weighted` | `src/orchestration/council/council.rs` | ⏳ | 信誉加权 |
| 209 | `DistributedDag.remote_exec_failover` | `src/orchestration/distributed/dag_coordinator.rs` | ⏳ | 远程执行故障转移 |
| 210 | `SelfEvolution.sandbox_rollback` | `src/orchestration/self_evolution/sandbox.rs` | ⏳ | 沙箱回滚 |
| 211 | `ToolPipeline.transaction_rollback` | `src/orchestration/tool/pipeline.rs` | ⏳ | 事务回滚 |
| 212 | `ToolRecommender.context_aware` | `src/orchestration/tool/recommender.rs` | ⏳ | 上下文感知 |

## O. 韧性层（8 项）

| # | 特征 | 位置 | 状态 | 备注 |
|---|------|------|:----:|------|
| 213 | `ChaosEngine.network_partition` | `src/resilience/chaos.rs` | ⏳ | 网络分区注入 |
| 214 | `ChaosEngine.resource_exhaustion` | `src/resilience/chaos.rs` | ⏳ | 资源耗尽注入 |
| 215 | `ChaosEngine.latency_injection` | `src/resilience/chaos.rs` | ⏳ | 延迟注入 |
| 216 | `ChaosEngine.error_injection` | `src/resilience/chaos.rs` | ⏳ | 错误注入 |
| 217 | `HyperResilience.auto_recovery` | `src/resilience/hyper_resilience.rs` | ⏳ | 自动恢复 |
| 218 | `HyperResilience.circuit_breaker_reset` | `src/resilience/hyper_resilience.rs` | ⏳ | 断路器重置 |
| 219 | `HyperResilience.graceful_degradation` | `src/resilience/hyper_resilience.rs` | ⏳ | 优雅降级 |
| 220 | `HyperResilience.rate_limiter_auto` | `src/resilience/hyper_resilience.rs` | ⏳ | 自动限流器调整 |

## P. 安全层（10 项）

| # | 特征 | 位置 | 状态 | 备注 |
|---|------|------|:----:|------|
| 221 | `Mtls.cert_auto_renew` | `src/security/mtls.rs` | ⏳ | 证书自动续期 |
| 222 | `Mtls.revocation_check` | `src/security/mtls.rs` | ⏳ | 吊销检查 |
| 223 | `PromptInjection.ml_detector` | `src/security/prompt_injection.rs` | ⏳ | ML检测器 |
| 224 | `PromptInjection.adaptive_threshold` | `src/security/prompt_injection.rs` | ⏳ | 自适应阈值 |
| 225 | `RequestSigning.key_rotation` | `src/security/request_signing.rs` | ⏳ | 密钥轮换 |
| 226 | `RequestSigning.nonce_replay_protection` | `src/security/request_signing.rs` | ⏳ | 重放保护 |
| 227 | `SecretRotation.auto_rotate` | `src/security/secret_rotation.rs` | ⏳ | 自动轮换 |
| 228 | `VulnerabilityScan.sbom_export` | `src/security/vulnerability_scan.rs` | ⏳ | SBOM导出 |
| 229 | `ContentSafety.auto_censor` | `src/security/content_safety.rs` | ⏳ | 自动审查 |
| 230 | `AuditIntegrity.chain_validation` | `src/security/audit_integrity.rs` | ⏳ | 链验证 |

## Q. 多模态层（6 项）

| # | 特征 | 位置 | 状态 | 备注 |
|---|------|------|:----:|------|
| 231 | `AudioProcessor.streaming_transcribe` | `src/multimodal/audio_processor.rs` | ⏳ | 流式转录 |
| 232 | `AudioProcessor.speaker_diarization` | `src/multimodal/audio_processor.rs` | ⏳ | 声纹识别 |
| 233 | `VideoProcessor.frame_extraction` | `src/multimodal/video_processor.rs` | ⏳ | 帧提取 |
| 234 | `VideoProcessor.motion_analysis` | `src/multimodal/video_processor.rs` | ⏳ | 运动分析 |
| 235 | `DocumentParser.ocr_fallback` | `src/multimodal/document_parser.rs` | ⏳ | OCR回退 |
| 236 | `CodeRepoAnalyzer.dependency_graph` | `src/multimodal/code_repo_analyzer.rs` | ⏳ | 依赖图 |

## R. 联邦学习层（6 项）

| # | 特征 | 位置 | 状态 | 备注 |
|---|------|------|:----:|------|
| 237 | `FederatedRLAdapter.model_aggregation` | `src/intelligence/reinforcement/federated.rs` | ⏳ | 模型聚合 |
| 238 | `FederatedRLAdapter.round_management` | `src/intelligence/reinforcement/federated.rs` | ⏳ | 回合管理 |
| 239 | `FederatedDiscovery.peer_discovery` | `src/intelligence/reinforcement/federated_discovery.rs` | ⏳ | 节点发现 |
| 240 | `FederatedPrivacy.mpc_protocol` | `src/intelligence/reinforcement/federated_privacy.rs` | ⏳ | MPC协议 |
| 241 | `FederatedTransport.secure_channel` | `src/intelligence/reinforcement/federated_transport.rs` | ⏳ | 安全通道 |
| 242 | `FederatedVersioning.model_migration` | `src/intelligence/reinforcement/federated_versioning.rs` | ⏳ | 模型迁移 |

## S. GUI 层（22 项）

| # | 特征 | 位置 | 状态 | 备注 |
|---|------|------|:----:|------|
| 243 | `detect_initial_window_title()` | `gui/src/app.rs` | ⏳ | 程序化标题检测 |
| 244 | `provider_catalog_rpc()` | `gui/src/backend.rs` | ⏳ | Provider目录RPC |
| 245 | `delete_secret_key()` | `gui/src/keyring_util.rs` | ⏳ | 密钥管理UI |
| 246 | `autotune_save_state()` | `gui/src/views/autotune.rs` | ⏳ | 自动调优持久化 |
| 247 | `expand_prompt_command()` | `gui/src/views/chat/chat_impl.rs` | ⏳ | Prompt命令扩展 |
| 248 | `current_category_templates()` | `gui/src/views/prompts.rs` | ⏳ | 模板浏览 |
| 249 | `search_templates()` | `gui/src/views/prompts.rs` | ⏳ | 模板搜索 |
| 250 | `PROVIDER_NAMES` dynamic | `gui/src/views/providers.rs` | ⏳ | 动态Provider列表 |
| 251 | `monitor/metrics_dashboard` | `gui/src/views/monitor.rs` | ⏳ | 监控仪表盘增强 |
| 252 | `config_editor/schema_validation` | `gui/src/views/config_editor.rs` | ⏳ | 配置编辑器校验 |
| 253 | `security/audit_viewer` | `gui/src/views/security.rs` | ⏳ | 审计查看器 |
| 254 | `workflow/visual_editor` | `gui/src/views/workflow.rs` | ⏳ | 工作流可视化 |
| 255 | `skills/import_ui` | `gui/src/views/skills.rs` | ⏳ | 技能导入UI |
| 256 | `setup/wizard_ux` | `gui/src/views/setup.rs` | ⏳ | 设置向导 |
| 257 | `settings/advanced_options` | `gui/src/views/settings.rs` | ⏳ | 高级选项 |
| 258 | `about/version_check` | `gui/src/views/about.rs` | ⏳ | 版本检查 |
| 259 | `chat/streaming_animation` | `gui/src/views/chat/chat_impl/ui.rs` | ⏳ | 流式动画 |
| 260 | `chat/message_search` | `gui/src/views/chat/chat_impl/ui.rs` | ⏳ | 消息搜索 |
| 261 | `chat/conversation_export` | `gui/src/views/chat/chat_impl/storage.rs` | ⏳ | 会话导出 |
| 262 | `chat/attachment_preview` | `gui/src/views/chat/chat_impl/render.rs` | ⏳ | 附件预览 |
| 263 | `theme/custom_css` | `gui/src/theme.rs` | ⏳ | 自定义主题 |
| 264 | `i18n/live_switch` | `gui/src/i18n/mod.rs` | ⏳ | 实时语言切换 |

## T. VS Code Addon 层（6 项）

| # | 特征 | 位置 | 状态 | 备注 |
|---|------|------|:----:|------|
| 265 | `advanced_edit/suggestion` | `vscode-addon/src/advancedEdit.ts` | ⏳ | 高级编辑建议 |
| 266 | `approval_panel/history` | `vscode-addon/src/approvalPanel.ts` | ⏳ | 审批历史 |
| 267 | `multi_agent_panel/comparison` | `vscode-addon/src/multiAgentPanel.ts` | ⏳ | 智能体对比 |
| 268 | `process_flow/live_update` | `vscode-addon/src/processFlowView.ts` | ⏳ | 流程实时更新 |
| 269 | `workflow_view/debug_mode` | `vscode-addon/src/workflowView.ts` | ⏳ | 工作流调试 |
| 270 | `status_monitor/alert_action` | `vscode-addon/src/statusMonitor.ts` | ⏳ | 告警操作 |

## U. SDK 层（15 项）

| # | 特征 | 语言 | 状态 | 备注 |
|---|------|:----:|:----:|------|
| 271 | `Client.streaming_timeout` | Rust SDK | ⏳ | 流式超时 |
| 272 | `Client.retry_policy` | Rust SDK | ⏳ | 重试策略 |
| 273 | `Client.concurrency_limit` | Rust SDK | ⏳ | 并发限制 |
| 274 | `Client.circuit_breaker` | Rust SDK | ⏳ | 断路器 |
| 275 | `Types.batch_operations` | Rust SDK | ⏳ | 批处理操作 |
| 276 | `Client.streaming_timeout` | Python SDK | ⏳ | 流式超时 |
| 277 | `Client.retry_policy` | Python SDK | ⏳ | 重试策略 |
| 278 | `Client.concurrency_limit` | Python SDK | ⏳ | 并发限制 |
| 279 | `Client.circuit_breaker` | Python SDK | ⏳ | 断路器 |
| 280 | `Types.batch_operations` | Python SDK | ⏳ | 批处理操作 |
| 281 | `Client.streaming_timeout` | TypeScript SDK | ⏳ | 流式超时 |
| 282 | `Client.retry_policy` | TypeScript SDK | ⏳ | 重试策略 |
| 283 | `Client.concurrency_limit` | TypeScript SDK | ⏳ | 并发限制 |
| 284 | `Client.circuit_breaker` | TypeScript SDK | ⏳ | 断路器 |
| 285 | `Types.batch_operations` | TypeScript SDK | ⏳ | 批处理操作 |

## V. 部署层（6 项）

| # | 特征 | 位置 | 状态 | 备注 |
|---|------|------|:----:|------|
| 286 | `K8s.hpa_autoscaling` | `deploy/k8s/` | ⏳ | HPA自动伸缩 |
| 287 | `K8s.vertical_scaling` | `deploy/k8s/` | ⏳ | 垂直伸缩 |
| 288 | `K8s.pod_disruption_budget` | `deploy/k8s/` | ⏳ | Pod干扰预算 |
| 289 | `MultiUsers.rate_limit_per_user` | `deploy/multi-users-server/` | ⏳ | 用户限流 |
| 290 | `SimpleServer.metrics_endpoint` | `deploy/simple-server/` | ⏳ | 指标端点 |
| 291 | `Docker.healthcheck` | `deploy/` | ⏳ | 健康检查 |

## W. 测试层（5 项）

| # | 特征 | 位置 | 状态 | 备注 |
|---|------|------|:----:|------|
| 292 | `chaos_drill/network_chaos` | `tests/chaos_drill.rs` | ⏳ | 网络混沌测试 |
| 293 | `chaos_drill/resource_exhaustion` | `tests/chaos_drill.rs` | ⏳ | 资源耗尽测试 |
| 294 | `e2e/federated_learning` | `tests/e2e/test_federated_learning_e2e.rs` | ⏳ | 联邦学习E2E |
| 295 | `e2e/self_evolution` | `tests/e2e/test_self_evolution_e2e.rs` | ⏳ | 自进化E2E |
| 296 | `benchmark/streaming_throughput` | `tests/streaming_e2e_benchmark.rs` | ⏳ | 流吞吐量基准 |

## X. i18n 层（2 项）

| # | 特征 | 位置 | 状态 | 备注 |
|---|------|------|:----:|------|
| 297 | `Runtime.lazy_loading` | `src/i18n/runtime.rs` | ⏳ | 惰性加载 |
| 298 | `Watcher.directory_monitor` | `src/i18n/watcher.rs` | ⏳ | 目录监视器 |

## Y. 安全治理层（2 项）

| # | 特征 | 位置 | 状态 | 备注 |
|---|------|------|:----:|------|
| 299 | `SecurityAdvisor.auto_remediation` | `src/security/security_advisor.rs` | ⏳ | 自动修复建议 |
| 300 | `DriftProtection.auto_heal` | `src/governance/drift/drift_protection.rs` | ⏳ | 自动修复 |

---

## 汇总统计

| 类别 | 项数 | ✅ 已激活 | ⏳ 待激活 | 激活率 |
|------|:----:|:---------:|:---------:|:------:|
| A. ACP Prelude | 40 | 0 | 40 | 0% |
| B. Conversation | 15 | 0 | 15 | 0% |
| C. Metrics | 40 | 0 | 40 | 0% |
| D. Misc | 10 | 0 | 10 | 0% |
| E. Request | 10 | 0 | 10 | 0% |
| F. Runtime | 8 | 0 | 8 | 0% |
| G. Governance | 3 | 0 | 3 | 0% |
| H. Observability | 8 | 0 | 8 | 0% |
| I. Shared | 5 | 0 | 5 | 0% |
| J. Protocol | 5 | 0 | 5 | 0% |
| K. Architecture | 10 | 0 | 10 | 0% |
| L. Memory | 10 | 0 | 10 | 0% |
| M. Intelligence | 28 | 0 | 28 | 0% |
| N. Orchestration | 20 | 0 | 20 | 0% |
| O. Resilience | 8 | 0 | 8 | 0% |
| P. Security | 10 | 0 | 10 | 0% |
| Q. Multimodal | 6 | 0 | 6 | 0% |
| R. Federated | 6 | 0 | 6 | 0% |
| S. GUI | 22 | 0 | 22 | 0% |
| T. VS Code | 6 | 0 | 6 | 0% |
| U. SDK | 15 | 0 | 15 | 0% |
| V. Deploy | 6 | 0 | 6 | 0% |
| W. Test | 5 | 0 | 5 | 0% |
| X. i18n | 2 | 0 | 2 | 0% |
| Y. Security Gov | 2 | 0 | 2 | 0% |
| **总计** | **300** | **0** | **300** | **0%** |

> 注：此表仅追踪 F-GAP-49 通用预留项。其他 GAP 系列（如 F-GAP-05/06/07/08/11/15/19/25/51/87）已单独追踪在 blue61.md 中。
