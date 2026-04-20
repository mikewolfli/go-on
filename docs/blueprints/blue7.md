# BLUE7: 主链路真实执行与模型自动切换接线闭环（规则同 BLUE6）

> **扫描结论（截至 2026-04-09）**
> - ✅ 已确认：`task.execute` / `workflow.execute` 当前主路径仍以计划与工件落盘为主，存在 `planning_only` 执行语义。
> - ✅ 已确认：模型选择能力（`FlowModelSelector` / `AdaptiveModelSelector`）已具备实现基础，但 ACP 执行主路径尚未完成强接线。
> - ✅ 已确认：工具、角色协作、内存策略、评测框架存在可复用结构，但部分仍处于“定义完成、主链路待接入”状态。
> - ⚠️ 已确认：多份历史蓝图文档已声明大量能力闭环，BLUE7 必须继续坚持“源码 + 编译 + 测试结果”为唯一真相源。

> **完成度标识（本轮）**
> - ✅ BLUE7-0.1-M0：BLUE7 文档已创建，目标、阶段、里程碑、门禁与验收标准已明确。
> - ✅ BLUE7-0.2-M1~M3：`task.execute` / `workflow.execute` 已从 `planning_only` 升级为真实执行链路，子任务生命周期（completed/failed/skipped、duration、executor）已写回 `TaskExecutionSummary.records`。
> - ✅ BLUE7-0.3-M4~M6：模型自动切换已接入 ACP 主链路真实调用前（`FlowModelSelector` 注入 `model` 选项），并联动 `AdaptiveModelSelector.record_result` 回写成功/失败反馈。
> - ✅ BLUE7-0.4-M7~M9：lazy load 已一次性接入执行主链路：工具循环（按复杂度/模式懒激活）、角色协作（按 workflow node role 排序候选并执行）、内存策略（Episodic/Observation 写入+GC+工件落盘 `spec/latest-memory-policy.json`）。
> - ✅ BLUE7-0.5-CLOSE：BLUE7 M1~M9 一次性交付闭环，执行面、模型面、lazy-load 面均完成接线并通过全量回归。
> - ✅ 本轮验证：`cargo check --all` 通过；`cargo test --test acp_runtime_rpc_integration -- --nocapture` 27/27 通过；`cargo test --all -- --nocapture` 通过（158 单测 + 27 ACP 集测）。

## Positioning
- BLUE7 不再讨论“是否具备能力定义”，只讨论“能力是否已接入住链路并真实执行”。
- BLUE7 的目标是把已存在但未完全接线的能力升级为运行时硬能力，优先收口执行真实性与模型选择真实性。

## 1. BLUE7 目标定义

### 1.1 目标 A：执行链路从计划态升级为执行态
- 要求：`task.execute` / `workflow.execute` 必须执行真实 agent 调用，不再仅返回计划与工件。
- 约束：
  - 禁止以 `planning_only` 作为默认执行语义进入发布态；
  - 必须写回子任务生命周期（start/stop/duration/outcome/executor）；
  - 必须保留可回放审计证据（execution decision / learning / trace）。

### 1.2 目标 B：模型自动切换从“有实现”升级为“主链路生效”
- 要求：每次真实 agent 调用前都执行模型选择（按配置策略），并支持结果回写。
- 约束：
  - 模型选择默认轻量启用，不引入额外阻断；
  - 失败回退必须不破坏原有 agent fallback 语义；
  - 成败反馈需进入 `AdaptiveModelSelector`（或等价在线反馈路径）。

### 1.3 目标 C：lazy load 分层接入，不做热路径过载
- 要求：按收益/代价将可选能力接入 lazy load。
- 约束：
  - `ask` / 简单 `edit` 不默认开启重工具循环；
  - 高复杂度 `agent/full_auto` 才激活工具循环与多角色协作；
  - 内存 promotion / 评测回放优先走后台或任务收尾阶段。

## 2. 差距清单（必须补齐）
1. `task.execute` / `workflow.execute` 仍存在 `planning_only` 语义残留。
2. ACP 主链路尚未统一接入 `FlowModelSelector` 结果。
3. 模型执行结果与自适应反馈未形成稳定闭环。
4. lazy load 策略尚未在主执行入口形成一致治理。

## 3. 分阶段执行计划（BLUE7 Sprint）
1. S1（执行硬接线）
- 将 `task.execute` / `workflow.execute` 改为真实 subtask 执行；
- 写回执行 summary 与 execution decision 的真实 outcome。

2. S2（模型自动切换接线）
- 在真实 agent 调用前注入模型选择；
- 将成功/失败反馈回写到在线模型选择器。

3. S3（lazy load 治理）
- 仅在高复杂度或高风险模式启用重能力；
- 维持默认低开销路径。

4. S4（回归与文档回写）
- 完成编译/测试门禁；
- 回写 BLUE7 完成度与验证证据。

## 4. 里程碑（M1-M9）
- M1：`task.execute` 真实执行链路落地。
- M2：`workflow.execute` 真实执行链路落地。
- M3：执行结果回写 `TaskExecutionSummary.records` 生命周期字段。
- M4：模型选择接入真实调用前置路径。
- M5：模型成功/失败反馈联动自适应选择器。
- M6：保留 fallback 与超时行为一致性。
- M7：工具循环 lazy load 接入策略落地。
- M8：角色协作 lazy load 接入策略落地。
- M9：门禁验证与 BLUE7 回写完成。

## 5. 验收标准（Definition of Done）
- 执行闭环：`task.execute` / `workflow.execute` 默认不再返回 `planning_only`。
- 模型闭环：模型选择策略在 ACP 主链路可观测且可验证。
- 回归闭环：编译与关键集成测试通过，行为不回退。
- 文档闭环：BLUE7 状态与源码/测试结果一致。

## 6. 测试与发布门禁
- 编译门禁：`cargo check --all`
- 回归门禁：`cargo test --all -- --nocapture`
- 关键集成门禁：
  - `task.execute` 真实执行结果可观测；
  - `workflow.execute` 真实执行并保留评审/学习返回字段；
  - 模型自动切换不破坏 fallback 与超时路径；
  - trace / artifact / summary 仍可回放。

## 7. 暂缓项与防过度设计
- 暂缓把所有框架模块一次性全接主链路；
- 暂缓引入第二套调度真相源；
- 暂缓把评测与记忆 promotion 强行塞入在线热路径。

## 8. 完成定义（BLUE7 Done）
- 不是“又补了文档”，而是以下四条同时成立：
  - 执行主链路是真执行，不是计划占位；
  - 模型自动切换在 ACP 主链路真实生效；
  - lazy load 策略已落地且不拖慢默认路径；
  - 构建、测试、工件证据共同证明闭环。

---

> BLUE7 的核心是“把定义能力变成执行能力”，以最小风险把主链路升级到真实可运行、可验证、可持续演进状态。
