# BLUE39 — GUI 对标 CHERRY STUDIO / MANUS / OPENCLAW 功能补充蓝图

更新时间：2026-05-08

> 注意：本文聚焦 GUI 其他界面（非 Chat 主功能）对标差距。
> 基于当前代码实际实现进行差距识别，不以文档声明为准。

---

## 0. 核心规则

本文仅记录 GUI 推荐（建议）但尚未完全实现、或已实现但未接入主链路的功能模块。

### 0.1 硬性执行规则（复制 BLUE38）

1. 5 种协议全链路闭合 — auto、acp stdio、acp http、mcp stdio、mcp http。每个推荐能力必须接入全部 5 种协议模式，不允许静默缺失。
2. 3 种服务器 Profile 全链路闭合 — profile-local、profile-simple-server、profile-multi-users-server。每个推荐能力必须在全部 3 种 profile 特性集下正确编译和行为一致。不允许 cfg 不匹配。
3. 注释英文 — 所有新增模块的代码注释必须使用英文。不允许中英文混合。
4. 国际化（i18n）全覆盖 — 所有面向用户的字符串（GUI、addon、后端日志）必须经过 locale 键转译。不允许任何语言的硬编码展示字符串。
5. 完整闭合 — 本文列出的每个模块最终必须达到：编译通过、零警告、接入 governance.status、可通过 health 端点观测、有集成测试覆盖。
6. 三端一致性 — backend（Rust）、GUI、vscode-addon。无字段漂移，无静默回退，契约 smoke 必须断言全部三端。
7. 零警告、零冲突、零遗漏 — 最终验证必须显示 cargo check --all-features 零警告，生产代码中无 allow dead_code，无未实现的 match 分支。
8. 回写完成率 — 每轮完成后，回写完成率（简述）。
9. 不要随意变更计划 — 严格按计划完整实施改进，未经充分验证和讨论，不要随意调整计划或回退已完成改进。
10. 三端一统（backend / GUI / vscode-addon）。
11. 主链路完整闭环。
12. 最完美、最优化修改，不需要简化修改或最小修改。
13. 不留 warning（以后端 cargo clippy --all-features -- -D warnings 为硬门）。
14. 不允许占位、空函数、逻辑错误、不完整函数或结构。
15. 功能增强 — 所有新增功能根据 local、simple-server、multi-users-server 接入主链路，纳入对应总线框架内。

### 0.2 扫描范围

来源与文件范围：

1. GUI 主应用与路由：gui/src/app.rs
2. GUI 视图：gui/src/views/*.rs
3. 国际化：gui/src/i18n.rs
4. 后端 GUI 客户端契约：gui/src/backend.rs

对标对象：

1. CHERRY STUDIO（模型与工作台能力）
2. MANUS（执行与流程编排能力）
3. OPENCLAW（开源代理与可观测能力）

---

## 1. 当前已具备能力（非差距项）

以下能力在 GUI 中已具备基础实现，不列为本轮新增差距：

1. 多 Tab 架构与功能开关（Monitor、Chat、Skills、Workflow、AutoTune、Security、Config、Providers、Settings）。
2. Providers 基础管理：新增、删除、更新 key、模型选择、推送配置。
3. Skills 基础能力：列表、创建、URL 导入、基础刷新。
4. Workflow 基础能力：步骤增删、启停、串行执行、超时终止、结果落盘。
5. Security 偏好：危险操作二次确认、API key 脱敏、外链导入拦截。
6. Config 编辑：JSON 直接编辑并应用。

---

## 2. 推荐未实现功能模块（对标差距清单）

### GUI-01 — 执行中枢与任务可观测闭环（对标 MANUS）

优先级：P0

当前状态：

1. Workflow 为本地串行 shell 步骤执行。
2. 缺少任务中心、步骤状态机、运行历史检索。
3. 缺少步骤级可视化事件流（开始、结束、输出、错误、耗时）。

差距：

1. 无任务队列管理（并发上限、排队、取消、重试、暂停/继续）。
2. 无审批节点（步骤级人工批准）及本次放行策略。
3. 无跨会话运行历史追踪与筛选。

推荐行动：

1. 引入 WorkflowRun、WorkflowStepRun、RunEvent 持久化模型。
2. 增加任务运行中心面板（运行中、成功、失败、已取消）。
3. 增加步骤级时间线与 stderr/stdout 结构化展示。
4. 对危险步骤接入 Security 二次确认策略（含按步骤记忆）。

验收标准：

1. Workflow 运行可追踪至步骤级事件。
2. 任一运行可重放关键上下文。
3. 可取消、重试、恢复，且状态一致。

---

### GUI-02 — AutoTune 参数接入主链路（对标 CHERRY STUDIO）

优先级：P0

当前状态：

1. AutoTune 页面可编辑 temperature、top_p、max_tokens、aggressive 并持久化。
2. 参数未实际注入 Chat 或 Workflow 请求链路。

差距：

1. 存在有配置无效果问题。
2. 缺少全局默认与会话覆盖优先级规则。

推荐行动：

1. 在 Chat 请求 options 中接入 AutoTune 参数。
2. 在 Workflow 触发 AI 步骤时透传同一参数集。
3. 定义优先级：会话覆盖 > 工作流覆盖 > 全局 AutoTune 默认。

验收标准：

1. 修改 AutoTune 后请求侧参数可观测。
2. 不同会话和流程可按优先级正确覆盖。

---

### GUI-03 — Skills 生命周期管理（对标 OPENCLAW）

优先级：P1

当前状态：

1. 已有创建、导入、列表。
2. 缺少编辑、删除、启停、版本化、试运行。

差距：

1. Skills 仅为登记，非完整生命周期管理。
2. 无版本回滚与变更审计。

推荐行动：

1. 增加 Skill 编辑器（name、description、prompt、input_schema）。
2. 增加启用/禁用与删除流程。
3. 增加版本快照与回滚。
4. 增加试运行面板（输入 JSON、输出预览、错误定位）。

验收标准：

1. Skill 可创建、编辑、版本化、回滚、试运行。
2. 删除和禁用不影响历史运行可追溯。

---

### GUI-04 — Providers 运营化能力（对标 CHERRY STUDIO）

优先级：P1

当前状态：

1. Providers 支持配置管理与推送。
2. 缺少质量验证、能力标注、路由策略。

差距：

1. 无单 Provider 连通性与延迟测试。
2. 无模型能力标签（上下文长度、工具调用、成本区间、速率限制）。
3. 无路由策略（主备切换、成本优先、延迟优先、质量优先）。

推荐行动：

1. 增加 Test Connection 与 Test Completion。
2. 引入模型能力元数据展示。
3. 增加 provider/model route policy 面板。

验收标准：

1. 配置前后可验证连通性与响应质量。
2. 路由策略在实际请求中可观测生效。

---

### GUI-05 — Monitor 历史化监控与告警

优先级：P1

当前状态：

1. 仅显示健康、成功率、延迟、rpm、provider 就绪状态。
2. 缺少历史趋势、异常聚类、告警机制。

差距：

1. 无时间窗口趋势（1m、5m、1h）。
2. 无错误分布与失败样本。
3. 无阈值告警策略与通知。

推荐行动：

1. 增加 metrics 时间序列缓存与图表展示。
2. 增加错误类型聚类与最近失败请求摘要。
3. 增加阈值告警（本地提示与状态栏指示）。

验收标准：

1. 可回看至少近 1 小时关键指标趋势。
2. 告警触发与恢复路径明确。

---

### GUI-06 — Config Editor 安全化与可回滚

优先级：P1

当前状态：

1. Config 以原始 JSON 直接编辑应用。
2. 缺少 schema 校验、diff 预览、备份回滚。

差距：

1. 高风险误改不可控。
2. 缺少审计与恢复能力。

推荐行动：

1. 增加 JSON schema 校验和错误定位。
2. 增加 Apply 前 diff 预览。
3. 增加配置快照与一键回滚。

验收标准：

1. 非法配置无法进入主链路。
2. 任意应用操作可恢复到前一版本。

---

### GUI-07 — 跨页面 i18n 收口（非 Chat）

优先级：P2

当前状态：

1. Chat i18n 完整度较高。
2. Workflow、Security、AutoTune、Config、Setup 仍存在硬编码显示文本。

差距：

1. 多语言一致性不完整。
2. 文案语义在不同页面不统一。

推荐行动：

1. 全量提取硬编码文案为 i18n key。
2. 统一按钮语义词典（Run、Apply、Push、Restart 等）。
3. 新增 i18n 回归检查（grep + UI smoke）。

验收标准：

1. GUI 所有可见字符串均可通过 locale 切换。
2. 不再出现硬编码可见文案。

---

### GUI-08 — Setup/Settings 企业化能力

优先级：P2

当前状态：

1. Setup 为单次引导。
2. Settings 为基础配置开关。

差距：

1. 缺少多环境配置与切换。
2. 缺少凭据来源优先级可视化。
3. 缺少配置包导入导出与脱敏分享。

推荐行动：

1. 增加环境管理（dev、stage、prod）。
2. 增加 secret source 策略配置（keyring/env/file）。
3. 增加配置导入导出和脱敏导出。

验收标准：

1. 多环境切换无字段漂移。
2. 凭据来源策略可观测且可回滚。

---

## 3. 三竞品对标映射

### 3.1 相对 CHERRY STUDIO

主要差距：

1. 模型运营深度（provider 验证、能力标签、路由策略）。
2. 监控图表化与可回看能力。
3. 配置治理体验（diff、校验、回滚）。

### 3.2 相对 MANUS

主要差距：

1. 执行中枢能力（任务队列、审批、历史、可恢复）。
2. 运行路径可追踪性与事件时间线。

### 3.3 相对 OPENCLAW

主要差距：

1. Skills 生命周期管理深度。
2. 运行与观测闭环。
3. 策略路由与治理接入。

---

## 4. 迭代建议（3 周）

第 1 周：

1. 完成 GUI-02（AutoTune 接入）与 GUI-07（i18n 收口）。
2. 完成 GUI-04 的最小连通性测试能力。

第 2 周：

1. 完成 GUI-01 执行中枢基础模型与任务中心面板。
2. 完成 GUI-05 指标趋势与失败样本展示。

第 3 周：

1. 完成 GUI-03 Skills 生命周期核心功能（编辑、删除、试运行）。
2. 完成 GUI-06 配置 diff + 校验 + 回滚。
3. 完成 GUI-08 多环境切换最小可用版本。

---

## 5. 完成定义（Definition of Done）

每项功能必须满足：

1. cargo check 通过。
2. cargo clippy 在对应目标集零 warning。
3. GUI 有可执行 smoke 覆盖。
4. backend 与 GUI 契约字段一致。
5. i18n 覆盖完成。
6. 文档与变更回写到对应 BLUE 文件。

---

## 6. Backend 可行性确认（对比结果）

本节基于当前 backend 实际可调用能力进行确认，核心判断维度：

1. 是否已有稳定 RPC/HTTP 接口。
2. 是否已有可观测字段可供 GUI 消费。
3. 是否需要先补后端再做 GUI。

### 6.1 总览结论

| 编号 | 项目 | 可行性 | 结论 |
|------|------|--------|------|
| GUI-01 | 执行中枢与任务可观测闭环 | 中高 | backend 已有 `workflow.execute` / `task.execute` 与执行产物，但缺少统一任务生命周期 API（取消/暂停/恢复/历史查询） |
| GUI-02 | AutoTune 参数接入主链路 | 高 | chat 链路可通过 `options.extra` 透传 agent 参数；GUI 可先做。workflow 链路需补参数映射 |
| GUI-03 | Skills 生命周期管理 | 中高 | 已有 `skill.create/import/enable/disable/list/remove`，缺少 `skill.update` 与版本快照 API |
| GUI-04 | Providers 运营化能力 | 中 | 已有 `provider.configure` / `provider.status` / `models.list`，但 `provider.configure` 当前仅持久化 key，未做连通性与质量验证 |
| GUI-05 | Monitor 历史化与告警 | 中高 | backend 已有 `runtime.health`、`health.probes`、`metrics.prometheus`、`trace.get`，可先做 GUI 侧时间窗缓存；长期建议补后端聚合 API |
| GUI-06 | Config Editor 安全化与可回滚 | 高 | 主要为 GUI 本地配置治理能力，后端依赖低 |
| GUI-07 | 跨页面 i18n 收口 | 高 | 纯 GUI 侧能力，后端无阻塞 |
| GUI-08 | Setup/Settings 企业化能力 | 中高 | 多环境与凭据策略主要在 GUI；若要环境级 runtime 生效，需配合 `config.reload` / profile 装配策略 |

### 6.2 关键 backend 证据（当前已具备）

1. Chat / Stream：已有 `/chat`、`/chat/stream` 与 `chat` RPC 主链路。
2. Workflow / Task：已有 `workflow.confirm/clarify/research/consult/generate/execute`、`task.plan`、`task.execute`。
3. Skills：已有 `skill.create`、`skill.import`、`skill.list_imported`、`skill.enable`、`skill.disable`、`skill.remove`。
4. Providers / Models：已有 `provider.configure`、`provider.status`、`models.list`。
5. Monitor：已有 `runtime.health`、`health.probes`、`metrics.prometheus`、`trace.get`、`governance.status`。
6. Config：已有 `config.baseline`、`config.reload`。

### 6.3 backend 缺口与前置依赖

以下能力若要达到 BLUE39 目标，需要先补 backend：

1. 任务中心类 API：缺少 `workflow.run.list`、`workflow.run.get`、`workflow.run.cancel`、`workflow.run.pause`、`workflow.run.resume` 等统一生命周期接口。
2. Skill 编辑/版本 API：缺少 `skill.update`、`skill.version.list`、`skill.version.rollback`。
3. Provider 验证 API：缺少 `provider.test_connection`、`provider.test_completion`、`provider.capabilities`。
4. 聚合监控 API：缺少按时间窗口聚合（1m/5m/1h）与错误分类统计接口。
5. Workflow LLM 参数映射：缺少将 GUI AutoTune 参数稳定映射到 `workflow.execute/task.execute` 执行上下文的统一约定字段。

### 6.4 实施建议（按可行性落地）

1. 先做高可行项（GUI-02 chat 部分、GUI-06、GUI-07），无需等待 backend。
2. 并行推进中高可行项（GUI-03、GUI-05、GUI-08），采用 GUI 先行 + backend 增量补齐。
3. 对 GUI-01、GUI-04 建立 backend 前置里程碑，先交付最小 API 集，再做完整界面。

### 6.5 可行性确认结论

BLUE39 所列项目总体可行，且不存在“完全不可落地”的项。

1. 约 40% 可直接 GUI 侧落地（高可行）。
2. 约 35% 可在现有 backend 能力上增量落地（中高可行）。
3. 约 25% 需先补 backend 专用 API（中可行，受前置约束）。

---

## 7. 后端先行补齐清单（先补后端）

目标：优先补齐 BLUE39 中阻塞 GUI 落地的后端接口与契约，按 P0 -> P1 推进。

### 7.1 P0（必须先做）

#### BE-01 任务运行生命周期 API（支撑 GUI-01）

新增 RPC：

1. `workflow.run.list`（分页查询运行实例）
2. `workflow.run.get`（查询单次运行详情）
3. `workflow.run.cancel`（取消运行）
4. `workflow.run.pause`（暂停运行）
5. `workflow.run.resume`（恢复运行）

最小返回字段（统一契约）：

1. `run_id`
2. `task`
3. `status`（queued/running/paused/succeeded/failed/cancelled）
4. `created_at`、`started_at`、`ended_at`
5. `phase`
6. `error`（可空）
7. `artifacts`（执行产物路径列表）

验收标准：

1. `workflow.execute` / `task.execute` 产生可追踪 `run_id`。
2. GUI 可查询历史、查看详情、执行取消/暂停/恢复。
3. 并发运行下状态转换一致且可重放。

#### BE-02 Provider 验证与能力探测 API（支撑 GUI-04）

新增 RPC：

1. `provider.test_connection`
2. `provider.test_completion`
3. `provider.capabilities`

最小返回字段：

1. `ok`
2. `latency_ms`
3. `model`
4. `error_code`、`error_message`（失败时）
5. `capabilities`：`context_window`、`tool_calling`、`vision`、`rate_limit`、`cost_tier`

验收标准：

1. 不可用 provider 可在配置阶段被阻断并返回可读错误。
2. 可用 provider 返回稳定能力标签，供 GUI 路由策略消费。

#### BE-03 Workflow/Task 参数映射规范（支撑 GUI-02）

新增约定：

1. 统一 `options.extra` 中可透传到执行链路的白名单字段。
2. 明确温度与采样参数字段：`temperature`、`top_p`、`max_tokens`、`model`。
3. 输出“生效参数回显”字段，便于 GUI 校验是否真正生效。

验收标准：

1. `workflow.execute` / `task.execute` 对白名单参数可稳定生效。
2. 响应中可见 `effective_options`（最终应用参数）。

### 7.2 P1（P0 后立即跟进）

#### BE-04 Skill 编辑与版本 API（支撑 GUI-03）

新增 RPC：

1. `skill.update`
2. `skill.version.list`
3. `skill.version.rollback`

最小返回字段：

1. `skill_id`
2. `version`
3. `updated_at`
4. `updated_by`（可选）
5. `change_summary`

验收标准：

1. Skill 支持编辑、查看历史版本、回滚到指定版本。
2. 历史运行与当前版本解耦，保证可追溯。

#### BE-05 聚合监控 API（支撑 GUI-05）

新增 RPC：

1. `metrics.window.query`
2. `metrics.errors.summary`

最小返回字段：

1. `window`（1m/5m/1h）
2. `series`（qps、p95、error_rate、success_rate）
3. `error_groups`（按 error_code/type 聚类）
4. `sample_failures`（最近失败样本摘要）

验收标准：

1. GUI 无需自行拼装原始指标即可展示趋势。
2. 错误分类与失败样本可直接用于告警和排障。

### 7.3 交付顺序（建议）

1. 第 1 批：BE-01 + BE-03（先打通执行中枢与参数生效闭环）。
2. 第 2 批：BE-02（补 provider 运营化验证闭环）。
3. 第 3 批：BE-04 + BE-05（补技能生命周期与监控聚合）。

### 7.4 与 GUI 条目映射

1. GUI-01 <- BE-01
2. GUI-02 <- BE-03
3. GUI-03 <- BE-04
4. GUI-04 <- BE-02
5. GUI-05 <- BE-05
6. GUI-06/GUI-07/GUI-08：可并行 GUI 先行，不阻塞上述后端主线。

### 7.5 Definition of Ready（后端任务开工前）

1. 每个 RPC 的 request/response JSON schema 明确。
2. 错误码约定明确（参数错误、鉴权失败、上游超时、限流）。
3. 至少 1 个集成测试覆盖每个新增 RPC。
4. 文档同步更新：方法名、字段、示例请求与示例响应。
