# BLUE23 — 改通用平台全套步骤（实施基线）

更新时间：2026-04-18

本文沿用 BLUE22 的同一验收规则与收口口径：
- 三端一统（backend / vscode-addon / GUI）
- 主链路完整闭环
- 后端主链路功能完整
- 不留 warning
- 最小修改：仅改与目标直接相关内容；禁止为了“过测试”而做功能语义不完整的最小改动
- 完成率必须回写

---

## 前提与目标

前提：本轮允许把平台从“phase 驱动范式”升级为“通用能力平台范式”，但必须确保旧 phase 语义可兼容迁移。

目标：形成可对标顶级代码平台的通用执行内核，不再把核心能力绑定在 phase 名称上，而是抽象为稳定能力面：
- 任务理解与约束治理
- 计划与执行循环
- 代码改动与验证闭环
- 工具编排与降级
- 记忆与复盘
- 三端统一可观测

---

## 扫描范围

- backend：`src/**`（ACP 请求链、governance、orchestration、tool、memory、config）
- GUI：`GUI/src/**` + `GUI/src-tauri/src/**`
- addon：`vscode-addon/src/**`
- 契约：`contracts/editor-capability-matrix.json`
- 文档与门禁：`README.md`、`README.zh-CN.md`、`scripts/**`

---

## 现状差距（改通用平台视角）

### G23-1 能力语义仍与 phase 强耦合（P0）

现状：部分治理、路由、执行策略仍以 phase 名称作为主键。

目标状态：
- 统一以 capability profile 作为第一语义源。
- phase 仅作为兼容输入层，不再作为内核决策核心。

### G23-2 配置与契约仍偏 phase 中心（P0）

现状：配置项、指标项、文档口径存在 phase 词汇主导。

目标状态：
- 配置支持“通用平台模式 + phase 兼容模式”双轨。
- 契约以 capability/rule/gate 为核心字段，phase 仅保留映射字段。

### G23-3 三端展示与交互模型不统一（P1）

现状：backend 已有强治理，GUI/addon 在执行闭环展示上仍偏功能散点。

目标状态：
- 三端使用同一执行对象骨架（task -> cycle -> gate -> artifact -> metrics）。
- 前端不自推核心状态，只消费契约字段。

### G23-4 迁移安全网不足（P0）

现状：若直接改 phase，有治理语义漂移、指标断档和兼容回归风险。

目标状态：
- 提供完整迁移层、双写窗口、回滚开关与对账机制。

### G23-5 Git 原生代码工作流不足（P0）

现状：平台具备 workflow/task 与治理能力，但对顶级代码平台常见的“分支 / worktree / commit bundle / PR 语义对象”支持仍不够系统。

目标状态：
- 一次任务天然绑定 repo 工作上下文。
- 支持隔离 worktree、结构化 patch set、提交建议、PR-ready 说明与风险摘要。

### G23-6 隔离执行与沙箱层级不足（P0）

现状：已有 hardening / budget / sandbox deny 语义，但还缺少“按任务风险动态选择隔离级别”的平台级调度层。

目标状态：
- 针对读代码、改代码、运行测试、执行工具分别选择不同 sandbox level。
- 高风险工具与写操作默认进入更强隔离区。

### G23-7 人类审批检查点不足（P1）

现状：已有 review gate，但“必须人工确认才能跨越”的审批点还未产品化为统一平台对象。

目标状态：
- 对高风险变更、跨模块重构、批量删除、外部副作用操作设置 approval checkpoint。
- GUI/addon 可清晰展示待审批项、证据与建议动作。

### G23-8 多代理协同与委派模型不足（P1）

现状：已有 planner / reviewer / agent ranking 等基础能力，但还缺少稳定的“主代理-子代理-审查代理”协同协议。

目标状态：
- 支持任务拆分、并行子任务、结果合并、冲突消解、最终审查。
- 每个子代理输出统一 artifact，方便追责与复盘。

### G23-9 评测驱动优化闭环不足（P1）

现状：已有 benchmark / requests / replay 基础，但缺少“评测结果直接反哺策略选择”的统一闭环。

目标状态：
- benchmark -> regression detection -> strategy tuning -> rollout recommendation 完整闭环。
- 发布前自动生成平台能力评分卡。

### G23-10 组织级治理与策略发布不足（P1）

现状：已有 RULES 与治理接口，但组织/团队级策略版本化、灰度发布、回滚与例外审批机制仍可增强。

目标状态：
- 支持组织级 policy bundles、环境分层（dev/staging/prod）、例外审批与到期失效。
- 平台可追踪是谁、何时、为何修改了策略。

---

## BLUE23 一轮封口实施步骤（改通用平台）

### Step 1：定义通用平台语义层（P0）

改进内容：
- 新增 Platform Capability Model（PCM）：
  - `intent`
  - `constraints`
  - `gates`
  - `execution_cycle`
  - `toolchain`
  - `evidence`
- 输出统一 `capability_profile`（替代内核 phase 主键语义）。

验收点：
- backend 能在请求入口生成 `capability_profile`。
- `phase` 仍可输入，但被映射到 capability，不直接驱动核心逻辑。

### Step 2：建立 phase 兼容映射层（P0）

改进内容：
- 新增 `phase -> capability_profile` 映射表（可配置、可审计）。
- 旧请求不改参数即可继续运行。
- 映射失败时返回结构化错误并给出修复建议。

验收点：
- 旧 phase 配置下主链路功能不退化。
- 映射层命中率、失败率可观测。

### Step 3：重构执行主链到通用循环（P0）

改进内容：
- 把 `workflow.generate / task.plan / workflow.execute / task.execute / workflow.research / workflow.consult` 统一纳入通用执行循环。
- 循环对象统一：
  - `cycle_id`
  - `plan`
  - `actions`
  - `gate_results`
  - `artifacts`
  - `next_action`

验收点：
- 各入口返回统一骨架字段。
- Gate 语义一致，不因入口差异而漂移。

### Step 4：治理与预算系统通用化（P0）

改进内容：
- 将预算、风险、工具权限从 phase 维度迁移到 capability/risk band 维度。
- 保留 phase 兼容策略，但由映射层转换后执行。

验收点：
- `governance.status` 同时可读“原 phase 视图”与“通用平台视图”。
- 预算拒绝、风险阻断有统一错误模型。

### Step 5：配置系统双轨迁移（P0）

改进内容：
- 配置支持：
  - `platform_mode = "universal" | "phase_compat"`
- universal 模式优先 capability 配置。
- phase_compat 模式走映射层。

验收点：
- 两种模式均可通过全门禁。
- 可在线回退到 `phase_compat`。

### Step 6：契约升级与三端对齐（P0）

改进内容：
- 扩展 `contracts/editor-capability-matrix.json`：
  - `universalPlatformEnabled`
  - `phaseCompatMappingEnabled`
  - `universalExecutionCycleSchemaVersion`
  - `universalGateModelCheckedInMainChain`
- GUI/addon smoke 同步断言。

验收点：
- 三端字段一致，无二义性。
- 契约作为唯一事实来源。

### Step 7：指标连续性与对账（P1）

改进内容：
- 建立 phase 指标到 universal 指标映射：
  - success_rate
  - gate_reject_rate
  - repair_iterations
  - intervention_rate
- 双写窗口内同时产出两套指标并自动对账。

验收点：
- 指标断档为 0。
- 对账偏差超过阈值时自动告警。

### Step 8：自动化迁移与回滚（P0）

改进内容：
- 提供迁移命令：
  - 配置迁移
  - 契约升级
  - 文档升级
  - 回滚脚本
- 迁移全过程生成审计报告。

验收点：
- 一键迁移可执行。
- 一键回滚可恢复到改造前状态。

### Step 9：基准对标验证（P0）

改进内容：
- 在 universal 模式下跑完整 benchmark：
  - 编译修复
  - 契约一致性
  - 工具降级恢复
  - 多轮自动修复收敛
- 与 phase_compat 模式并行对比。

验收点：
- universal 模式不低于兼容模式稳定性。
- 关键指标达到或超过既有基线。

### Step 10：Git 原生工作流对象化（P0）

改进内容：
- 增加 repo-native execution context：
  - `repository`
  - `branch`
  - `worktree`
  - `patch_set`
  - `commit_bundle`
  - `pr_bundle`
- 为代码任务提供标准化产物：
  - 改动摘要
  - 风险说明
  - 提交建议
  - PR 说明建议

验收点：
- 代码任务不再只是返回 workflow artifact，而是返回 repo-ready change bundle。
- 三端均可展示相同变更对象。

### Step 11：分层沙箱与执行隔离（P0）

改进内容：
- 定义统一 `sandbox_profile`：
  - `read_only`
  - `workspace_write`
  - `workspace_exec`
  - `elevated`
- 任务进入执行前先完成 risk -> sandbox_profile 决策。
- 高风险写操作默认进入更高等级隔离与强审计。

验收点：
- 每次任务都能看到选定的 sandbox profile 与选择原因。
- 高风险操作有强制审批或拒绝策略。

### Step 12：审批检查点产品化（P1）

改进内容：
- 新增 `approval_checkpoint` 对象：
  - `reason`
  - `risk_level`
  - `required_evidence`
  - `approver_scope`
  - `expires_at`
- review gate 与 approval checkpoint 区分：
  - review gate 偏质量/治理
  - approval checkpoint 偏人类授权

验收点：
- 高风险任务可被显式挂起等待审批。
- addon/GUI 能恢复挂起任务并继续执行。

### Step 13：多代理协同协议（P1）

改进内容：
- 定义 `agent_session` / `subtask_session` / `merge_session` 对象。
- 引入标准角色：
  - planner
  - implementer
  - verifier
  - reviewer
- 增加结果合并、冲突裁决与统一审查步骤。

验收点：
- 多代理执行有统一审计轨迹。
- 子任务结果可并行执行后汇总，不再依赖隐式约定。

### Step 14：评测驱动调优与发布评分卡（P1）

改进内容：
- benchmark 结果直接写入 strategy tuning 输入。
- 自动生成版本评分卡：
  - 代码修复成功率
  - 首轮通过率
  - 平均修复轮次
  - 人工干预率
  - 回归率
  - 成本/时延指标

验收点：
- GUI/addon 可查看评分卡。
- 发布前必须通过评分卡阈值。

### Step 15：组织级策略发布与例外治理（P1）

改进内容：
- 增加 policy bundle 版本、灰度发布、例外审批、过期回收。
- 支持不同环境：
  - local/dev
  - staging
  - production
- 每条例外策略必须可审计、可追责、可失效。

验收点：
- 组织级治理有版本号与发布时间线。
- 任何策略例外都有审计记录。

---

## 三端一致性要求（强约束）

- backend 为唯一语义源，GUI/addon 严禁推断核心状态。
- 任何新增字段必须先入契约，再落地三端。
- 所有 workflow/task 响应统一骨架：
  - `ok`
  - `capability_profile`
  - `execution_cycle`
  - `sandbox_profile`
  - `requirement_gate`
  - `approval_checkpoint`
  - `gates`
  - `artifacts`
  - `change_bundle`
  - `trace_ref`

---

## 门禁与质量要求（同 BLUE22）

- backend：`cargo check --all-targets`
- backend 集成：`cargo test --test protocol_consistency_integration -- --nocapture`
- addon：`npm --prefix vscode-addon run check && node vscode-addon/scripts/contract-smoke.js`
- GUI：`npm --prefix GUI run test:contract && npm --prefix GUI run build`
- 通用平台新增回归（建议）：
  - `cargo test --test acp_runtime_rpc_integration -- --nocapture`
  - `scripts/run-quality-gate.sh`

判定标准：
- 任一门禁失败即不允许标记完成。
- 严禁通过降语义、绕主链路来“过门禁”。

---

## 实施节奏建议

- R1（语义与兼容层）：Step 1 + Step 2 + Step 5
- R2（主链重构与治理）：Step 3 + Step 4 + Step 6
- R3（可观测与迁移闭环）：Step 7 + Step 8 + Step 9
- R4（代码平台化）：Step 10 + Step 11 + Step 12
- R5（顶级通用平台补齐）：Step 13 + Step 14 + Step 15

每个 R 必须满足：
- 契约更新
- 三端对齐
- 全门禁通过
- 指标回写

---

## 风险与防偏航机制

- 风险：通用化导致语义回归
  - 缓解：phase_compat 双轨 + 映射审计 + 灰度放量
- 风险：迁移期三端字段漂移
  - 缓解：契约先行 + 双 smoke + 版本闸门
- 风险：指标不可比
  - 缓解：双写对账窗口 + 偏差阈值告警
- 风险：回滚不可用
  - 缓解：迁移脚本必须同批次产出逆向回滚脚本
- 风险：多代理协同导致结果不一致
  - 缓解：统一 merge session + 最终 reviewer 裁决 + artifact 对账
- 风险：Git 原生工作流引入仓库污染
  - 缓解：默认隔离 worktree + commit bundle 审批后落主分支
- 风险：组织级例外策略失控扩散
  - 缓解：例外策略强制过期时间 + 审计追踪 + 定期回收

---

## 再次深度比较后的补齐结论

若目标不是“可运行的通用平台”，而是“顶级通用代码平台”，在 BLUE23 原有 Step 1-9 之外，至少还要补齐以下五类能力：

1. Git 原生工作流对象化
- 没有这一层，就仍偏 runtime/workflow 平台，而不是 repo-native 代码平台。

2. 分层沙箱与按风险调度
- 没有这一层，平台治理仍停留在 deny/allow，不足以支撑顶级自治代码执行。

3. 审批检查点产品化
- 没有这一层，高风险操作的人机协同闭环仍不完整。

4. 多代理协同协议
- 没有这一层，复杂任务只能靠单代理或隐式约定，扩展性与稳定性都不足。

5. 评测驱动发布评分卡 + 组织级策略发布
- 没有这一层，平台就缺少面向组织落地和持续优化的最后一公里。

---

## BLUE23 完成率回写模板

- BLUE23 本轮任务完成率：`0%`（初始模板）
- 回写规则：
  - 每完成一个 Step 并通过对应门禁后更新百分比
  - 全部 Step 完成且三端门禁全绿后回写 `100%`
- 备注：
  - 本文为“改通用平台”目标下的全套实施基线与封口标准
