# BLUE2: Remaining Improvement Opportunities (Rules Reference: BLUE1.MD)

## 1. 错误处理集成（参考 BLUE1.MD 质量统一与主流程强约束原则）
- 全项目所有主流程、调度、模型选择、外部调用、IO、缓存、向量、网络、任务分解、优化器等模块，所有错误分支（unwrap/expect/anyhow/bail/兜底默认/Err/Option::None）应统一为 AgentError/Result 或对应模块统一错误类型。
- 禁止 silent fallback，所有错误必须显式处理、记录、上报。
- 参考 BLUE1.MD：所有 pipeline/route/execute/verify/evaluate/learn 阶段必须有强约束的错误信号流转，不能因兜底默认值导致 silent failure。
- 建议：批量排查 agent.rs、acp.rs、mode.rs、tool.rs、orchestrator.rs、flow.rs、mcp_server.rs、selector.rs、optimizer.rs、cache.rs、memory.rs、vector.rs、setup.rs、task.rs、roles.rs、promotion.rs、audit.rs、error.rs、hardening.rs、reliability_optimizer.rs、speed_optimizer.rs、workflow_optimizer.rs、evaluation.rs、failure_prevention.rs、context.rs、config.rs、i18n.rs、i18n_watcher.rs、graph.rs、telemetry.rs、pua.rs、task_decomposer.rs、task_graph.rs、adaptive_selector.rs、advanced_modules.rs。
- 具体见 agent.rs 代表性清单（详见 BLUE1.MD 执行管道与质量模型章节）。

## 2. 性能监控扩展（参考 BLUE1.MD 深度可观测性与主流程埋点原则）
- 所有主流程、调度、模型选择、外部调用、IO、缓存、向量、网络、任务分解、优化器、评估、硬化、回退、分支、并发、异步等高频/高耗时路径，均应补充 #[instrument] 或 tracing 埋点。
- 埋点类型建议：
  - pipeline/route/execute/verify/evaluate/learn 入口：#[instrument(level = "info")]
  - 关键分支/外部调用/IO/网络/缓存/向量/模型选择/优化器：tracing::info/debug
  - 错误/降级/回退/异常：tracing::warn/error
- 参考 BLUE1.MD：所有 pipeline 阶段、review gate、online controller、cache/vector/telemetry 路径必须有 trace_id 贯穿和事件流。
- 建议：批量补充 acp.rs、agent.rs、mode.rs、tool.rs、orchestrator.rs、flow.rs、mcp_server.rs、selector.rs、optimizer.rs、cache.rs、memory.rs、vector.rs、setup.rs、task.rs、roles.rs、promotion.rs、audit.rs、error.rs、hardening.rs、reliability_optimizer.rs、speed_optimizer.rs、workflow_optimizer.rs、evaluation.rs、failure_prevention.rs、context.rs、config.rs、i18n.rs、i18n_watcher.rs、graph.rs、telemetry.rs、pua.rs、task_decomposer.rs、task_graph.rs、adaptive_selector.rs、advanced_modules.rs。
- 具体见 agent.rs 代表性清单（详见 BLUE1.MD 深度可观测性章节）。

---

> 本清单为 go-on 项目全量剩余改进空间，所有建议均严格引用 BLUE1.MD 路线图与质量/可观测性强约束规则，后续批量修正与 review 应以此为准。

## 3. 可借鉴的流程工程化增强（仅保留可落地能力）

### 3.1 Durable 状态与产物账本（建议新增）
- 为多轮/多天任务建立持久化工程产物目录（建议 `.goon/`），将以下内容落盘：
  - 需求澄清（intake）
  - 规格与分解（spec/sprint plan）
  - 执行状态机快照（status/checkpoint）
  - QA 报告、修复日志、复测报告、最终报告
- 目标：避免仅依赖聊天上下文，支持中断恢复、跨会话追踪、审计回放。
- 对齐 BLUE1.MD：强化 pipeline 可追溯性与 review 证据链闭环。

### 3.2 显式状态机（phase + pending_action）
- 将当前执行状态统一为 machine-readable 状态机字段：
  - `phase`
  - `pending_action`
  - `current_iteration`
  - `approval_required`
  - `last_executor`
- 所有执行入口先读取状态机判断“合法下一步”，非法跳转直接拒绝并给出修复建议。
- 对齐 BLUE1.MD：Route/Review Gate 从建议型升级为强约束执行控制。

### 3.3 角色产物所有权边界
- 明确 Planner/Generator/Evaluator/Orchestrator 的文件所有权边界，禁止跨角色无约束写入核心产物。
- 在运行前置检查中增加“产物所有权校验”，防止错误角色污染状态源。
- 对齐 BLUE1.MD：模块职责清晰化，降低 ACP 主流程耦合与回归风险。

### 3.4 动作完成检查器（Action Check）
- 为每个关键动作定义可执行验收脚本（例如 contract 检查、QA 报告结构检查、复测结论检查）。
- 状态推进前必须通过对应检查器，否则不允许进入下一阶段。
- 对齐 BLUE1.MD：从“人工判断完成”升级为“可执行完成定义（DoD）”。

### 3.5 会话恢复与压缩前快照钩子
- 在会话开始、上下文压缩前、关键阶段切换时，自动刷新“latest checkpoint”快照。
- 增加恢复策略：发生中断时优先以 checkpoint + status 恢复，而不是依赖最近消息。
- 对齐 BLUE1.MD：提高长流程稳定性，降低 context loss 风险。

### 3.6 子代理生命周期可观测性
- 统一记录子任务/子代理生命周期事件：start/stop/duration/outcome/retry_count。
- 将事件并入现有 trace_id 体系，支持按任务链路回放瓶颈与失败点。
- 对齐 BLUE1.MD：扩展深度可观测性到“任务编排层”。

### 3.7 运行时健康检查标准化
- 增加统一 runtime healthcheck 协议（缓存、向量、MCP、网络、限流、breaker、配置健康）。
- 在 QA 前与发布前强制执行 healthcheck，失败则阻断进入下一阶段。
- 对齐 BLUE1.MD：把稳定性验证前置，减少后置故障修复成本。

### 3.8 QA/复测/最终结论三级审计门
- 将 QA、复测、最终报告分别作为独立 gate，要求每一关有结构化审计结果。
- 任一关失败必须进入 fix loop，不允许跳过复测直接结束。
- 对齐 BLUE1.MD：强化质量模型闭环与失败驱动迭代。

### 3.9 命令面分层（Plan/Build/QA/Full）
- 建议在操作层提供分层命令入口：
  - Plan：仅需求澄清与规格
  - Build：仅实现与修复
  - QA：仅验证与复测
  - Full：端到端自动编排
- 目标：减少误操作，提高阶段边界清晰度与可审计性。

### 3.10 规则执行优先级
- 将以下规则设为硬性优先级：
  1. 状态机合法性
  2. 产物完整性与所有权
  3. Action Check 通过
  4. Healthcheck 通过
  5. 才允许状态推进
- 对齐 BLUE1.MD：把“流程纪律”落实为运行时硬门，而非文档约定。
