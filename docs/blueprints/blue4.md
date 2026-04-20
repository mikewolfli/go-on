# BLUE4: 需求澄清闭环与自治执行补强方案（规则同 BLUE3）

> **扫描结论（截至 2026-04-08）**
> - ✅ 已具备：任务分析、自动路由、自动拆解、阶段并行、执行证据链、LearningBus 分级与调优、Review Gate 组件
> - ⚠️ 未达闭环：缺少“主动澄清需求 -> 用户确认 -> 再执行”的刚性流程；Coding Review 在全入口仍非统一强制门禁
> - ⚠️ 关键差距：部分能力“存在”但“不是默认主链路硬约束”，端到端治理一致性仍需补强
>
> **完成度标识（本轮）**
> - ✅ BLUE4-0.1-M0：BLUE4 改进建议文档已创建，目标、里程碑、验收标准、回归策略已完整定义
> - ✅ BLUE4-3.1-M1：新增 `RequirementContractArtifact` 与 `GovernancePolicyArtifact`，并落盘 `spec/latest-clarification.json` / `spec/latest-governance-policy.json`
> - ✅ BLUE4-3.1-M2：新增控制面接口 `workflow.clarify`，可输出缺失字段、澄清问题与下一步确认指令
> - ✅ BLUE4-3.1-M3：新增控制面接口 `workflow.confirm`，支持结构化需求确认并持久化治理决策
> - ✅ BLUE4-3.2-M4：`task.plan` / `workflow.generate` 已接入需求确认门禁（复杂任务未确认则阻断并返回 `next_step`）
> - ✅ BLUE4-3.1-M5：`task.execute` / `workflow.execute` 已接入需求确认硬门禁（未确认不执行）
> - ✅ BLUE4-7.0-V1：本轮变更已通过 `cargo check --all` 与 `cargo test --all -- --nocapture` 验证（192 单测 + 14 集成测试）
> - ✅ BLUE4-3.3-M6：新增 `ExecutionDecisionArtifact` 并落盘 `spec/latest-execution-decision.json`，执行分配与并行策略具备可回放审计证据
> - ✅ BLUE4-3.3-M7：执行分配已升级为“历史顺序 + 角色匹配 + 负载扩散”联合评分机制，替代纯关键词回退
> - ✅ BLUE4-3.3-M8：并行决策已审计化并接入依赖约束（phase 内存在依赖时强制串行），决策原因进入 artifact 与执行返回
> - ✅ BLUE4-7.0-V2：S3 本轮变更已完成编译与全量测试复核（`cargo check --all` + `cargo test --all -- --nocapture`）
> - ✅ BLUE4-3.4-M9：已落地统一 `ReviewPolicy` 并接入 `chat` / `task.execute` / `workflow.execute` 三入口（评审级别、required_checks、timeout_policy 统一治理）
> - ✅ BLUE4-7.0-V3：S4 本轮变更已完成编译与全量测试复核（`cargo check --all` + `cargo test --all -- --nocapture`）
> - ✅ BLUE4-3.5-M10：LearningBus 事件已扩展并接入 `clarification_rounds` / `clarification_quality_score` / `requirement_change_count` / `review_reject_root_cause`
> - ✅ BLUE4-3.5-M11：新增 task/workflow 端到端集成测试矩阵，覆盖“未确认阻断”与“确认后执行+统一 review_policy + LearningBus 字段回写”
> - ✅ BLUE4-3.5-M12：完成发布前验收闭环（代码改造 + 回归验证 + BLUE4 完成度回写）
> - ✅ BLUE4-7.0-V4：S5 本轮变更已完成编译与全量测试复核（`cargo check --all` + `cargo test --all -- --nocapture`）
> - ✅ BLUE4-3.5-M12+：新增 `learning.summary` 统一聚合接口，将 LearningBus 新字段转为治理可观测指标（rounds/quality/change_count/reject_root_cause/rates）
> - ✅ BLUE4-7.0-V5：S5 收口增强已完成编译与全量测试复核（`cargo check --all` + `cargo test --all -- --nocapture`）
> - ✅ BLUE4-BLUE5-LINK-V1：已完成 BLUE5 M13-M19 联动回写：`workflow.consult`、自动会诊触发门禁、`latest-consultation.json`、`latest-clarification-session.json`、多轮澄清 `ready_to_confirm` 门禁与端到端验证（194 单测 + 22 集成测试）
> - ✅ BLUE4-BLUE5-LINK-V2：已完成 BLUE5-0.7-M20 联动回写：补齐 `primary_failover_report` 响应语义与 `spec/latest-primary-secondary-failover.json` 独立工件，`task.execute/workflow.execute` trace 与结果同步暴露 failover 路径与摘要，并通过回归验证

## Positioning
- 本清单遵循 BLUE3 的方法论：一次推进一个大项闭环，不做无验证的功能堆叠。
- BLUE4 目标：将现有“可用增强态”升级为“可证明闭环态”，重点补齐以下四项：
  - AI 主动与用户沟通需求
  - 明确需求后自动分解任务
  - 按可用 AI 与任务特征自动串并行执行
  - 自学习分级 + Coding Review 全链路刚性治理

## 1. BLUE4 目标定义（对齐 4 条要求）

### 1.1 目标 A：主动需求沟通闭环（Requirement Clarification Loop）
- 要求：当输入存在歧义、范围不完整、验收标准缺失时，系统必须先发起澄清，不得直接进入执行。
- 闭环状态机：
  - `intake` -> `ambiguity_detected` -> `clarification_asked` -> `user_confirmed` -> `plan_generated` -> `execution`。
- 最低输出要求：
  - 目标定义（Goal）
  - 边界与非目标（Scope / Non-goals）
  - 验收标准（Acceptance）
  - 风险与约束（Risk / Constraints）

### 1.2 目标 B：确认后自动分解任务（Auto Decomposition on Confirmed Intent）
- 要求：仅在 `user_confirmed` 后触发任务分解，防止“误分解/误执行”。
- 分解产物必须结构化：
  - 子任务 DAG
  - 依赖关系
  - 阶段并发宽度建议
  - 子任务验证义务（测试/回归/证据）

### 1.3 目标 C：按可用 AI 自动分配并串并行执行（Adaptive Assignment + Serial/Parallel）
- 要求：在 env-ready agent 集合内按任务特征动态分配，自动选择串行/并行。
- 路由决策必须显式可审计：
  - 为什么选该 agent
  - 为什么该 phase 并行或串行
  - 失败后如何降级（degrade / multi_ai / fail_fast / tolerant）

### 1.4 目标 D：自学习分级 + Coding Review（Learning Governance + Review Gate）
- 要求：工作分级与审核策略可学习、可解释、可回放。
- 核心补强：
  - Coding Review 从“能力项”升级为“全入口一致门禁策略”
  - 任何入口（chat/task/workflow）都应满足统一最低审核要求

## 2. 现状与缺口（相对 BLUE4）

### 2.1 已有基础能力（可复用）
- 任务分析/路由：`TaskRouter::analyze_task` + `TaskRouter::route_task`
- 自动分解：`build_task_plan` + `TaskDecomposer::decompose`
- 执行编排：`task.execute` / `workflow.execute`（phase 分组并行、失败策略）
- 自学习：LearningBus（success rate、parallelism、work grade、agent order）
- 审核组件：Review Gate + QA/Retest/Final Action Checks

### 2.2 缺口清单（必须补齐）
1. 缺少刚性“需求澄清回合”状态机与确认门禁。
2. 分解与执行尚可绕过“确认后执行”的前置条件。
3. agent 分配存在名称关键词启发式回退，鲁棒性不足。
4. Coding Review 在不同入口存在策略不一致，尚未形成统一硬门禁。
5. 关键新策略缺少覆盖 `task.execute/workflow.execute` 的端到端回归测试矩阵。

## 3. BLUE4 终态方案（完整改进建议）

### 3.1 方案 A：需求澄清协议化（必须先问清）
- 在控制面新增统一澄清对象 `RequirementContract`：
  - `goal`
  - `scope`
  - `non_goals`
  - `acceptance_criteria`
  - `constraints`
  - `user_confirmed`（bool）
- 新增控制面入口：
  - `workflow.clarify`
  - `workflow.confirm`
- 行为约束：
  - 当复杂度 >= 3 或风险任务且 `user_confirmed=false` 时，阻止 `task.execute/workflow.execute`。
  - 错误返回必须包含 `next_step`，指导用户补充信息并确认。

### 3.2 方案 B：确认驱动的规划与拆解
- `task.plan/workflow.generate` 引入 `RequirementContract` 作为必选输入之一。
- 只允许在 `user_confirmed=true` 时写入正式 plan artifact。
- 将“澄清摘要”注入后续子任务上下文，避免执行阶段目标漂移。

### 3.3 方案 C：分配与并行决策升级
- 从“关键词匹配优先”升级为“能力向量 + 历史成功率 + phase 负载”联合评分。
- 统一执行决策对象 `ExecutionDecisionArtifact`：
  - `selected_agents`
  - `assignment_reason`
  - `parallelism`
  - `failure_strategy`
  - `degrade_policy`
- 对并行策略增加强约束：
  - 存在依赖链必须 barrier
  - 仅独立子任务可并行
  - 降级时强制并行度收敛到 1

### 3.4 方案 D：Coding Review 全入口统一门禁
- 建立统一审核策略 `ReviewPolicy`（全入口共享）：
  - `min_review_level`
  - `required_reviews`
  - `required_checks`（QA/Retest/Final）
  - `timeout_policy`
- 对 `chat`、`task.execute`、`workflow.execute` 统一生效，禁止入口策略漂移。
- 默认策略：
  - L0/L1 可单审 + 必要 gate
  - L2/L3/L4 强制增强审核（条件触发双审）

### 3.5 方案 E：自学习治理与可解释回放
- 扩展 LearningBus 事件：
  - `clarification_rounds`
  - `clarification_quality_score`
  - `requirement_change_count`
  - `review_reject_root_cause`
- 新增策略工件：
  - `spec/latest-governance-policy.json`
  - `spec/latest-clarification.json`
- 每次策略变化必须输出“变更原因 + 影响范围 + 回滚条件”。

## 4. 分阶段执行计划（BLUE4 Sprint）

1. S1（澄清门禁）
- 落地 `RequirementContract` 与 `workflow.clarify/workflow.confirm`。
- 对执行入口加前置校验：未确认不得执行。

2. S2（确认后分解）
- 让 `task.plan/workflow.generate` 强依赖确认态。
- 产出 `clarification -> plan` 证据链。

3. S3（分配与并行升级）
- 实现能力评分分配器，替换单一关键词启发。
- 输出 `ExecutionDecisionArtifact`，打通审计。

4. S4（统一 Coding Review）
- 把审核策略下沉为全入口统一策略。
- 统一 gate 与 review 计数规则。

5. S5（学习闭环与回归）
- LearningBus 接入澄清质量与审核拒绝根因。
- 补齐端到端测试矩阵并设为发布门禁。

## 5. 里程碑（M1-M12）

- M1：新增 `RequirementContract` 数据结构与持久化。
- M2：新增 `workflow.clarify` 接口，输出结构化澄清问题。
- M3：新增 `workflow.confirm` 接口，生成确认快照。
- M4：执行入口接入“未确认阻断”硬门禁。
- M5：`task.plan/workflow.generate` 切换到确认驱动。
- M6：新增 `ExecutionDecisionArtifact` 并落盘。
- M7：agent 分配升级为能力评分机制。
- M8：并行决策与依赖约束审计化。
- M9：统一 `ReviewPolicy` 并接入全部入口。
- M10：LearningBus 扩展澄清与审核反馈字段。
- M11：新增端到端集成测试（task/workflow 全路径）。
- M12：发布前验收通过并更新 BLUE4 完成度。

## 6. 验收标准（Definition of Done）

- 主动沟通：复杂/歧义任务 100% 先进入澄清回合。
- 确认后执行：未确认任务 100% 被执行门禁阻断。
- 自动分解：确认后任务 100% 产出结构化分解与依赖图。
- 自动分配：所有子任务都有可解释分配原因与回退策略。
- 串并行治理：并行决策可审计，依赖违规为 0。
- Coding Review：三个入口策略一致且门禁可证明生效。
- 自学习：策略调优有可解释记录且可回放。
- 测试保障：新增测试覆盖关键治理路径，回归通过后方可发布。

## 7. 测试与发布门禁

- 编译门禁：`cargo check --all`
- 单测门禁：`cargo test --all -- --nocapture`
- 关键集成测试门禁：
  - 澄清后确认才能执行
  - 超能力上限决策分支（multi_ai / degrade / warn_only）
  - 全入口统一 review 策略生效
- 工件门禁：必须生成并可读取以下工件：
  - `spec/latest-clarification.json`
  - `spec/latest-plan.json`
  - `spec/latest-workflow.json`
  - `spec/latest-execution.json`
  - `spec/latest-governance-policy.json`
  - `spec/latest-learning.json`

## 8. 暂缓项与防过度设计

- 暂缓新增第四套策略引擎，避免第二控制面。
- 暂缓仅为“看起来更智能”而引入不可解释模型路由。
- 暂缓无证据价值的新文档堆叠，所有规则继续 RULES-first。

## 9. 完成定义（BLUE4 Done）

- 不是“又加了多少模块”，而是四条目标全部可证明：
  - 会主动澄清需求
  - 确认后自动分解
  - 按可用 AI 自动串并行执行
  - 自学习分级 + Coding Review 全入口一致生效

---

> BLUE4 的核心是把“能力存在”提升为“治理硬约束存在”，让自治执行从增强态进入可证明闭环态。
