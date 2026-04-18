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

---

## 三端一致性要求（强约束）

- backend 为唯一语义源，GUI/addon 严禁推断核心状态。
- 任何新增字段必须先入契约，再落地三端。
- 所有 workflow/task 响应统一骨架：
  - `ok`
  - `capability_profile`
  - `execution_cycle`
  - `requirement_gate`
  - `gates`
  - `artifacts`
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

---

## BLUE23 完成率回写模板

- BLUE23 本轮任务完成率：`0%`（初始模板）
- 回写规则：
  - 每完成一个 Step 并通过对应门禁后更新百分比
  - 全部 Step 完成且三端门禁全绿后回写 `100%`
- 备注：
  - 本文为“改通用平台”目标下的全套实施基线与封口标准
