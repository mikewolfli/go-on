# BLUE5: 多AI主从协议硬约束与节点级编排闭环（规则同 BLUE4）

> **扫描结论（截至 2026-04-08）**
> - ✅ 已具备：多AI候选评分、节点级自动选执行者、执行决策审计（candidate_scores / selected_agent）
> - ✅ 已闭环：主从协议（`primary_agent` / `secondary_agents`）统一输出、硬校验与 failover 策略已落地
> - ✅ 已补齐：自动会诊触发、会诊工件、会诊后回切单主执行、需求多轮协同澄清与确认门禁
>
> **完成度标识（本轮）**
> - ✅ BLUE5-0.1-M0：BLUE5 改进建议文档已创建，目标、里程碑、验收标准、回归策略已完整定义
> - ✅ BLUE5-0.2-M1：已新增 `PrimarySecondaryPolicy` 数据结构与硬校验（主唯一、主必须在 env-ready 集合）
> - ✅ BLUE5-0.2-M2：`task.execute/workflow.execute` 返回与 trace 已接入 `primary_secondary_policy`
> - ✅ BLUE5-0.2-M3：已接入 `primary_failover_policy` 与 `secondary_max_count` 配置解析（默认 `first_secondary`）
> - ✅ BLUE5-0.3-M4：`ExecutionAssignmentRecord` 已扩展 `node_primary_agent / node_secondary_agents / effective_executor / failover_applied / failover_reason` 五个节点级字段
> - ✅ BLUE5-0.3-M5：`first_secondary` 接管链路已落地（主失败后自动接管第一从，并写入 audit record）
> - ✅ BLUE5-0.3-M6：`score_based_secondary` 接管链路已落地（按评分顺序依次尝试，首次成功即接管）
> - ✅ BLUE5-0.3-M7：`abort` 策略已落地（主失败直接置 failover_reason，不重试）
> - ✅ BLUE5-0.3-M8：`PrimarySecondaryPolicyArtifact` 已新增并在每次执行后落盘 `spec/latest-primary-secondary-policy.json`
> - ✅ BLUE5-0.4-M9：LearningBus 已扩展 `primary_stability_score / secondary_utilization_rate / failover_count / failover_root_cause` 四个主从稳定性字段
> - ✅ BLUE5-0.4-M10：`primary_secondary.summary` 聚合接口已落地，输出主稳定度、接管率、从利用率与接管根因画像
> - ✅ BLUE5-0.4-M11：新增两条端到端集成测试（`rpc_primary_secondary_policy_artifact_is_persisted_and_response_contains_policy` / `rpc_primary_secondary_summary_reports_stability_and_failover_metrics`）；194 单测 + 19 集成测试全绿
> - ✅ BLUE5-0.5-M12：发布前验收通过并完成完成度回写（`cargo check --all` + `cargo test --all -- --nocapture`）
> - ✅ BLUE5-0.5-M13：新增会诊触发门禁（`consultation_required` 显式触发 + 失败阈值/低质量自动触发）
> - ✅ BLUE5-0.5-M14：新增 `ConsultationArtifact` 并落盘 `spec/latest-consultation.json`
> - ✅ BLUE5-0.5-M15：会诊后“回切唯一主执行”闭环已落地（共识注入后仍由 `primary_agent` 主执行）
> - ✅ BLUE5-0.6-M16：新增会诊端到端集成测试（显式 `workflow.consult` 与自动触发阻断分支）
> - ✅ BLUE5-0.6-M17：新增 `ClarificationSessionArtifact` 并落盘 `spec/latest-clarification-session.json`
> - ✅ BLUE5-0.6-M18：`workflow.clarify/workflow.confirm` 已支持多轮 `round_index` 协同澄清与 `ready_to_confirm` 门禁
> - ✅ BLUE5-0.6-M19：新增需求协同端到端测试（多轮讨论、确认阻断、ready 后确认通过）；194 单测 + 22 集成测试全绿
> - ✅ BLUE5-0.7-M20：已补齐 `primary_failover_report` 统一返回语义，并新增独立工件 `spec/latest-primary-secondary-failover.json`（执行结果/trace 同步暴露路径与摘要）

## Positioning
- 本清单遵循 BLUE4 的方法论：一次推进一个大项闭环，不做无验证的能力堆叠。
- BLUE5 目标：把“多AI可用”升级为“多AI主从可证明治理”，重点补齐以下四项：
  - 自动确认唯一主AI（Primary）
  - 自动确认多个从AI（Secondary）
  - 任务节点级主从编排（node-level）
  - 主从决策可审计、可回放、可降级
  - 疑难问题/需求不明确时自动触发多模型会诊
  - 需求阶段支持多轮用户讨论与多AI协同澄清

## 1. BLUE5 目标定义（对齐主从诉求）

### 1.1 目标 A：全局唯一主（Global Single Primary）
- 要求：每次 `task.execute/workflow.execute` 必须存在且仅存在一个 `primary_agent`。
- 约束：
  - 主AI必须来自 env-ready agent 集合；
  - 禁止出现主AI为空、多个主AI、主AI不在候选集合内；
  - 主AI选择理由必须可审计（评分、风险、能力上限决策）。

### 1.2 目标 B：多个从（Secondary Set）
- 要求：`secondary_agents` 可为 0..N，但当候选大于1时默认至少保留1个从。
- 约束：
  - 从AI不得包含主AI；
  - 从AI集合需稳定排序（可复现）；
  - 支持上限配置（如 `secondary_max_count`）。

### 1.3 目标 C：任务节点主从（Node-level Primary/Secondary）
- 要求：每个任务节点必须有 `node_primary_agent`，并可选 `node_secondary_agents`。
- 约束：
  - 节点主AI只能一个；
  - 节点从AI可多个；
  - 若节点依赖阻塞或能力降级，必须记录主从变更原因（重排/替补/降级串行）。

### 1.4 目标 D：主从治理与降级（Governance + Degrade）
- 要求：主AI失败或不可用时，按从AI顺位自动接管并输出治理证据。
- 约束：
  - 接管路径可配置：`primary_failover_policy = first_secondary | score_based_secondary | abort`；
  - 能力上限触发 `degrade` 时，主从关系仍保持“唯一主”不变；
  - 所有接管事件写入 artifact 与 trace。

### 1.5 目标 E：疑难与不明确需求会诊（Consultation Trigger）
- 要求：当 AI 遇到无法解决问题，或用户需求特别不明确时，自动进入多模型会诊。
- 触发条件（满足任一即可）：
  - 连续失败达到阈值（如 `consultation_failure_threshold`）；
  - 需求澄清后仍存在关键字段缺失或冲突；
  - 主AI输出包含“无法确定/证据不足/冲突建议”等不可执行信号。
- 约束：
  - 会诊必须输出统一结论（推荐方案 + 风险 + 放弃理由）；
  - 会诊结束后仍需回到“唯一主执行”治理模型；
  - 会诊过程与结论必须可审计并可回放。

### 1.6 目标 F：需求阶段多轮讨论与多AI协同（Requirement Co-Discussion）
- 要求：在 `workflow.clarify/workflow.confirm` 阶段，允许 AI 与用户进行多轮讨论，并引入多AI协同输出澄清问题与备选需求表述。
- 触发条件（满足任一即可）：
  - 需求关键字段缺失（goal/scope/acceptance/constraints）；
  - 用户表达存在冲突或跨模块高耦合；
  - 单AI澄清回合超过阈值仍无法确认。
- 约束：
  - 每一轮必须产出“本轮结论 + 仍缺失项 + 下一轮问题”；
  - 多AI协同必须有主澄清AI（唯一）与辅助澄清AI（多个）；
  - 未完成确认前禁止进入正式 plan/execute。

## 2. 现状与缺口（相对 BLUE5）

### 2.1 已有基础能力（可复用）
- 候选评分：`rank_execution_agents` 已输出候选分与理由。
- 节点选择：`ExecutionAssignmentRecord.selected_agent` 已体现节点实际执行者。
- 执行审计：`ExecutionDecisionArtifact` 已可落盘并回放主要分配事实。

### 2.2 缺口清单（必须补齐）
1. 缺少统一主从字段（全局与节点）与协议版本。
2. 缺少“主只能一个”的输入/输出硬校验与错误码规范。
3. 缺少主失败->从接管策略配置与统一返回语义。
4. 缺少针对主从治理的端到端测试矩阵。
5. 缺少主从聚合观测（接管率、主稳定度、从利用率）。
6. 缺少“疑难/需求不明确 -> 自动会诊”的触发门禁与统一结论结构。
7. 缺少需求阶段“多轮讨论 + 多AI协同澄清”的结构化会话工件与回放能力。

## 3. BLUE5 终态方案（完整改进建议）

### 3.1 方案 A：主从协议对象化
- 新增统一协议对象 `PrimarySecondaryPolicy`：
  - `primary_agent: String`
  - `secondary_agents: Vec<String>`
  - `policy_version: String`
  - `failover_policy: String`
  - `secondary_max_count: usize`
- 在 `task.execute/workflow.execute` 返回与 trace 中统一暴露该对象。

### 3.2 方案 B：节点主从结构化
- 扩展节点审计对象 `ExecutionAssignmentRecord`：
  - `node_primary_agent`
  - `node_secondary_agents`
  - `effective_executor`
  - `failover_applied`
  - `failover_reason`
- 保证每个节点可直接回放“主从决策 -> 实际执行”。

### 3.3 方案 C：接管策略硬门禁
- 主不可用或失败时执行策略：
  - `first_secondary`：按从队列首位接管；
  - `score_based_secondary`：按最新评分重排后接管；
  - `abort`：直接失败并输出下一步建议。
- 所有分支统一输出 `primary_failover_report`。

### 3.4 方案 D：主从稳定性学习闭环
- 扩展 LearningBus 字段：
  - `primary_stability_score`
  - `secondary_utilization_rate`
  - `failover_count`
  - `failover_root_cause`
- 新增聚合接口：
  - `primary_secondary.summary`（按窗口输出主稳定度与接管画像）。

### 3.5 方案 E：可观测与审计工件
- 新增工件：
  - `spec/latest-primary-secondary-policy.json`
  - `spec/latest-primary-secondary-failover.json`
- 与现有 `latest-execution-decision.json` 形成双向引用。

### 3.6 方案 F：多模型会诊编排（Consultation Workflow）
- 新增会诊入口：
  - `workflow.consult`（显式调用）
  - `task.execute/workflow.execute` 内部自动触发（隐式调用）
- 会诊角色建议：
  - `consult_lead`（主裁决）
  - `consult_specialist_*`（可多个）
  - `consult_reviewer`（一致性审阅）
- 会诊结果对象 `ConsultationArtifact`：
  - `trigger_reason`
  - `participants`
  - `candidate_plans`
  - `consensus_plan`
  - `risk_matrix`
  - `decision_confidence`
  - `handoff_primary_agent`
- 会诊后执行规则：
  - 若共识达成：回切单主执行并保留从队列；
  - 若共识未达成：返回结构化阻断与下一步澄清问题。

### 3.7 方案 G：需求多轮协同协议（Clarification Session）
- 新增会话对象 `ClarificationSessionArtifact`：
  - `session_id`
  - `round_index`
  - `lead_clarifier`
  - `assistant_clarifiers`
  - `user_feedback`
  - `resolved_points`
  - `open_points`
  - `next_questions`
  - `ready_to_confirm`
- 控制面行为：
  - `workflow.clarify` 支持 `round_index` 自增与历史上下文回放；
  - `workflow.confirm` 仅在 `ready_to_confirm=true` 且关键字段齐备时通过；
  - 允许 `clarify_collaboration_mode = single_ai | multi_ai` 配置切换。
- 产出要求：
  - 每轮都要生成结构化摘要，且可由用户确认/修订；
  - 多AI协同结果必须归并为单一对用户可执行的确认草案。

## 4. 分阶段执行计划（BLUE5 Sprint）

1. S1（协议定义）
- 新增主从策略对象与返回结构，建立唯一主校验。

2. S2（节点落地）
- 节点级主从字段接入执行与审计。

3. S3（接管策略）
- 落地主失败->从接管三种策略及错误语义。

4. S4（学习与观测）
- LearningBus 接入主从稳定性指标并新增 summary 接口。

5. S5（回归与发布）
- 完整端到端测试矩阵 + 发布门禁 + 完成度回写。

6. S6（会诊治理）
- 落地自动会诊触发、会诊工件、会诊后回切单主执行。

7. S7（需求协同澄清）
- 落地需求阶段多轮讨论状态机与多AI协同澄清工件。

## 5. 里程碑（M1-M12）

- M1：新增 `PrimarySecondaryPolicy` 数据结构与校验。
- M2：`task.execute/workflow.execute` 返回主从策略对象。
- M3：新增 `primary_failover_policy` 配置项与默认策略。
- M4：节点审计对象扩展 `node_primary_agent/node_secondary_agents`。
- M5：主失败接管链路落地（first_secondary）。
- M6：主失败接管链路落地（score_based_secondary）。
- M7：`abort` 策略与错误数据结构落地。
- M8：新增主从治理工件并落盘。
- M9：LearningBus 接入主从稳定性字段。
- M10：新增 `primary_secondary.summary` 聚合接口。
- M11：新增端到端集成测试（主唯一/从多/节点接管）。
- M12：发布前验收通过并更新 BLUE5 完成度。
- M13：新增会诊触发门禁（失败阈值/需求不明确/证据不足）。
- M14：新增 `ConsultationArtifact` 并落盘。
- M15：会诊后“回切唯一主执行”闭环落地。
- M16：新增会诊端到端测试（自动触发/共识成功/共识失败）。
- M17：新增 `ClarificationSessionArtifact` 并落盘。
- M18：`workflow.clarify/workflow.confirm` 支持多轮协同澄清与回放。
- M19：新增需求协同端到端测试（多轮讨论/多AI协同/确认门禁）。

## 6. 验收标准（Definition of Done）

- 唯一主：每次执行 `primary_agent` 恰好 1 个（100%）。
- 多从：存在候选时 `secondary_agents` 可用且不含主（100%）。
- 节点治理：每个节点均有主从决策证据（100%）。
- 接管闭环：主失败接管路径可复现且有根因记录（100%）。
- 可观测：主稳定度、接管率、从利用率可查询可回放（100%）。
- 会诊闭环：疑难/不明确场景可自动会诊并输出统一结论（100%）。
- 需求闭环：需求阶段可多轮讨论且多AI协同澄清，最终形成可确认草案（100%）。
- 测试保障：新增测试覆盖关键分支并全绿后方可发布。

## 7. 测试与发布门禁

- 编译门禁：`cargo check --all`
- 单测门禁：`cargo test --all -- --nocapture`
- 关键集成测试门禁：
  - 全局唯一主校验失败分支
  - 节点级主从字段完整性
  - 三类 failover 策略行为一致性
  - 主从聚合 summary 数值正确性
  - 自动会诊触发与回切单主执行行为正确性
  - 会诊共识失败时的结构化阻断语义一致性
  - 需求阶段多轮讨论状态机正确性
  - 多AI协同澄清归并结果一致性与确认门禁正确性
- 工件门禁：
  - `spec/latest-primary-secondary-policy.json`
  - `spec/latest-primary-secondary-failover.json`
  - `spec/latest-execution-decision.json`
  - `spec/latest-consultation.json`
  - `spec/latest-clarification-session.json`

## 8. 暂缓项与防过度设计

- 暂缓引入“多主并行写入”模式，防止责任边界失焦。
- 暂缓将主从策略拆分为独立控制面，避免第二治理平面。
- 暂缓仅展示型指标，所有指标必须可追溯到执行事实。
- 暂缓把会诊直接绑定固定模型厂商，优先保留提供商无关编排能力。
- 暂缓把需求讨论做成无限回合，必须保留最大轮次与升级策略防止对话失控。

## 9. 完成定义（BLUE5 Done）

- 不是“支持了多AI”，而是主从治理四条全部可证明：
  - 主只能一个（硬约束）
  - 从可以多个（可配置）
  - 任务节点主从清晰（可回放）
  - 主失败接管闭环（可审计、可学习）
  - 疑难/不明确需求自动会诊并回切单主执行（可治理）
  - 需求阶段可多轮讨论且多AI协同澄清（可确认、可阻断、可回放）

---

> BLUE5 的核心是把“候选排序”提升为“主从治理协议”，让多AI执行从可用态进入可证明控制态。
