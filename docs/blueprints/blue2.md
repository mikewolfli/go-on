# BLUE2: Targeted Reinforcement Plan (Aligned with BLUE1, Avoiding Whole-Repo Overreach)

> **完成状态** (截至 2026-04-07, 全部完成):
> - ✅ Section 1 — 主链路错误处理加固
> - ✅ Section 2 — 关键边界可观测性补点
> - ✅ Section 3 — Durable 产物账本 (`.goon/` ArtifactLedger)
> - ✅ Section 4 — Action Check 与 Healthcheck 前置
> - ✅ Section 5 — 子任务生命周期追踪（`start_ts/stop_ts/duration_ms/outcome/executor` + `mark_executed()` + `spec/latest-execution.json` 持久化）
> - ✅ Section 6 — 受控 Sub Agent 编排（`task.execute` RPC：plan → sequential sub-agent dispatch → per-subtask lifecycle recording → ledger 持久化）
> - ✅ Section 7 — QA Gate 强制执行（FullAuto + complexity>=3 自动触发 `ActionCheckKind::Qa`；`chat.qa_gate` 通知；warn! on incomplete artifacts）
>
> **验证**: `cargo check --all` ✅ · `cargo test` 202/202 ✅
## Positioning
- 本清单不是“全仓一刀切整改”，而是围绕已落地的 BLUE1 主流程继续加固运行时主链路、关键失败面、可观测边界与恢复能力。
- 原则：优先修正真正影响执行正确性、排障效率、审计闭环的点；不把所有模块、所有函数、所有错误都强行收敛到同一种实现方式。
- 判断标准：对运行时主链路有直接收益的，属于“正好加强”；只增加抽象、重复已有机制、制造 tracing 噪音或错误类型耦合的，视为过度。

## 1. 主链路错误处理加固 ✅
> 已落地：`cache.rs` 6 个操作上下文、`vector.rs` 8 个操作上下文、`acp.rs` 请求循环带 method 字段、`mcp_server.rs` HTTP 解析失败 warn!、`mcp.rs` 未知方法/资源 warn!。
- 范围限定在用户可见、主流程强相关模块：`acp.rs`、`mcp.rs`、`mcp_server.rs`、`tool.rs`、`flow.rs`、`cache.rs`、`vector.rs`、`setup.rs`、`main.rs`。
- 目标不是“全仓统一成单一错误枚举”，而是统一主链路的错误边界、上下文信息、日志与上报格式。
- 优先处理以下问题：
  - 缺少上下文的 `anyhow!` / `bail!`
  - 主链路上的 silent failure 或 silent fallback
  - join error / mutex poisoned / IO / network / config error 没有统一分类
  - 对外返回信息与内部 trace 信息脱节
- 保留分层错误模型：底层模块可以继续使用 `anyhow::Result` 或模块内错误类型；跨模块边界、RPC 响应、主流程出口应统一映射到稳定的错误语义。
- 禁止事项只针对主链路：禁止 silent fallback、禁止吞掉错误后继续伪成功、禁止无 trace_id 的关键失败。

## 2. 关键边界可观测性补点 ✅
> 已落地：pipeline 入口 `info!(trace_id)` · review gate 降级 `warn!(trace_id)` · sqlite cache miss `debug!` · all-agents-failed `error!(trace_id, phase, errors)` · `tool.rs` RunTestsTool/ApplyPatchTool `debug!/warn!` · `mcp_server.rs` dispatch `debug!`。
- 不追求“所有函数都加 `#[instrument]`”，只补主链路和高价值边界：
  - pipeline 入口与阶段切换
  - review gate / route / degrade / fallback 决策点
  - 外部模型调用、MCP 调用、IO、网络
  - cache / vector 查询与写入
  - 并发任务 join / timeout / cancellation
- 埋点目标：让一次失败请求能沿 `trace_id` 看到入口、阶段、分支、降级、错误归因，而不是增加大量低信号 span。
- 建议埋点形式：
  - 阶段入口与关键边界：`#[instrument(level = "info" | "debug")]`
  - 状态变更、分支选择、命中率、退化路径：`tracing::info/debug`
  - 错误、拒绝、超时、降级：`tracing::warn/error`
- 成功标准：出现故障时，能从 trace 中直接回答“在哪个阶段失败、为什么降级、用了哪个 fallback、对外结果是什么”。

## 3. Durable 产物账本 ✅
> 已落地：`src/reinforcement.rs` `ArtifactLedger`，`.goon/{spec,qa,checkpoints,retest,action-checks,final}/` 结构，checkpoint 摘要自动持久化。
- 建议新增 `.goon/` 持久化目录，但仅存放高价值工程产物，不复制现有运行时内存结构。
- 建议落盘内容：
  - intake / spec / sprint plan
  - latest checkpoint 摘要
  - QA 报告、修复日志、复测报告、最终结论
- 目标：支持跨会话恢复、审计回放、长任务中断续跑，避免只依赖聊天上下文和临时内存状态。
- 边界约束：不要把所有运行时瞬时状态都写盘；只保留恢复和审计真正需要的产物。

## 4. Action Check 与 Healthcheck 前置 ✅
> 已落地：`action.check` / `task.plan` RPC 处理器、`acp_action_check` / `acp_task_plan` MCP 工具、`runtime.health` 结构化报告、`--healthcheck` / `--action-check` / `--plan-task` CLI 参数。
  - 规格产物结构检查
  - QA 报告结构检查
  - 复测结果完整性检查
  - 最终报告是否引用了验证证据
  - 缓存
  - 向量存储
  - MCP 通道
  - 网络可用性
  - breaker / rate limit 状态
  - 配置健康

## 5. 子任务与子代理生命周期追踪 ✅
> 已落地：`PlannedSubtaskRecord` 新增 `start_ts/stop_ts/duration_ms/outcome/executor` 可选字段 + `mark_executed()` 方法；`TaskExecutionSummary` 结构体持久化至 `spec/latest-execution.json`。
## 6. 受控 Sub Agent 编排（按复杂度启用） ✅
> 已落地：`task.execute` RPC — build_task_plan() -> routing primary agent -> subtask mark_executed() -> persist_task_execution_summary() 落盘。
>
> 部分覆盖（历史）：`TaskPlanArtifact` 含 `planned_subtasks`、`sub_agent_recommended`、`activation_reasons`；`task.plan` RPC 已落地；执行调度与并行编排待续。
  - 高复杂度任务拆解
  - 明确存在并行机会的任务
  - 研究 / 实现 / 测试 / 评审职责可明确切分的任务
  - 需要长链路证据与恢复能力的 FullAuto 场景
  - 单文件小改动
  - 低风险问答
  - 已能由单代理稳定完成的短链路任务
  - 复用现有 `TaskDecomposer`、`AgentRole`、`HandoffContract`、routing 与 review gate 语义
  - Sub agent 必须有明确目标、输入、输出、超时、重试与失败归因
  - Sub agent 输出必须回收进主链路 trace、checkpoint 与审计证据链
  - 不允许让 sub agent 绕过 review gate、healthcheck、Action Check 或主流程错误边界
  - 先做受控委派与生命周期观测
  - 再做有限并行
  - 最后才考虑更激进的自治式子代理网络
  - 子代理能带来更清晰的职责切分或更高的成功率
  - 不引入第二套调度真相源
  - 故障时能准确回放“哪个子代理因何失败，由谁接管”

## 7. QA / 复测 / 最终结论三级 Gate ✅
> 已落地：handle_chat FullAuto + complexity>=3 自动触发 ActionCheckKind::Qa；chat.qa_gate 通知；warn! on incomplete artifacts。
>
> 基础结构就绪（历史）（`ActionCheckKind::{Qa,Retest,Final}` + `FinalSummaryArtifact`）；三级强制 gate 逻辑待续，当前仅在显式调用 `action.check` 时触发。
- 这项增强适合高风险任务、FullAuto、发布前流程，不适合所有请求默认开启。
- 要求：
  - QA 有结构化审计结果
  - 修复后必须有复测结论
  - 最终结论必须引用前两者的证据
- 任一关失败，进入 fix loop；但普通低风险请求可以走轻量路径，避免主流程过重。

## 8. 暂缓项与防过度约束
- 以下方向暂不作为全仓整改目标：
  - 把所有模块统一成单一错误类型
  - 给所有高频函数补 `#[instrument]`
  - 再造一套独立于现有 flow/checkpoint/review gate 的总状态机真相源
  - 在没有稳定磁盘产物流转之前，提前做严格的“角色文件所有权系统”
  - 为所有命令路径强制启用 QA/复测/最终结论三级 gate
  - 把 sub agent 作为所有请求默认必经层，或允许其绕过主链路控制面
- 原因：这些项要么与现有机制重复，要么收益递减，要么会显著抬高复杂度和维护成本。

## 9. 推荐执行顺序
1. 主链路错误处理加固
2. 关键边界可观测性补点
3. Durable 产物账本
4. Action Check 与 Healthcheck 前置
5. 子任务生命周期追踪
6. 受控 Sub Agent 编排
7. 高风险路径的三级 QA Gate

## 10. 完成定义
- BLUE2 的完成不以“全仓统一率”衡量，而以以下结果衡量：
  - 主链路失败可被准确分类、记录、上报
  - 关键决策点可沿 `trace_id` 回放
  - 长任务可通过持久化产物恢复和审计
  - 高复杂度任务可在受控 sub agent 编排下获得更清晰的职责切分与失败归因
  - 高风险流程有可执行验收定义与复测闭环
  - 新增机制不与 BLUE1 已有控制面重复，不制造第二套真相源

---

> 本清单用于对 BLUE1 已完成能力做“窄而深”的强化，重点是主流程闭环、失败归因、恢复能力与审计证据链，而不是追求全仓形式统一。
