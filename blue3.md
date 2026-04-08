# BLUE3: 全项目精炼与终态自治优化方案（规则同 BLUE2）

> **扫描结论（截至 2026-04-07）**
> - ✅ 已具备：任务分析、角色路由、任务拆解、子任务生命周期、Action Check、Healthcheck、持久化账本、向量检索自动调优（局部）
> - ⚠️ 未达终态：尚未达到“最完美 agents proxy（全链路自治、并行、自学习、自动研究讨论、自动工作流编排与分配）”
> - ⚠️ 关键差距：存在“声明能力”与“主链路接入”不一致，部分模块偏离执行闭环
>
> **完成度标识（本轮）**
> - ✅ BLUE3-3.2-M1：`task.execute` 已落地“按 `phase_index` 分组并行（组内并行、组间 barrier）”
> - ✅ BLUE3-3.2-M1：并行度可配置（`phase_max_inflight` / `subtask_parallelism`）
> - ✅ BLUE3-3.2-M1：失败策略可配置（`tolerant` / `fail_fast`）并产出并行执行指标
> - ✅ BLUE3-3.2-M2：并行执行指标已持久化到 `spec/latest-execution.json`
> - ✅ BLUE3-3.5-M3：新增工作流入口 `workflow.generate` / `workflow.execute`（复用 `task.plan` / `task.execute` 主链路）
> - ✅ BLUE3-3.5-M4：`workflow.execute` 已自动触发 `QA -> Retest -> Final` Gate，并返回 gate 报告
> - ✅ BLUE3-3.3-M5：新增 `workflow.research`，自动产出结构化研讨结果并落盘 `spec/latest-research.json`
> - ✅ BLUE3-3.4-M6：LearningBus 执行事件已落盘 `spec/latest-learning.json`（窗口化保留）
> - ✅ BLUE3-3.5-M7：`workflow.execute` 已支持按 `phase_index` 自动分配执行代理（多代理可用时）
> - ✅ BLUE3-3.4-M8：`workflow.execute` 已支持基于 LearningBus 的并行度自适应调参（默认开启）
> - ✅ BLUE3-3.5-M9：`workflow.generate` 已产出独立工作流 DAG（节点依赖/角色/超时/重试/优先级）并落盘 `spec/latest-workflow.json`
> - ✅ BLUE3-3.5-M10：`workflow.execute` 已支持角色感知执行分配（优先按节点角色匹配代理，失败回退轮转）
> - ✅ BLUE3-3.4-M11：`workflow.execute` 已支持基于 LearningBus 的失败策略自适应（`fail_fast` / `tolerant`）
> - ✅ BLUE3-3.6-M12：主控制面已接入 `auto_attach/auto_detach/optimization_modules` 治理策略，并统一输出结构化优化决策报告
> - ✅ BLUE3-3.6-M13：优化治理证据已独立落盘 `spec/latest-optimization-policy.json`（执行返回与 trace 均携带 artifact 路径）
> - ✅ BLUE3-3.6-M14：模块软卸载已补齐“原因 + 影响 + 恢复条件”三元审计信息，并进入治理报告与证据链
> - ✅ BLUE3-3.6-M15：基于治理历史已实现模块自动恢复（auto reattach）执行闭环，并将恢复行为与证据一并持久化
> - ✅ BLUE3-3.7-M16：主控制面已落地工作分级自动裁决（ask/edit/agent/safeguard/full_auto）与升级/降级原因审计，并落盘 `spec/latest-work-grade.json`
> - ✅ BLUE3-3.4-M17：LearningBus 已反哺工作分级策略（跨任务自适应升级/降级），执行事件新增 `gates_ok/work_grade/risk_score/runtime_healthy` 并用于下一次分级决策
> - ✅ BLUE3-3.8-M18：PUA/CLAUDE 重复文档已归并为索引页并收敛到权威规则源（`.github/copilot-instructions.md` + `RULES/*.md`）
> - ✅ BLUE3-3.8-M19：`.github/copilot-instructions.md` 已改为兼容 bootstrap，核心规则已合并精炼到 `RULES/global.md` / `RULES/coding.md` / `RULES/review.md` / `RULES/pua.md`，形成编辑器无关单一真相源（RULES-first）
> - ✅ BLUE3-3.3-M20：`workflow.execute` 已接入自治研究回合（Planner/Researcher/Reviewer），自动落盘 `spec/latest-research.json` 并将研究共识注入子任务执行上下文，形成“研究->执行”主链路闭环
> - ✅ BLUE3-3.5-M21：`workflow.execute` 已补齐“执行时自动生成并落盘 workflow DAG + 返回最终结论与 Gate 证据链”，形成“generate->assign->execute->conclusion”端到端闭环
> - ✅ BLUE3-3.1-M22：主链路已收敛统一指标证据源 `spec/latest-pipeline-metrics.json`（success_rate/risk/health/gates/work_grade/failure_strategy 等字段统一出自控制面），完成 3.1 的“接入后收敛”闭环
> - ✅ BLUE3-3.2-M23：并行执行硬指标已闭环落盘（`parallel_utilization` / `serial_degradation_count` / `parallel_failure_rollback_count`），并纳入执行结果与统一指标工件，完成 3.2 从“可并行”到“可量化并行”闭环
> - ✅ BLUE3-3.4-M24：LearningBus 已接入“路由成功率经验回归 + 代理 fallback 链按历史执行成功率重排”，并在执行返回/trace 暴露调优前后指标，完成 3.4 全局自学习闭环

## Positioning
- 本清单遵循 BLUE2 的方法论：**不做全仓形式统一**，聚焦主链路价值与可验证收益。
- 目标不是继续堆模块，而是完成“能力收敛”：
	- 把已存在但未接入的能力接入主链路；
	- 消除重复设计与平行真相源；
	- 让并行、自学习、自治编排在运行时真正生效并可度量。

## 1. 全项目扫描结论（冗余与不足）

### 1.1 冗余设计（高优先级）
- 多个优化模块仅存在于模块内部或测试调用，未进入核心执行链路：
	- `workflow_optimizer.rs`（`WorkflowOptimizer/ExecutionOptimizer/PredictiveFailureHandler`）
	- `adaptive_selector.rs`
	- `advanced_modules.rs`（`ContinuousLearner/ResourceAllocator/DynamicParameterTuner`）
	- `cost_optimizer.rs`
	- `speed_optimizer.rs`
	- `reliability_optimizer.rs`
	- `failure_prevention.rs`
- `task.execute` 已完成 phase 内并行，但仍需继续完善“关键路径优化 + 自学习并行参数调优”的全闭环。
- `roles.rs` 已明示“待集成到 orchestrator”，说明角色协作协议尚未真正作为执行协议落地。

### 1.2 自适应能力现状
- 已有真实闭环的自适应：向量检索精度反馈 + `AutoTuneState` 持久化（局部有效）。
- 已新增执行层自适应：`workflow.execute` 基于 `latest-learning.json` 对并行度做自动调参。
- 仍待完善全局自适应：路由成功率与模型策略尚未形成统一在线学习闭环（执行层并行度与失败策略已进入自适应）。

### 1.3 “最完美 agents proxy”达成度评估
- 并行执行：**部分（phase 内并行已落地，仍需关键路径优化与自学习调参）**
- 自学习：**部分（向量检索层）**
- AI 主动讨论研究方案：**部分（角色定义与路由有，未形成多代理研究会话闭环）**
- 自动生成工作流并分配任务：**部分（`workflow.generate/execute` 已落地，已支持 phase 级与角色感知代理分配，仍需更深的多角色协作协议执行）**
- 结论：当前为 **“可用增强态”**，尚非 **“终态自治态”**。

## 2. BLUE3 目标架构（单一控制面 + 双层执行面）

- **控制面（唯一真相源）**：`acp.rs` + `reinforcement.rs`
	- 负责任务分析、路由、策略决策、证据链、审计、降级。
- **执行面 A（子任务图执行器）**：基于 `TaskDecomposition.execution_phases` 的 DAG 调度器
	- 阶段内并行、阶段间顺序、失败可恢复。
- **执行面 B（角色协作执行器）**：`RoleSpecification/HandoffContract/RoleOutput`
	- 把 Planner/Researcher/Coder/Tester/Reviewer 从“静态定义”升级为“可执行协作协议”。
- **学习面（统一学习总线）**：收敛各模块反馈为统一 `LearningBus`
	- 输入：任务结果、失败原因、重试收益、并行收益、模型质量、向量精度。
	- 输出：动态调整路由阈值、并行度、模型策略、重试链。

## 3. 终态改进方案（最优精炼）

### 3.1 方案 A：主链路收敛（去冗余，不删能力）
- 原则：先“接入”后“删减”。
- 做法：
	- 将 `workflow_optimizer/adaptive_selector/advanced_modules/cost|speed|reliability|failure_prevention` 统一接到 `acp` pipeline 的可配置阶段钩子。
	- 未接入前禁止继续扩展新优化模块，防止设计漂移。
	- 对重复指标（success_rate/risk/health）统一字段与来源，避免多处各算各的。
	- 当前进展：控制面已在执行收口阶段统一落盘 `spec/latest-pipeline-metrics.json`，将 `predicted_success_rate/risk_score/runtime_healthy/gates_ok/work_grade/failure_strategy` 与并行执行结果统一到单一工件，作为主链路唯一指标证据源。

### 3.2 方案 B：并行执行落地（从“可并行”到“已并行”）
- 将 `TaskDecomposer.execution_phases` 作为运行时调度输入：
	- phase 内子任务用 `tokio::JoinSet` 并行执行；
	- phase 间 barrier；
	- 失败策略：快速失败 / 容错继续（按任务类型配置）；
	- 将每个并行分支结果回填 `PlannedSubtaskRecord`。
- 为并行执行增加硬指标：
	- 并行利用率、关键路径耗时、串行退化次数、并行失败回滚次数。
	- 当前进展：并行执行硬指标已接入主链路并持久化（`spec/latest-execution.json` + `spec/latest-pipeline-metrics.json`），执行返回与 trace 均可直接读取 `parallel_utilization`、`serial_degradation_count`、`parallel_failure_rollback_count`，实现“并行可观测 + 可回放 + 可审计”。

### 3.3 方案 C：自治研究讨论（AI 主动研讨）
- 新增“研究回合”编排：
	- Planner 产出问题树与验收标准；
	- Researcher 产出候选方案与风险矩阵；
	- Reviewer 给出采纳/拒绝理由；
	- 控制面自动选择最优方案进入编码。
- 讨论必须结构化落盘到 `.goon/spec/research-*.json`，进入证据链。
	- 当前进展：`workflow.research` 已支持结构化研讨产物落盘；`workflow.execute` 已支持自动触发 Planner/Researcher/Reviewer 研究回合，并将推荐方案共识注入后续子任务执行提示，形成“研究->执行”自治闭环。

### 3.4 方案 D：全局自学习闭环（不依赖人工判断）
- 建立 `LearningBus`（持久化 + 在线更新）：
	- 每次执行写入 `TaskOutcomeEvent`：复杂度、路由、并行度、模型、成功/失败、耗时、重试次数。
	- 周期性更新策略参数：
		- `TaskRouter` 的成功率估计从静态规则升级为“规则 + 经验回归”；
		- `ExecutionOptimizer` 并行阈值按真实收益动态调节；
		- fallback 链按历史恢复率重排。
- 禁止“黑箱自学习”：每次策略变更要有可解释变更记录。
	- 当前进展：执行事件已进入 `latest-learning.json`，形成可持续学习输入面；LearningBus 已驱动工作分级自适应输出；并新增 `adaptive_routing`（经验回归调优 `predicted_success_rate`）与 `adaptive_agent_order`（按历史执行成功率重排 fallback 链），且调优前后值已在执行返回与 trace 可审计暴露。

### 3.5 方案 E：自动工作流生成与任务分配
- 从 `task.plan` 升级为 `workflow.generate` + `workflow.execute`：
	- 自动输出 DAG（节点=子任务，边=依赖，属性=角色、预算、超时、重试）；
	- 自动分配执行者（角色/模型/模式）；
	- 自动触发 QA/Retest/Final Gate；
	- 自动输出最终结论与证据引用。
	- 当前进展：`workflow.generate` / `workflow.execute` 入口已落地；`workflow.execute` 默认自动触发 `QA -> Retest -> Final` Gate 并返回结构化报告；且执行时会自动生成并持久化 workflow DAG 证据，回传 `final_conclusion`（含 Gate 证据链与总结路径），形成端到端可回放闭环。

### 3.6 模块接入/卸载治理（主控制面唯一决策）
- 结论：**已有模块是否自动接入、是否卸载，必须由主控制面统一决策**，不允许模块自行激活或自行退场。
- 决策入口统一在控制面（`acp.rs` + 配置重载路径），策略来源为配置 + 健康检查 + Action Check + 运行指标。
- 执行规则：
	- `auto_attach = true` 时，仅允许进入“已注册策略钩子”的模块；
	- `auto_detach = true` 时，只允许主控制面依据 fail-open/fail-close 策略进行软卸载；
	- 任何模块卸载都必须写入审计事件与证据链（原因、影响、恢复条件）。
- 安全边界：
	- 禁止模块内部自我 `enable/disable` 影响主流程路由；
	- 禁止出现第二控制面（平行开关系统）。
	- 当前进展：`task.execute/workflow.execute` 已接入治理钩子，且优化治理报告可审计落盘，并覆盖模块卸载原因/影响/恢复条件；当满足连续健康窗口时可自动 reattach。

### 3.7 AI 审核与工作分级治理（主控制面裁决）
- 结论：**代码 AI 审核不强求双 AI**；是否启用双 AI/多 AI 审核，由主控制面按任务分级、风险和证据充分性自动决定。
- 审核策略：
	- 默认单 AI 审核 + 结构化 Gate（QA/Retest/Final）；
	- 仅在高风险场景触发双 AI（安全敏感、跨模块高复杂度、回归失败后复审）；
	- 低风险任务禁止机械触发双 AI，避免冗余成本与时延。
- 所有工作按分级执行（由主控制面自动判级）：
	- L0 `ask`：问答/解释，最小执行。
	- L1 `edit`：单点改动，轻量验证。
	- L2 `agent`：多步骤实现，标准验证与审计。
	- L3 `safeguard`：高风险任务，强制审批节点与增强验证。
	- L4 `full_auto`：端到端自治执行，自动编排、自动分配、自动 Gate、自动恢复。
- 分级升级/降级规则：
	- 失败累计、风险升高、证据不足时自动升级；
	- 连续稳定通过且风险降低时自动降级；
	- 任何升级/降级都必须写入审计记录（触发条件、决策依据、影响范围）。
	- 当前进展：`task.execute/workflow.execute` 已接入自动分级裁决，并对升级/降级动作与原因进行持久化审计。

### 3.8 PUA / CLAUDE / Hardening 推荐项治理（优化与冗余分类）
- 结论：后续新增的 PUA、CLAUDE Code、hardening 推荐项，必须先做“主链路价值判定”，再决定保留、合并或删除。

- 当前进展：`CLAUDE.md`、`README-PUA-UNIVERSAL.md`、`PUA-EMBEDDED.md`、`.github/PUA-ACTIVATION-COMPLETE.md` 已收敛为索引页；`.github/copilot-instructions.md` 已转为 RULES 引导入口；核心治理规则已统一沉淀到 `RULES/global.md`、`RULES/coding.md`、`RULES/review.md`、`RULES/pua.md`，并由 `RULES/README.md` 约束 RULES-first 治理模型。

#### A. 判定为优化项（应保留并持续演进）
- `src/pua.rs` 中与主执行链路直接耦合的能力：
	- `build_enforcement_plan`（风险分级、强制角色、证据义务）
	- `quality_compass`（可验证交付标准）
	- `review_gate_prompt`（审核门禁统一语义）
- `src/task_router.rs` 对 `pua_enforcement` 的消费与强制检查（路由阶段刚性约束）。
- `src/acp.rs` 对 safeguards / stage evidence / action check / runtime health 的执行与审计落盘。
- `src/hardening.rs` 中可直接用于运行时控制面的策略对象：
	- `PolicyBundle`（部署级策略）
	- `SandboxPolicy`（能力白名单/黑名单）
	- `Idempotency`（幂等保护）

#### B. 判定为冗余臃肿项（可删除或归档）
- 文档重复堆叠且语义重复的“PUA 宣传型文档”可归并，并统一归档到 `RULES/` 体系供全局复用：
	- `CLAUDE.md`（已声明以 `.github/copilot-instructions.md` 为权威源）
	- `README-PUA-UNIVERSAL.md`
	- `PUA-EMBEDDED.md`
	- `.github/PUA-ACTIVATION-COMPLETE.md`
	- 以及同类“完成报告/激活报告/快速参考”的重复副本
- 归档输出要求（跨项目可用）：
	- 规则沉淀到 `RULES/global.md`（保留与具体仓库无关的通用治理规则）。
	- 项目相关差异放到 `RULES/common.md` 或 `RULES/pua.md`，避免污染全局规则。
	- 被归档文档改为短索引页，只保留“权威规则入口”链接。
- `src/hardening.rs` 中若长期不被主控制面调用、仅停留在框架注释层的结构体（预算/队列/配额等）视为候选冗余，满足删除条件后可删除。
- `src/pua.rs` 中与当前治理冲突的“强制双审”表述（如复杂度触发固定 dual review）若未通过主控制面分级裁决，应改为“条件触发”或移除硬编码。

#### C. 冗余删除条件（必须全部满足）
- 条件 1：在主链路（`acp.rs` / `task_router.rs` / `flow.rs`）无调用，且连续两个版本无接入计划。
- 条件 2：存在等价或更高质量的单一权威实现（Single Source of Truth）。
- 条件 3：删除后不降低审计、回放、验收能力（QA/Retest/Final Gate 仍完整）。
- 条件 4：通过编译与回归验证，且关键指标无退化。

#### D. 删除执行顺序（先合并后删除）
- 第一步：定义权威源。
	- 规范类仅保留 `.github/copilot-instructions.md` + `RULES/global.md` 作为全局权威文本。
- 第二步：将重复文档提炼后归档到 `RULES/`（优先 `RULES/global.md`），原文改为“跳转索引页”。
- 第三步：删除无调用代码与重复文档。
- 第四步：运行 `cargo check --all` + 关键测试，生成删除审计记录。

## 4. 精炼策略（减法优先）

- 不追求“模块数更多”，追求“主链路更短、更稳、可解释”。
- 具体减法：
	- 合并重复优化器接口，统一为 `OptimizationPolicy`；
	- 合并重复学习结构，统一为 `LearningBus`；
	- 清理未接入但长期闲置的公共 API（先标记 deprecated，再移除）。
	- 所有模块开关收敛到主控制面配置，不再在模块内散落启停逻辑。
	- 对 PUA/CLAUDE/hardening 推荐项执行“价值证明制”：无主链路价值证明即不保留。

## 5. 分阶段执行计划（BLUE3 Sprint）

1. **S1（收敛）**
	 - 完成模块接入清单与调用路径梳理；
	 - 给每个“声明能力”补充“主链路入口”。
2. **S2（并行）**
	 - 上线 DAG 并行执行器；
	 - 产出并行性能基线与失败恢复策略。
3. **S3（自治研讨）**
	 - 上线结构化研究回合；
	 - 输出研究证据与方案选择理由。
4. **S4（自学习）**
	 - 上线 LearningBus；
	 - 打通策略在线更新与回放可解释性。
5. **S5（闭环验收）**
	 - 完成 workflow.generate/workflow.execute；
	 - Gate 全自动触发与报告闭环。

## 6. 验收标准（是否达到“最完美”）

- 自动性：复杂任务从输入到结论无需人工判断与切换。
- 并行性：至少 70% 的可并行子任务在运行时实际并行。
- 自学习：策略参数在连续窗口中出现可验证优化（成功率、耗时、重试成本）。
- 自治研讨：复杂任务必须有研究方案对比与采纳依据。
- 可审计：每次执行都可回放任务图、决策、失败、恢复、最终证据链。
- 分级治理：所有请求都能映射到 `ask/edit/agent/safeguard/full_auto` 之一，并可解释升级/降级原因。
- 审核治理：双 AI 审核触发率与误触发率可度量，且不对低风险任务强制开启。
- 冗余治理：PUA/CLAUDE/hardening 的重复文档与无调用代码已完成归并或删除，且不影响治理闭环。

## 7. 暂缓项与防过度设计

- 暂缓“全模块统一抽象重写”。
- 暂缓“为了形式统一而迁移全部错误模型”。
- 暂缓“新增第三套控制面状态机”。
- 原则：先打通运行闭环，再做结构美化。

## 8. 完成定义（BLUE3 Done）

- 不是“新增了多少模块”，而是以下结果全部成立：
	- 并行执行真实生效且有收益；
	- 自学习跨任务生效且可解释；
	- 自治研究讨论进入主链路；
	- 自动工作流生成与分配可端到端执行；
	- 主链路无重复控制面、无伪接入能力。

---

> BLUE3 的核心不是“再加功能”，而是把已声明能力全部压实到运行时闭环，最终实现真正的自适应自治 agents proxy。
