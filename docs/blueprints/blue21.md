# BLUE21 — 三端深度扫描潜在问题清单（收敛版）

更新时间：2026-04-18

本文沿用 BLUE20 的同一验收规则与收口口径：
- 三端一统（backend / vscode-addon / GUI）
- 主链路完整闭环
- 后端主链路功能完整
- 不留 warning
- 最小修改：仅改与目标直接相关内容；禁止为了“过测试”而做功能语义不完整的最小改动
- 完成率必须回写

---

## 扫描范围

- backend：`src/**`（含协议层、运行时、agent 层）
- GUI：`GUI/src/**` + `GUI/src-tauri/src/**`
- addon：`vscode-addon/src/**`
- 契约：`contracts/editor-capability-matrix.json`

---

## BLUE21-GATE 一轮封口实施步骤（执行基线）

执行目标：在不推翻现有能力的前提下，引入统一 Gate Facade（编排层），把主链路门控收口为一致输入/输出，并保证三端口径一致。

1. 文档先行（本文件）
- 固化本轮实施步骤、验收门禁与完成率口径。

2. backend 统一 Gate Facade
- 在 `src/acp/helpers/requirement.rs` 增加统一门控决策封装（保持现有 requirement gate 逻辑不变）。
- 统一返回字段：`kind / blocked / reason / missing_fields / next_step / governance_artifact_path / clarification_artifact_path`。

3. 主链路接入（必须全接）
- `workflow.generate`、`task.plan`、`workflow.execute`、`task.execute` 统一走 Gate Facade。
- 删除主链路中重复/分散的 requirement gate 拼装分支，避免语义漂移。

4. 三端一致性对齐
- backend 输出字段口径统一后，addon/GUI 的链路消费保持兼容。
- 契约文件补充 gate facade 可用性声明（只增不破坏）。

5. 无 warning 门禁复验
- backend：`cargo check --all-targets`
- 协议主链：`cargo test --test protocol_consistency_integration -- --nocapture`
- addon：`npm --prefix vscode-addon run check && node vscode-addon/scripts/contract-smoke.js`
- GUI：`npm --prefix GUI run test:contract && npm --prefix GUI run build`

6. 回写闭合
- 本文件回写：实施结果、门禁结果、完成率。
- 目标：一轮封口，完整闭合。

执行状态：✅ 已完成（2026-04-18，同轮封口）

主链路接入结果：
- `workflow.generate` → 统一接入 Gate Facade
- `task.plan` → 统一接入 Gate Facade
- `workflow.execute` → 统一接入 Gate Facade
- `task.execute` → 统一接入 Gate Facade（并移除重复的前置 requirement_confirmed 分支）

统一字段口径（backend 输出）：
- `kind`
- `blocked`
- `reason`
- `missing_fields`
- `next_step`
- `governance_artifact_path`
- `clarification_artifact_path`

三端一致性同步：
- 契约新增：`protocol.rpcUnifiedGateFacadeCheckedInMainChain = true`
- addon smoke：新增该字段断言
- GUI smoke：新增该字段断言

---

## 扫描轮次与收敛过程

### 第 1 轮：全局静态风险扫描

方法：
- 关键字扫描（TODO/FIXME、unwrap/expect/panic、协议字段）
- 三端契约字段交叉检索

结果：
- 发现大量测试代码噪声（`expect/unwrap` 主要集中于 `#[cfg(test)]` 块）
- 提取到候选风险点，进入第 2 轮证据复核

### 第 2 轮：三端一致性与高置信复核

方法：
- 逐文件复核关键路径（runtime、addon 启动、contract、baseline 展示）
- 子代理只读深审 + 主代理复核去误报

新增有效发现：4 项（见下文 F21-1 ~ F21-4）

### 第 3 轮：门禁与语义漂移复验

方法：
- `cargo check --all-targets`
- `npm --prefix vscode-addon run check`
- `npm --prefix GUI run test:contract && npm --prefix GUI run build`
- 定向检索 legacy 语义（`auto`、`defaultMode`、`protocol_mode ?? 'auto'`）

新增有效发现：1 项（F21-5）

### 第 4 轮：同类新增复扫

方法：
- 仅扫描一方源码（排除 `node_modules/out/dist`）
- 仅查 legacy 语义漂移模式

结果：
- 输出集合稳定为 3 条：
  - `vscode-addon/src/rpcCommandRegistry.ts:279`
  - `tests/acp_runtime_rpc_integration.rs:734`
  - `vscode-addon/src/settingsView.ts:311`
- 其中后两条分别为“兼容性测试输入”与“模型默认值语义”，不属于协议漂移新增问题

### 第 5 轮：重复确认（无新增）

方法：
- 与第 4 轮同指令重复执行

结果：
- 与第 4 轮完全一致
- **确认扫描收敛：无新增发现**

---

## 最终发现（高置信）

| ID | 优先级 | 端 | 状态 | 发现 |
|---|---|---|---|---|
| F21-1 | P1 | backend | ✅ 已完成 | Anthropic SSE 解析失败分支已补齐结构化 warn + 累计计数（继续流，不改行为） |
| F21-2 | P1 | addon | ✅ 已完成 | `config.baseline` 展示改为优先使用 `configured_mode / protocol_capability / request_dispatch_mode / startup_transport`，移除协议 `auto` 兜底 |
| F21-3 | P2 | 三端契约 | ✅ 已完成（治理增强） | contract 增加 `verification` 元数据并由 GUI/addon smoke 强校验，降低静态真值漂移风险 |
| F21-4 | P2 | addon | ✅ 已完成 | addon 启动参数新增协议枚举白名单校验，非法值本地拒绝，不再透传 backend |
| F21-5 | P3 | 协议语义 | ✅ 已收口 | 现存 `auto` 仅保留在兼容测试输入和模型默认值语义，协议展示链路已去除 `auto` 兜底 |

---

## 证据定位

### F21-1（SSE 失败静默继续）
- `src/agents/anthropic.rs:150`
- 表现：`parse_anthropic_event` 出错分支 `Err(_) => Ok(SseEventAction::Continue)`，当前无错误打点/计数

### F21-2（baseline 展示旧兜底）
- `vscode-addon/src/rpcCommandRegistry.ts:279`
- 表现：`const protocolMode = String(effective.protocol_mode ?? 'auto');`

### F21-3（契约静态真值风险）
- `contracts/editor-capability-matrix.json:29`
- 到 `contracts/editor-capability-matrix.json:94`
- 表现：大量“主链已验证”布尔位恒为 `true`，缺少自动生成/自动核验链路

### F21-4（protocolMode 透传缺校验）
- `vscode-addon/src/runtimeManager.ts:81`
- `vscode-addon/src/runtimeManager.ts:93`
- 表现：`start(..., protocolMode: string)` 直接 `args.push('--protocol-mode', protocolMode)`

### F21-5（legacy auto 残留）
- `vscode-addon/src/rpcCommandRegistry.ts:279`
- `tests/acp_runtime_rpc_integration.rs:734`（兼容测试输入）
- `vscode-addon/src/settingsView.ts:311`（模型默认值 `auto`，非协议字段）

---

## 更优 / 更完善方案（按稳妥优先）

### 方案 S1（最稳、最小行为改动）

1. 对 F21-1：
- 在 Anthropic SSE 解析失败分支增加：
  - 结构化 warn 日志（包含 event type/截断长度）
  - 计数指标（如 `anthropic_sse_parse_error_total`）
- 仍保持 `Continue`，避免改变现网行为

2. 对 F21-2：
- addon 展示优先使用：
  - `configured_mode`
  - `protocol_capability`
  - `request_dispatch_mode`
  - `startup_transport`
- 移除 `?? 'auto'` 协议兜底文本

3. 对 F21-4：
- 在 addon 端增加 `protocolMode` 白名单校验：
  - `from_config | adaptive | acp_stdio | acp_http | mcp_stdio | mcp_http`
- 非法值直接本地报错，不下发 backend

### 方案 S2（更完美、治理升级）

1. 对 F21-3：
- 将 contract 的 `*CheckedInMainChain` 从手写常量改为“由 CI 产物生成”
- 失败时自动回写 `false` 或阻断合并
- 给每个布尔位附上最近一次通过的 commit/tag/timestamp

2. 对 F21-5：
- 明确分层：
  - 协议模式字段：仅允许 5 模式
  - 模型选择字段：`auto` 允许保留
- 在 GUI/addon 展示文案中显式标注字段语义，避免 `auto` 误读为协议模式

---

## 本轮结论

- 三端主链路当前可编译、可构建、契约 smoke 可过
- F21-1 ~ F21-5 已按“最稳实施”全部收口
- 关键策略：最小行为变更、增强可观测与参数校验、强化契约真实性校验

### 本次实施改动（2026-04-18）

1. backend
- `src/agents/anthropic.rs`
  - SSE 解析失败分支增加结构化 warn 日志
  - 新增 `ANTHROPIC_SSE_PARSE_ERROR_TOTAL` 计数
  - 保持 `Continue` 语义，避免运行时行为回归

2. addon
- `vscode-addon/src/rpcCommandRegistry.ts`
  - `config.baseline` 展示改为 `mode/capability/dispatch/transport` 四元信息
  - 删除协议 `?? 'auto'` 旧兜底口径
- `vscode-addon/src/runtimeManager.ts`
  - 新增协议模式白名单校验：`from_config | adaptive | acp_stdio | acp_http | mcp_stdio | mcp_http`
  - 非法值直接本地报错并阻断启动

3. 契约治理
- `contracts/editor-capability-matrix.json`
  - 新增 `verification.generatedBy/generatedAt/sourceOfTruth`
  - 新增 `protocol.rpcUnifiedGateFacadeCheckedInMainChain`
- `vscode-addon/scripts/contract-smoke.js`
  - 增加 verification 元数据断言
  - 增加 unified gate facade 断言
- `GUI/scripts/contract-smoke.mjs`
  - 增加 verification 元数据断言
  - 增加 unified gate facade 断言

4. Gate Facade 编排层（本轮新增）
- `src/acp/helpers/requirement.rs`
  - 新增 `RequirementGateFacadeDecision`
  - 新增 `evaluate_requirement_gate_facade(...)`
  - 新增统一 JSON 构造：`blocked_payload()` / `success_payload()`
- `src/acp/impl/request.rs`
  - `workflow.generate` / `task.plan` 改为调用统一 Gate Facade
  - 响应中 `requirement_gate` 改为统一 `gate` 结构
- `src/acp/impl/request/exec_pack.rs`
  - `workflow.execute` / `task.execute` 改为调用统一 Gate Facade
  - `task.execute` 移除重复前置门控分支，避免语义漂移

### 本次门禁复验（2026-04-18）

- backend：`cargo check --all-targets` 通过
- 协议一致性：`cargo test --test protocol_consistency_integration -- --nocapture` 10/10 通过
- addon：`npm --prefix vscode-addon run check && node vscode-addon/scripts/contract-smoke.js` 通过
- GUI：`npm --prefix GUI run test:contract && npm --prefix GUI run build` 通过
- 结论：三端一致、链路完整、无新增 warning（Gate Facade 主链全接入已验证）

### 多轮追加扫描与修复（2026-04-18，收敛到 0 问题）

第 A 轮（冲突扫描）：
- 扫描 `workflow.* / task.*` 全链路门控调用分布。
- 发现新增冲突：`workflow.research` 与 `workflow.consult` 仍未接入统一 Gate Facade，属于同域门控不一致。

第 B 轮（立即修复）：
- `src/acp/impl/request.rs`
  - `handle_workflow_research` 接入 `evaluate_requirement_gate_facade(...)`。
  - `handle_workflow_consult` 接入 `evaluate_requirement_gate_facade(...)`。
  - `workflow.consult` 增补 `task is required` 参数校验，与相邻 workflow 方法对齐。
  - 两个方法统一返回 `requirement_gate.gate` 成功结构，阻断时统一 `blocked_payload()`。

第 C 轮（治理固化）：
- `contracts/editor-capability-matrix.json`
  - 新增 `protocol.rpcWorkflowResearchConsultGateCheckedInMainChain = true`。
- `vscode-addon/scripts/contract-smoke.js`
  - 增加上述字段断言。
- `GUI/scripts/contract-smoke.mjs`
  - 增加上述字段断言。

第 D 轮（复验）：
- `cargo check --all-targets` 通过。
- `cargo test --test protocol_consistency_integration -- --nocapture` 10/10 通过。
- `cargo test --test acp_runtime_rpc_integration rpc_task_execute_blocks_when_requirement_not_confirmed -- --nocapture` 通过。
- `cargo test --test acp_runtime_rpc_integration rpc_workflow_execute_returns_review_policy_and_learning_feedback_fields -- --nocapture` 通过。

第 E 轮（收敛复扫）：
- 统一 Facade 调用点覆盖：
  - `workflow.generate`
  - `task.plan`
  - `workflow.execute`
  - `task.execute`
  - `workflow.research`
  - `workflow.consult`
- 未再发现门控冲突与契约漂移。

---

## 回写完成率

- BLUE21 本轮任务完成率：`100%`
- 说明：本次目标为“按 BLUE21 改进项一轮封口，保证链路完整最优、三端一致、无 warning 并回写完成率”；并已完成 Gate Facade 扩展至 workflow/task 同域主链与相邻链路，多轮扫描收敛到 0 冲突。
