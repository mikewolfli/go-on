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
