# FUTURE3 — 全能助手化升级路线图（OpenClaw 级能力目标）

> 目标：在 FUTURE2 的双轨基础上，将 go-on 升级为“全能助手”运行时：
> 能规划、能执行、能审查、能记忆、能评测、能持续优化。
>
> 规则对齐 FUTURE2：
> - 保持单机与服务器双轨并存
> - 采用目标编号 + 分阶段执行 + 风险矩阵 + DoD
> - 以可落地工程路径为主，不做空泛概念堆砌

---

## 背景与结论

FUTURE2 解决的是“部署形态与数据后端升级”（Local: SQLite3+ACP / Server: PostgreSQL+pgvector+MCP）。

FUTURE3 解决的是“能力形态升级”：从可用代理，演进到 OpenClaw 风格的全能助手（工程级可控、多角色协作、可审计可回放、可持续进化）。

核心能力缺口：

| 领域 | 当前基础 | 目标状态 |
|---|---|---|
| 任务智能 | Phase 路由为主 | 任务图 + Planner/Executor 分离 + 可恢复执行 |
| 工具闭环 | 工具调用已具雏形 | think-act-observe 多轮闭环 + 预算/权限/重试策略 |
| 多代理协作 | fallback + review gate | 角色化协作（planner/coder/tester/reviewer/researcher） |
| 记忆系统 | 有缓存/向量/摘要 | 类型化记忆策略 + 晋升/漂移控制 + 决策引用 |
| 审查验证 | 审查门禁已接入 | 对抗式验证 + 证据化裁决 + 自动回归执行 |
| 观测评测 | 有运行指标 | 任务级 trace、失败分类、基准回放、质量评分 |
| 产物治理 | 输出可落地 | Artifact 契约化 + 版本化 + 可追责链路 |

---

## 目标

| ID | 目标 | 目标文件/模块 |
|----|------|---------------|
| M1 | 建立全能助手运行模式 `assistant_mode = omnipotent`（兼容现有模式） | `src/orchestration/*`、`src/core/config.rs` |
| M2 | 引入 Planner/Executor 双核：规划与执行职责完全分离 | `src/orchestration/*` |
| M3 | 引入任务图 TaskGraph（节点、依赖、重试、汇合、回滚） | `src/orchestration/graph/*` |
| M4 | 建立统一工具协议 ToolRuntime（预算、权限、超时、幂等） | `src/orchestration/tool.rs`、`src/mcp/*` |
| M5 | 角色化多代理协作协议（handoff schema + confidence + evidence） | `src/agents/*`、`src/orchestration/*` |
| M6 | 审查系统升级为对抗式验证（静态检查 + 动态执行 + 证据裁决） | `src/intelligence/verification.rs`、`src/acp/impl/agent.rs` |
| M7 | 记忆策略升级：transient/episodic/semantic/project-state/observation 全策略化 | `src/memory/*` |
| M8 | 记忆漂移控制：相关性评分、陈旧检测、自动降级与清理 | `src/memory/*` |
| M9 | Artifact 契约层：计划、补丁、测试报告、审查结论统一 schema | `src/orchestration/artifact/*` |
| M10 | 评测系统：任务基准集、回放引擎、质量分/稳定性分/成本分 | `src/intelligence/eval/*` |
| M11 | 可观测增强：任务级 trace、节点状态流、失败 taxonomy | `src/observability/*` |
| M12 | 安全与治理：工具白名单、危险操作二次确认、策略审计 | `src/governance/*` |
| M13 | 双轨一致性：Local 与 Server 在核心代理行为上结果一致 | `tests/*` |
| M14 | 发布门禁：全能模式 E2E 通过后才可发布 | `.github/workflows/*` |

---

## 执行路线（分阶段）

### P0 能力边界冻结（架构先行）
- [ ] 定义全能助手模式的边界：规划、执行、审查、记忆、评测
- [ ] 冻结任务图节点协议与 handoff schema
- [ ] 输出 ADR：哪些能力进入核心，哪些作为插件

### P1 Planner/Executor 与任务图落地
- [ ] 实现 Planner（任务分解、依赖推理、预算预估）
- [ ] 实现 Executor（节点调度、失败重试、回滚触发）
- [ ] 增加 TaskGraph 持久化与断点恢复

### P2 工具闭环与权限系统
- [ ] 建立 think-act-observe 主循环
- [ ] 引入工具调用预算：次数、时长、token、并发
- [ ] 引入工具权限策略：按模式/阶段/租户控制

### P3 多代理协作系统
- [ ] 固化角色：planner/coder/tester/reviewer/researcher
- [ ] 每次委派强制包含 objective、constraints、evidence
- [ ] 聚合器支持冲突裁决与置信度加权

### P4 审查与验证升级
- [ ] 接入 deterministic checks（编译、lint、测试、schema）
- [ ] 接入 adversarial checks（反例质询、边界条件）
- [ ] 输出结构化裁决：approve/revise/reject/insufficient-evidence

### P5 记忆策略与漂移防护
- [ ] 实现记忆写入策略（何时写、写到哪类、保留多久）
- [ ] 实现记忆晋升/降级规则与自动 GC
- [ ] 记忆命中必须附证据引用，禁止无证据复述

### P6 评测与可观测体系
- [ ] 建立任务基准库（修复、重构、迁移、审查、发布）
- [ ] 构建回放引擎（可复现输入、可复现工具响应）
- [ ] 输出质量/稳定性/成本三维评分

### P7 双轨集成与发布
- [ ] Local 轨验证：SQLite3 + ACP 下全能模式可运行
- [ ] Server 轨验证：PostgreSQL + pgvector + MCP 下全能模式可运行
- [ ] CI 引入 omnipotent 模式门禁后正式发布

---

## 能力矩阵（对标 OpenClaw 式全能助手）

| 能力维度 | 基线要求 | 验收信号 |
|---|---|---|
| 规划能力 | 自动任务分解与重规划 | 输出可执行任务图且可恢复 |
| 执行能力 | 多轮工具闭环执行 | 非空跑，产出真实补丁与测试结果 |
| 审查能力 | 独立验证与结构化裁决 | 错误可检出、误报率受控 |
| 记忆能力 | 类型化记忆与漂移控制 | 跨会话复用准确且可追溯 |
| 协作能力 | 多角色委派与汇合 | handoff 合法、冲突可裁决 |
| 评测能力 | 可回放可量化 | 基准任务分数稳定上升 |
| 治理能力 | 安全策略可审计 | 危险操作可拦截可追责 |

---

## 风险与应对

| 风险 | 影响 | 应对 |
|---|---|---|
| 任务图复杂度膨胀 | 开发与调试成本上升 | 限制节点类型、先小图后扩展 |
| 工具循环失控 | 成本与时延失控 | 强制预算 + 熔断 + 提前终止条件 |
| 多代理意见冲突 | 输出不稳定 | 统一聚合器 + 证据优先裁决 |
| 记忆污染与陈旧 | 决策偏差 | 漂移检测、再验证、自动降级 |
| 评测数据集失真 | 指标失真 | 固化基准集版本与变更审计 |
| 双轨行为不一致 | 本地与服务器结果分裂 | 行为一致性测试 + 契约测试 |

---

## 完成标准（DoD）

1. 所有阶段清单从 `[ ]` 变为 `[x]`。
2. `omnipotent` 模式在 Local 与 Server 双轨均可运行。
3. 任务图执行支持断点恢复、失败重试、可追溯回放。
4. 工具调用具备预算、权限、超时、幂等保护，且默认安全。
5. 多代理委派采用统一 schema，所有结论有 evidence 引用。
6. 审查输出结构化裁决，且自动回归执行纳入门禁。
7. 评测系统可稳定输出质量/稳定性/成本三维指标。
8. CI/CD 中全能模式门禁持续绿色后才允许发布。

---

## 里程碑建议（建议节奏）

- R1（1~2 周）：P0 + P1
- R2（2~3 周）：P2 + P3
- R3（2~3 周）：P4 + P5
- R4（1~2 周）：P6 + P7
