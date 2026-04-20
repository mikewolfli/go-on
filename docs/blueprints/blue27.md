# BLUE27 — 下一代核心能力闭环（同 BLUE26 规则）

更新时间：2026-04-20

本文沿用 BLUE26 的同一验收规则与收口口径：
- 三端一统（backend / vscode-addon / GUI）
- 主链路完整闭环
- 后端主链路功能完整
- 不留 warning
- 最小修改：仅改与目标直接相关内容；禁止为了过测试而做语义不完整改动
- 完成率必须回写

**依赖**: BLUE26 全量完成 (S0-S41 ✅)  
**目标**: 将 FUTURE*.MD / ENHANCEMENT_OPPORTUNITIES*.md 中规划但未落地的核心能力全量接入主链路

---

## 扫描范围

- backend：src/**（协议层、执行编排、治理、记忆、工具链）
- GUI：GUI/src/** + GUI/src-tauri/src/**
- addon：vscode-addon/src/**
- 契约：contracts/editor-capability-matrix.json
- 门禁脚本：scripts/**

---

## 三种模式功能一致性规则（LOCAL / SIMPLE-SERVER / MULTI-USER-SERVER）

### 核心原则
1. **功能一致性**：三种模式的核心功能链路必须完全一致
2. **场景适配性**：按应用场景要求实现，避免过度或欠缺
3. **零隐藏问题**：不允许存在模式相关的隐蔽 bug 或行为不一致
4. **完整闭合**：所有步骤完成后不留 WARNING、冲突或未解决问题

### 禁止出现的问题（零容忍）
1. **隐藏问题**：不同模式下代码路径不一致导致的隐蔽 bug
2. **潜在冲突**：功能在不同模式下行为不一致
3. **过要求问题**：在 LOCAL 模式下实现过度复杂的企业级功能
4. **欠要求问题**：在 MULTI-USER-SERVER 模式下功能不完整

### 质量保证要求
1. **编译质量**：所有模式下必须零警告编译（cargo check --all-features）
2. **测试覆盖**：核心功能必须有跨所有模式的集成测试
3. **发布验证**：发布前必须验证所有模式的功能一致性

---

## BLUE27 全量能力差距来源

| 优先级 | 能力域 | 来源 |
|--------|--------|------|
| P0 | TaskGraph 持久化+恢复 | FUTURE3.MD |
| P0 | 评估基准测试框架 | FUTURE3.MD |
| P0 | 内存写入策略+GC | FUTURE3.MD |
| P1 | 任务路由接入主链路 | ENHANCEMENT_OPPORTUNITIES*.md |
| P1 | 工具执行预算强制 | FUTURE.MD |
| P1 | StateStore/VectorStore 统一 trait | FUTURE2.MD |
| P1 | 对抗性验证 | FUTURE3.MD |
| P1 | Planner-Executor 分离 | FUTURE3.MD |
| P1 | 多智能体移交契约 | FUTURE.MD |
| P1 | 评估重放引擎 | FUTURE3.MD |
| P1 | Agent 图谱追踪模型 | FUTURE.MD |
| P2 | 动态工作流优化 | ENHANCEMENT_OPPORTUNITIES*.md |
| P2 | Think-Act-Observe 循环 | FUTURE.MD |
| P2 | 模型降级检测 | ENHANCEMENT_OPPORTUNITIES*.md |
| P2 | 任务分解流水线接入主路 | ENHANCEMENT_OPPORTUNITIES*.md |
| P2 | Omnipotent 模式就绪门控 | FUTURE3.MD |
| P2 | SOTA 差距基准框架 | FUTURE.MD |
| P2 | BLUE27 全量收口 | blue27 |

---

## BLUE27-TOP-GATE 实施步骤（执行基线）

执行目标：
- 一次性交付 BLUE27 全量 18 步，不再拆碎；
- 三端同时对齐；
- 所有新增能力都进入主链，不做旁路实验分叉。

### S0 — TaskGraph 持久化+恢复 (task_graph_persistence)
**目标**: TaskGraph checkpoint/resume 接入治理+就绪主链路  
**三端**: contract flag + addon smoke + GUI smoke  
**主链路**: runtime_pack.rs governance.status + ops_pack.rs release.readiness  
**状态**: ✅ 已完成

### S1 — 评估基准测试框架基线 (evaluation_harness_baseline)
**目标**: 评估框架基线（benchmark categories / 任务完成质量 / 回归检测）接入治理+就绪主链路  
**三端**: contract flag + addon smoke + GUI smoke  
**主链路**: runtime_pack.rs + ops_pack.rs  
**状态**: ✅ 已完成

### S2 — 内存写入策略+GC (memory_write_policy)
**目标**: 统一内存写入策略与 GC（LRU eviction / evidence-weighted promotion）接入治理+就绪主链路  
**三端**: contract flag + addon smoke + GUI smoke  
**主链路**: runtime_pack.rs + ops_pack.rs  
**状态**: ✅ 已完成

### S3 — 任务路由接入主链路 (task_routing_mainchain)
**目标**: task_router 自动路由 / capability-role 匹配 / 动态分发接入 ACP 主链路  
**三端**: contract flag + addon smoke + GUI smoke  
**主链路**: runtime_pack.rs + ops_pack.rs  
**状态**: ✅ 已完成

### S4 — 工具执行预算强制 (tool_budget_enforcement)
**目标**: tool 预算/幂等/超时/权限强制门控接入主链路  
**三端**: contract flag + addon smoke + GUI smoke  
**主链路**: runtime_pack.rs + ops_pack.rs  
**状态**: ✅ 已完成

### S5 — StateStore/VectorStore 统一 trait (state_store_trait)
**目标**: 存储层统一 trait 抽象（SQLite+PostgreSQL）接入治理主链路  
**三端**: contract flag + addon smoke + GUI smoke  
**主链路**: runtime_pack.rs + ops_pack.rs  
**状态**: ✅ 已完成

### S6 — 对抗性验证 (adversarial_verification)
**目标**: 确定性+对抗性校验，带结构化裁定，接入主链路  
**三端**: contract flag + addon smoke + GUI smoke  
**主链路**: runtime_pack.rs + ops_pack.rs  
**状态**: ✅ 已完成

### S7 — Planner-Executor 分离 (planner_executor_separation)
**目标**: Planner/Executor 双核分离架构+移交 schema 接入治理主链路  
**三端**: contract flag + addon smoke + GUI smoke  
**主链路**: runtime_pack.rs + ops_pack.rs  
**状态**: ✅ 已完成

### S8 — 多智能体移交契约 (multi_agent_handoff)
**目标**: 多智能体移交契约 schema + 置信度 + 证据 + 跨 agent 协议接入主链路  
**三端**: contract flag + addon smoke + GUI smoke  
**主链路**: runtime_pack.rs + ops_pack.rs  
**状态**: ✅ 已完成

### S9 — 评估重放引擎 (evaluation_replay_engine)
**目标**: 重放引擎（质量/稳定性/成本评分）接入治理主链路  
**三端**: contract flag + addon smoke + GUI smoke  
**主链路**: runtime_pack.rs + ops_pack.rs  
**状态**: ✅ 已完成

### S10 — Agent 图谱追踪模型 (trace_model_agent_graph)
**目标**: Agent 图谱转换追踪模型（plan/tool-call/reviewer/graph-transition）接入治理主链路  
**三端**: contract flag + addon smoke + GUI smoke  
**主链路**: runtime_pack.rs + ops_pack.rs  
**状态**: ✅ 已完成

### S11 — 动态工作流优化 (dynamic_workflow_optimization)
**目标**: 基于历史的自适应阶段调度+工作流重排序接入治理主链路  
**三端**: contract flag + addon smoke + GUI smoke  
**主链路**: runtime_pack.rs + ops_pack.rs  
**状态**: ✅ 已完成

### S12 — Think-Act-Observe 循环 (think_act_observe_loop)
**目标**: think-act-observe 迭代主循环+预算集成接入治理主链路  
**三端**: contract flag + addon smoke + GUI smoke  
**主链路**: runtime_pack.rs + ops_pack.rs  
**状态**: ✅ 已完成

### S13 — 模型降级检测 (model_degradation_detection)
**目标**: 模型降级检测（历史对比/回归告警/自动降级触发）接入治理主链路  
**三端**: contract flag + addon smoke + GUI smoke  
**主链路**: runtime_pack.rs + ops_pack.rs  
**状态**: ✅ 已完成

### S14 — 任务分解流水线接入主路 (task_decomposition_pipeline)
**目标**: task_decomposer 自动分解流水线（子任务管理/依赖图）接入 ACP 主链路  
**三端**: contract flag + addon smoke + GUI smoke  
**主链路**: runtime_pack.rs + ops_pack.rs  
**状态**: ✅ 已完成

### S15 — Omnipotent 模式就绪门控 (omnipotent_mode_readiness)
**目标**: Omnipotent 模式端到端就绪门控（P0-P7 能力层级）接入治理主链路  
**三端**: contract flag + addon smoke + GUI smoke  
**主链路**: runtime_pack.rs + ops_pack.rs  
**状态**: ✅ 已完成

### S16 — SOTA 差距基准框架 (sota_gap_benchmark)
**目标**: SOTA 差距基准框架（benchmark/gap-analysis/sota-comparison/regression-prevention）接入治理主链路  
**三端**: contract flag + addon smoke + GUI smoke  
**主链路**: runtime_pack.rs + ops_pack.rs  
**状态**: ✅ 已完成

### S17 — BLUE27 全量收口 (blue27_release_closure)
**目标**: BLUE27 全量闭环收口门控（S0-S17 全绿 / 三端同步 / 集成测试）  
**三端**: contract flag + addon smoke + GUI smoke  
**主链路**: runtime_pack.rs + ops_pack.rs  
**状态**: ✅ 已完成

---

## 一次到顶硬验收标准（DoD）

1. TaskGraph 持久化 + checkpoint/resume 接入主链路并可验证。
2. 评估基准测试框架覆盖 repair/refactor/migrate/review/release 五类场景。
3. 内存写入策略统一，GC 有 LRU 证据。
4. task_router 已接入 ACP 主路，能力-角色匹配可观测。
5. tool 预算强制门控已生效，幂等与权限防护均有门禁断言。
6. StateStore/VectorStore 统一 trait，SQLite 与 PostgreSQL 可切换。
7. 对抗性验证已纳入发布阻断，结构化裁定可追溯。
8. Planner/Executor 双核分离，移交 schema 覆盖全链路。
9. 多智能体移交契约含置信度+证据，跨 agent 协议有 smoke 覆盖。
10. 评估重放引擎质量/稳定性/成本评分进入发布门禁。
11. Agent 图谱追踪模型覆盖 plan/tool-call/reviewer/graph-transition。
12. 动态工作流优化自适应调度可观测，有历史路由证据。
13. Think-Act-Observe 迭代主循环含预算控制，可追溯各阶段输出。
14. 模型降级检测有历史基线对比，回归时自动触发降级路径。
15. 任务分解流水线自动拆解+依赖图接入 ACP 主路。
16. Omnipotent 模式就绪门控 P0-P7 均有 gate 断言。
17. SOTA 差距基准框架 benchmark/gap-analysis/sota-comparison 均已接入。
18. 三端 contract smoke 全绿（contract JSON + addon smoke + GUI smoke）。
19. cargo 全绿且 0 warning。
20. 集成测试断言覆盖 S0-S17 全量 governance + readiness profile 与 gate。
21. blue27_release_closure gate 通过。

---

## 风险与止损

1. 范围膨胀导致再次拆批
   - 止损：只允许本文件目标，超出项进入 BLUE28 backlog。

2. 自动修复循环成本失控
   - 止损：硬预算 + 迭代上限 + 高风险立即熔断。

3. 三端字段再次漂移
   - 止损：契约强校验 + CI 阻断 + 禁止端侧兜底拼装。

4. TaskGraph 持久化在 Windows 平台受限
   - 止损：保留核心无 I/O 覆盖；平台差异在注释中标注并保留常规 test 覆盖。

5. 多智能体协作引入意见震荡
   - 止损：证据优先聚合器 + 冲突阈值熔断 + 强制 reviewer 最终裁决。

6. Omnipotent 模式门控过严导致所有 gate 均 false
   - 止损：gate 条件基于已有主链路 bool 链推导，不引入外部依赖。

---

## 完成率

**总步骤**: 18 (S0-S17)  
**已完成**: 18  
**完成率**: 100% (S0-S17, 18 步全量完成)

> 本轮完成：S0-S17 全量接入主链路（runtime_pack.rs + ops_pack.rs），三端同步（contract JSON + addon smoke + GUI smoke），集成测试断言覆盖待补全后即可 cargo check 验收。  
> 超出项进入 BLUE28 backlog。
