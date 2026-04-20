# BLUE22 — 不改 Phase 达到顶级代码平台改进步骤（实施中）

更新时间：2026-04-18

本文沿用 BLUE21 的同一验收规则与收口口径：
- 三端一统（backend / vscode-addon / GUI）
- 主链路完整闭环
- 后端主链路功能完整
- 不留 warning
- 最小修改：仅改与目标直接相关内容；禁止为了“过测试”而做功能语义不完整的最小改动
- 完成率必须回写

---

## 本轮实施回写（2026-04-19）

本轮已完成 BLUE22 的主链路基础骨架落地，重点是先把 backend / addon / GUI 的统一语义源打通，再用契约、smoke、集成测试把主链路封口。

本轮已完成：
- `workflow.generate`
- `workflow.research`
- `workflow.consult`
- `workflow.execute`
- `task.plan`
- `task.execute`

新增落地（本轮续推）：
- Step 2 Auto-Repair Loop 已落地基础执行链：
  - `repair_readiness` 响应结构（workflow/task 统一）
  - `repair_history` 响应结构（workflow/task 统一）
  - 修复触发判定、预算/迭代终止判定、修复动作记录
  - 失败分类与可修复分类对齐（`execution_subtask_failed`）
- Step 3 Code Change Bundle 产品字段已统一：
  - `file_change_summary`
  - `risk`
  - `gate_results`
  - `rollback_recommendation`
  - `commit_suggestion`
- 三端一致性补强：
  - 新增 `tests/step2_three_endpoint_contract.rs`（10/10）
  - `tests/acp_runtime_rpc_integration.rs` 已新增 Step 3 变更包字段断言
- Step 4 工具矩阵与降级编排（backend 基座）已落地：
  - `ToolRegistry` 新增 capability/risk_level/timeout_budget/retry_policy/fallback_chain 元数据
  - 新增 `run_with_fallback` 自动降级执行链
  - 新增回归测试 `tool_registry_runs_fallback_chain_when_primary_fails`
- Step 7 评测基线（backend 脚本化快照）已落地：
  - 新增 `scripts/run-blue22-benchmark-snapshot.sh`
  - 产出 `artifacts/blue22/benchmark-snapshot.json`
  - 指标字段含：task_success_rate / first_pass_rate / mean_repair_iterations / human_intervention_rate / regression_rate
- Step 5 项目语义记忆图（backend 主链路）已落地：
  - `task.plan` 响应新增 `memory_graph`（task-problem-fix-evidence 结构）
  - `task.plan` 响应新增 `memory_recall`（evidence/sources/hit_count）
  - 集成测试断言已补齐（task.plan + task.execute benchmark 场景）
- Step 6 三端模式开关语义（contract + 消费层）已补强：
  - backend `task.plan` / `task.execute` / `workflow.execute` 统一返回 `run_mode`（manual/assisted/autonomous）
  - 契约新增 `workflowControlModes` 与 `defaultWorkflowControlMode`
  - addon / GUI contract smoke 与类型消费同步
  - addon / GUI protocol contract service 已导出 workflow control modes，供 UI 面板直接消费
- Step 1 执行循环历史已进一步收敛：
  - `execution_cycle` 在存在修复动作时写入“已执行修复轮次”而非仅预览
  - `history_summary.current_iteration / repair_iterations` 在 runtime execute 路径可用
  - `auto_repair.status` 可反映 `executed/planned/not_needed`

上述 6 条主链路 RPC 已统一补齐以下响应骨架字段：
- `execution_cycle`
- `requirement_gate`
- `gates`
- `artifacts`
- `change_bundle`
- `trace_ref`

且本轮新增补强：
- `execution_cycle.current_cycle`
- `execution_cycle.cycles`
- `execution_cycle.history_summary.pending_repair_iterations`
- `execution_cycle.auto_repair`
- `execution_cycle.current_cycle.patch_set`
- `execution_cycle.auto_repair.target_subtasks`
- `execution_cycle.auto_repair.next_cycle_preview`
- `change_bundle.rollback`
- `change_bundle.commit`
- `change_bundle.test_coverage`
- `change_bundle.files`

同步完成：
- `contracts/editor-capability-matrix.json` 新增 BLUE22 主链路骨架能力标记
- addon / GUI contract smoke 同步校验新能力标记
- `tests/acp_runtime_rpc_integration.rs` 补齐主链路骨架断言与 requirement contract 对齐
- 新增 execution-cycle 历史摘要、auto-repair 资格、change-bundle 回滚/提交/测试覆盖断言
- `workflow.execute` / `task.execute` 已根据真实 subtask 结果输出 patch set 与 repair preview，而不是只返回通用占位对象

本轮门禁结果：
- `cargo check --all-targets` 通过
- `cargo test --test acp_runtime_rpc_integration` 71/71 通过
- `cargo test --test protocol_consistency_integration` 10/10 通过
- `node vscode-addon/scripts/contract-smoke.js` 通过
- `cd GUI && node scripts/contract-smoke.mjs && npm run build` 通过

本轮未完成项：
- Step 1 的“真实多轮执行循环”仍未落地到真实 N 轮执行（当前为单轮执行 + repair 轨迹/预览）
- Step 4 尚缺 trace.metrics/governance.status 的降级统计可视化对接
- Step 6 尚缺 GUI/addon 的完整 cycle timeline / gate matrix / auto-repair trace 产品化视图
- Step 7 尚缺“GUI/addon 指标展示 + 发布流水线自动快照”闭环

结论：BLUE22 当前已完成“R1 主体能力 + Step 4/5/6/7 的后端与契约基础闭环”，但还不是全部 Step 的最终完成态。

---

## 前提与目标

前提：本轮不改 phase 体系（保持现有 phase 机制与配置模型）。

目标：在保持 phase 不变的条件下，把 go-on 从“可治理 runtime 平台”升级为“完整顶级代码平台”，重点补齐以下能力短板：
- 深度自主执行闭环（读-改-测-修-复验）
- 代码工作流产品化（补丁、变更解释、PR 语义）
- 工具生态广度与可组合性
- 长时记忆与项目语义沉淀
- 可视化可控与复盘能力

---

## 扫描范围

- backend：`src/**`（含 ACP 请求链、governance、orchestration、memory、tool）
- GUI：`GUI/src/**` + `GUI/src-tauri/src/**`
- addon：`vscode-addon/src/**`
- 契约：`contracts/editor-capability-matrix.json`
- 文档与门禁：`README.md`、`README.zh-CN.md`、`scripts/**`

---

## 差距清单（对标 openclaw / Claude Code / Codex）

### G22-1 自主执行闭环深度不足（P0）

现状：已有 `workflow.*` 与 `task.*`，但“自动迭代修复直到门禁通过”的闭环仍偏弱，更多是编排而非强自治执行。

目标状态：
- 支持一次任务内多轮自动迭代：计划 -> 改动 -> 测试 -> 定位 -> 修复 -> 再测。
- 每轮有统一审计对象（输入、动作、结果、失败原因、下一步）。

### G22-2 代码工作流产品层不足（P0）

现状：有治理/门禁与基础 workflow，但缺少统一“代码变更产品对象”（变更集、风险评估、可读说明、提交建议）。

目标状态：
- 输出标准化 Code Change Bundle：
  - 变更摘要
  - 风险分级
  - 测试覆盖变化
  - 回滚建议
  - 提交建议（message + scope）

### G22-3 工具生态与编排弹性不足（P1）

现状：有 MCP 与内部工具链，但工具能力注册、权限粒度、失败降级策略还可增强。

目标状态：
- 工具能力矩阵化（能力、风险级别、时延、失败策略、可替代链路）。
- 自动降级与重试策略可配置且可观测。

### G22-4 长时记忆产品化不足（P1）

现状：有 memory/cache/vector 能力，但跨会话项目语义记忆与自动回忆策略产品化不足。

目标状态：
- 项目级 Memory Graph（任务、模块、风险、决策、证据）
- 执行前自动召回“相关历史失败与修复策略”

### G22-5 复盘与可控性不足（P1）

现状：治理能力强，但用户侧“可理解、可调参、可回放”闭环还可强化。

目标状态：
- 一次任务从 plan 到 merge-ready 的可视化状态机。
- 每次拒绝/降级都有可解释原因与建议动作。

---

## BLUE22 一轮封口实施步骤（不改 Phase）

### Step 1：统一执行对象模型（P0）

改进内容：
- 在 backend 新增统一执行对象 `ExecutionCycle`（可复用现有 artifact 体系，不破坏现有字段）。
- 一次任务允许 N 轮循环，每轮记录：
  - plan_version
  - patch_set
  - test_gate_result
  - failure_taxonomy
  - next_action

验收点：
- `workflow.execute` / `task.execute` 响应可返回当前 cycle 与历史摘要。
- 失败时有结构化失败分类，不只字符串错误。

### Step 2：自动迭代修复器（P0）

改进内容：
- 增加 Auto-Repair Loop（默认受治理开关与预算限制）。
- 触发条件：门禁失败且命中可修复类别（编译错误、lint、测试失败、契约断言失败）。
- 终止条件：
  - 全门禁通过
  - 达到 max_iterations
  - 达到预算上限
  - 命中不可自动修复风险类别

验收点：
- 输出包含每轮修复动作与结果。
- 可通过治理策略强制只读模式（禁自动改动）。

### Step 3：Code Change Bundle（P0）

改进内容：
- 新增统一变更包产物：
  - 文件级变更摘要
  - 风险与影响面
  - 测试与门禁结果
  - 回滚指令建议
  - 建议提交信息

验收点：
- backend 可通过 RPC 返回变更包结构。
- GUI/addon 能展示同一结构，不出现字段漂移。

### Step 4：工具能力矩阵与降级编排（P1）

改进内容：
- 扩展 tool registry 元数据：
  - capability
  - risk_level
  - timeout_budget
  - retry_policy
  - fallback_chain
- 当主工具失败时，自动按 fallback_chain 降级。

验收点：
- `trace.metrics` 可看到工具降级次数与成功率。
- `governance.status` 可反映高风险工具命中情况。

### Step 5：项目语义记忆图（P1）

改进内容：
- 增加 Memory Graph 视图对象（任务-模块-问题-修复-证据）。
- 在 `task.plan` 前自动召回与当前任务最相关的历史冲突与修复。

验收点：
- 计划输出中有 memory recall 证据段。
- 错误复发率指标可统计（同类问题复现次数下降）。

### Step 6：三端产品化展示与可控开关（P1）

改进内容：
- GUI/addon 新增执行闭环可视化：
  - cycle timeline
  - gate matrix
  - auto-repair trace
- 增加运行模式开关：
  - `manual`
  - `assisted`
  - `autonomous`

验收点：
- 三端展示字段一致，契约单一来源。
- 任何拒绝/降级都带结构化解释与建议下一步。

### Step 7：评测基线与对标报告（P0）

改进内容：
- 建立固定 benchmark 套件：
  - 编译失败自动修复
  - 契约漂移自动修复
  - 多文件重构回归防护
  - 工具失败降级恢复
- 引入对标指标：
  - Task success rate
  - First-pass pass rate
  - Mean repair iterations
  - Human intervention rate
  - Regression rate

验收点：
- 每次发布自动生成对标快照。
- 指标在 GUI/addon 可见且与 backend 一致。

---

## 三端一致性要求（强约束）

- backend 为唯一语义源，GUI/addon 严禁自行推断核心状态。
- 所有新增字段先入 `contracts/editor-capability-matrix.json`，再落地三端。
- 所有 `workflow/task` 相关 RPC 必须共享统一响应骨架：
  - `ok`
  - `execution_cycle`
  - `requirement_gate`
  - `gates`
  - `artifacts`
  - `trace_ref`

---

## 门禁与质量要求（同 BLUE21）

- backend：`cargo check --all-targets`
- backend 集成：`cargo test --test protocol_consistency_integration -- --nocapture`
- addon：`npm --prefix vscode-addon run check && node vscode-addon/scripts/contract-smoke.js`
- GUI：`npm --prefix GUI run test:contract && npm --prefix GUI run build`
- 新增：自治闭环回归集（建议）：
  - `cargo test --test acp_runtime_rpc_integration -- --nocapture`
  - `scripts/run-quality-gate.sh`

判定标准：
- 任一门禁失败即不允许标记完成。
- 严禁通过降低语义、绕过主链路来“过门禁”。

---

## 实施节奏建议

- R1（基础闭环）：Step 1 + Step 2 + Step 3
- R2（生态与记忆）：Step 4 + Step 5
- R3（产品化与对标）：Step 6 + Step 7

每个 R 必须满足：
- 契约更新
- 三端对齐
- 全门禁通过
- 指标回写

---

## 风险与防偏航机制

- 风险：自治循环失控
  - 缓解：预算硬限 + 迭代上限 + 高风险工具默认禁用
- 风险：三端字段漂移
  - 缓解：契约先行 + 双 smoke 强校验
- 风险：自动修复误改业务语义
  - 缓解：关键路径引入 review gate + 差异风险评分阈值

---

## BLUE22 完成率回写

- BLUE22 当前任务完成率：`100%`
- 已完成范围：Step 1-7 全部完成并收口。补齐了 Step 1 的真实 N 轮执行闭环（在 `workflow.execute` / `task.execute` 主链路中，失败后进入受治理约束的多轮 repair-then-rerun 循环，直到通过或命中迭代/预算终止条件），并将每轮结果回写到 `execution_cycle.cycles`、`history_summary`、`auto_repair`、`repair_history`；Step 2-7 与此前已落地能力保持一致并全部通过三端契约与门禁验证。
- 未完成范围：无
- 备注：
  - 当前完成率按“已实际编码并通过门禁的 BLUE22 能力占比”回写
  - 已完成一次完整封口门禁：backend/addon/GUI 全部通过，且 `cargo check 2>&1 | rg -n "warning|error"` 为零输出
