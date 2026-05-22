# BLUE43 - SRC 多智能体编排深度扫描与钢铁侠级就绪蓝图

更新日期：2026-05-22

> 本文承接 BLUE42，对 `src/` 进行新一轮“深度+广度”扫描。
> 核心问题：作为多智能体编排系统，是否已经达到“钢铁侠战衣级”速度、流畅度与智能性，用于问题求解与任务执行？
> 结论基于当前可执行代码路径与指标语义，而非命名或注释。

---

## 0. 核心规则（与 BLUE42 一致）

BLUE42 的全部约束继续作为 BLUE43 的硬门槛：

1. 5 协议全闭环：auto、acp stdio、acp http、mcp stdio、mcp http。
2. 3 profile 全闭环：profile-local、profile-simple-server、profile-multi-users-server。
3. 新增代码注释仅使用英文。
4. 用户可见字符串完整 i18n。
5. 每个事项必须闭环：编译通过、零告警、governance.status 可见、health 可观测、集成测试。
6. Backend/GUI/vscode-addon 协议一致性。
7. clippy 严格零告警。
8. 每轮回写完成率。
9. 未验证不得任意漂移计划。
10. 主路径优先闭环，不允许占位行为。
11. 架构整洁，避免继续单体膨胀。

---

## 1. 扫描范围与方法

### 1.1 广度覆盖（核心代表路径）

本次覆盖 `src/` 中编排关键区域：

1. ACP 主路径与自治桥接：
   - `src/acp/impl/chat.rs`
   - `src/acp/helpers/autonomy_loop.rs`
   - `src/acp/helpers/autonomy_loop_adapter.rs`
   - `src/acp/helpers/agent_selector.rs`
   - `src/acp/helpers/cache_strategy.rs`
   - `src/acp/helpers/execution_intelligence.rs`
2. Planner/图执行/Council：
   - `src/orchestration/planner_executor.rs`
   - `src/orchestration/execution_graph.rs`
   - `src/orchestration/dag_driver.rs`
   - `src/orchestration/council/council.rs`
3. 智能与学习：
   - `src/intelligence/capability_bus/core.rs`
   - `src/intelligence/metacognitive.rs`
   - `src/intelligence/world_model.rs`
   - `src/intelligence/self_model.rs`
   - `src/intelligence/continuous_learning.rs`
4. 运行态治理指标：
   - `src/acp/impl/request/runtime_pack.rs`
5. E2E 性能基线现实性校验：
   - `tests/autonomy_benchmark.rs`

### 1.2 深度证据锚点

1. `process_chat_request` 体量仍然很大：`src/acp/impl/chat.rs` 为 6684 行。
2. Planner 在 `Planner::plan` 中仍输出固定 3 步模板。
3. DAG 执行链路已接入，但自治循环对 DAG 成功节点结果的映射仍将输出压扁为“空 JSON 占位载荷”。
4. `autonomy_perf.p95_latency_ms` 当前仍映射到平均请求时延指标。
5. `tests/autonomy_benchmark.rs` 仍以合成/微模拟基准为主。

---

## 2. 钢铁侠级就绪结论

### 2.1 总体评分

| 维度 | 评分 | 评估 |
|:--|:--:|:--|
| 架构完整度 | 8.5/10 | 关键组件已具备，且已进入运行主路径。 |
| 执行速度 | 6.0/10 | 相比 BLUE42 基线有提升，但关键指标语义与规划现实性限制了“真实加速”的可信度。 |
| 交互流畅度 | 6.5/10 | 多轮链路可运行（含 follow-up 与 fallback），但循环语义仍有可避免摩擦。 |
| 智能深度 | 6.0/10 | 元认知/世界模型/自模型/学习中心已存在，但对决策的实质影响仍偏浅层启发式。 |
| 可观测可信度 | 6.0/10 | 遥测面丰富，但部分关键 KPI 仍存在语义近似。 |

**结论**：尚未达到钢铁侠战衣级。

当前更接近“强装甲骨架 + 部分同步控制面”。
系统已可解决大量任务，但在复杂场景下尚未稳定达到“低延迟、高自适应、高置信”的编排执行状态。

---

## 3. 关键差距矩阵（速度 / 流畅度 / 智能）

### GAP-43-01（P0）- Planner 现实性瓶颈

现状：
1. `Planner::plan` 仍对全部任务输出固定 3 步工作流。

影响：
1. 复杂任务无法生成贴近依赖关系的 DAG。
2. 时延与轮次无法显著受益于规划质量提升。

### GAP-43-02（P0）- DAG 观察信息损失

现状：
1. 自治 DAG 路径中，成功工具节点被转成通用 `LoopDecision::Complete` 且结果载荷为空，导致下游观察保真度下降。

影响：
1. 工具密集任务的重规划质量与最终答复质量下降。
2. 多轮智能循环失去可执行证据。

### GAP-43-03（P0）- governance.status 指标语义漂移

现状：
1. `autonomy_perf.p95_latency_ms` 目前由平均请求时延支撑。

影响：
1. SLA 与优化决策可能偏差。
2. 性能回归可能被误判或漏判。

### GAP-43-04（P1）- 主路径单体压力仍高

现状：
1. `process_chat_request` 仍然巨大且混合多类职责。

影响：
1. 变更风险与验证成本持续偏高。
2. 流畅性优化仍需在单一热点文件中高成本改动。

### GAP-43-05（P1）- Agent 切换语义仍偏粗粒度

现状：
1. 虽有 reroute 记录与候选重试，但循环内切换仍以失败触发为主，策略较简单。

影响：
1. 自适应智能偏“被动反应”，预测性不足。

### GAP-43-06（P1）- CapabilityBus 最终选择仍偏重声誉

现状：
1. `select_best_agent` 主要按 reputation snapshot 排序（未知 agent 采用中性回退）。

影响：
1. 任务形态与近期执行证据在最终路由中的权重不足。

### GAP-43-07（P2）- 性能基准现实性不足

现状：
1. `tests/autonomy_benchmark.rs` 主要验证微观行为与模拟循环。

影响：
1. CI 难以有效防守真实编排负载下的用户感知回归（时延/流畅度）。

---

## 4. 能否达到钢铁侠级？（可行性判断）

### 4.1 简答

可以，且在当前代码基座内可达。

### 4.2 必要闭环条件

只有当以下条件同时成立，系统才可判定“接近钢铁侠级”：

1. Planner 产出任务自适应 DAG（而非固定模板）。
2. DAG 执行完整保留工具输出并注入 observe/replan 链路。
3. 治理 KPI 语义正确（真实 p95、真实并行利用率）。
4. 循环内 reroute 采用预测评分，而非仅失败后回退。
5. 端到端基准门禁可捕获现实场景下 >15% 回归。

---

## 5. BLUE43 具体改进计划（更细化、可执行）

### Step 1（P0）：Planner-to-DAG 真实规划引擎

目标：
1. 以自适应分解与依赖图输出，替换固定 3 步 Planner。

实施：
1. 在 `src/orchestration/planner_executor.rs` 新增 `Planner::plan_to_dag(task, context)`。
2. 基于任务特征与所需能力构建依赖边。
3. 对可独立子任务输出显式并行组。
4. 保持向后兼容：旧 `plan()` 映射到 `plan_to_dag()` 默认 profile。

验收：
1. 测试中不再假设固定 3 步。
2. 至少 3 个复杂度层级产出结构差异化计划。
3. governance payload 暴露 DAG 宽度/深度指标。

### Step 2（P0）：DAG 执行证据保真

目标：
1. 确保自治 DAG 路径保留真实工具输出用于 observe/replan。

实施：
1. 在 `src/orchestration/dag_driver.rs` 的返回结构中，为 `DagNodeResult` 增加可选 `tool_output` 载荷。
2. 在 `src/acp/helpers/autonomy_loop.rs` 中，将 DAG 节点输出映射为完整 `LoopDecision::Complete` 结果载荷。
3. 失败节点保留详细失败载荷，供反思诊断使用。

验收：
1. 成功 DAG 工具调用可在重规划提示中看到非空结构化证据。
2. E2E 测试可证明：去除证据后答复质量下降，恢复证据后质量回升。

### Step 3（P0）：修正 autonomy_perf 指标语义

目标：
1. 让 governance.status 的性能指标可用于真实治理决策。

实施：
1. 在 metrics 层增加滚动时延直方图（或显式百分位估计器），计算真实 p95。
2. 更新 `src/acp/impl/request/runtime_pack.rs`，将 `autonomy_perf.p95_latency_ms` 映射到百分位来源。
3. 轮次统计从 autonomy report 直接提取，不再依赖间接计数。

验收：
1. 在偏斜分布样本下，`p95_latency_ms` 与 avg 明显不同。
2. 校验测试包含偏斜时延样本并验证百分位计算正确。

### Step 4（P1）：继续拆分 chat 编排缝合点

目标：
1. 降低 `process_chat_request` 的热点耦合。

实施：
1. 在 `src/acp/helpers/` 提取 `review_gate`、`vote_orchestration`、`response_assembler` 辅助模块。
2. 维持严格 I/O 合同，避免行为漂移。
3. 为每个 helper 增加单测，并增加 1 个集成快照测试校验 payload 等价。

验收：
1. 本阶段将 `process_chat_request` 降至 5000 行以下。
2. 对外 API 响应 schema 不变。

### Step 5（P1）：循环内预测式 reroute 评分

目标：
1. 从“失败后切换”升级为“预测收益驱动切换”。

实施：
1. 在 autonomy loop 增加复合评分：声誉 + 任务成功率 + 轮次健康度 + 工具错误特征。
2. 当预期收益超阈值时切换，而非仅在空响应时切换。
3. 记录更细切换原因码（`predictive_gain`、`failure_recovery`、`budget_guard`）。

验收：
1. 运行指标中可观察到至少 3 类切换原因。
2. 复杂场景基准下 completion ratio 相比基线提升。

### Step 6（P1）：CapabilityBus 多因子选择

目标：
1. 提升 agent 选择的智能深度。

实施：
1. 扩展 `src/intelligence/capability_bus/core.rs` 的 `select_best_agent`，纳入任务特征与近期结果因子。
2. 增加 reputation / recency / task-fit 可配置权重。
3. 在 decision event detail 导出候选打分分解。

验收：
1. 决策事件包含每个 agent 的分项得分。
2. 路由变化可与任务类型变化形成一致性。

### Step 7（P1）：现实型 E2E 编排基准套件

目标：
1. 将基准从微模拟升级为可回放的真实编排场景。

实施：
1. 扩展 `tests/autonomy_benchmark.rs`，加入可回放场景：
   - 多工具串行
   - 并行 fan-out + join
   - 需要 reroute 的失败恢复
2. 采集 wall time、rounds、fan-out、最终成功率。
3. 设置 CI 门禁：p95 回归 >15% 或轮次膨胀 >20% 直接失败。

验收：
1. 基准在 CI 中可确定性运行。
2. 回归门禁可阻断劣化提交。

### Step 8（P2）：元认知动作链路加固

目标：
1. 让反思不止“被记录”，而是“驱动动作”。

实施：
1. 将高严重度观察模式转为自治循环可消费的纠偏动作提示。
2. 增加动作应用计数与成功率指标。

验收：
1. post-check 失败可实质影响下一轮策略。
2. 可观测面暴露动作有效性比例。

### Step 9（P2）：跨入口行为一致性

目标：
1. 降低 ACP 路径与 CLI 路径语义分叉。

实施：
1. CLI 尽可能复用共享自治运行时合同。
2. 增加工具调用处理与 follow-up 链路语义的一致性校验。

验收：
1. 同一场景在 ACP/CLI 下的 stop_reason 与 round 数保持同量级边界一致。

---

## 6. 实施顺序与里程碑

### Milestone A（速度基础层）

1. Step 1
2. Step 2
3. Step 3

预期收益：
1. 真实吞吐与时延收益变得可测且可信。

### Milestone B（流畅性与可靠性）

1. Step 4
2. Step 5
3. Step 7

预期收益：
1. 复杂任务下编排摩擦降低，稳定性提升。

### Milestone C（智能成熟度）

1. Step 6
2. Step 8
3. Step 9

预期收益：
1. 自适应行为增强，跨路径一致性提升。

---

## 7. 目标指标（BLUE43）

| 指标 | 当前（扫描） | BLUE43 目标 | 门禁 |
|:--|:--:|:--:|:--:|
| Planner 结构多样性 | 低（固定 3 步） | 高（任务自适应 DAG） | 必选 |
| DAG 证据完整性 | 部分 | 完整 | 必选 |
| p95 指标语义正确性 | 部分 | 真实百分位 | 必选 |
| `process_chat_request` 规模 | 6684 行 | <5000（本阶段） | 必选 |
| 预测式 reroute 使用率 | 低 | 中-高 | 必选 |
| E2E 基准现实性 | 低-中 | 高 | 必选 |
| 多智能体协同质量 | 中 | 高 | 必选 |

### 7.1 MCP 专项闭环指标（新增）

目标：
1. 将 MCP 从“原则约束”升级为“独立可验收交付物”，单独追踪到 100%。

MCP 关键能力清单（mcp stdio / mcp http 必须都通过）：
1. 协议握手与能力声明一致。
2. 工具注册、发现、调用、错误返回语义一致。
3. 流式响应/分块响应在两种传输模式下行为一致。
4. 超时、重试、取消语义一致且可观测。
5. 鉴权与租户隔离（multi-users profile）一致。

MCP 门禁指标：
1. mcp stdio 套件通过率 = 100%。
2. mcp http 套件通过率 = 100%。
3. MCP 协议兼容性回归用例通过率 = 100%。
4. MCP 关键路径 p95 不劣化（相对基线回归阈值 <= 15%）。
5. MCP 错误码映射一致性 = 100%。

---

## 8. 完成率追踪

### 8.1 本轮产出

1. SRC 深+广扫描：100%
2. 钢铁侠级就绪评估：100%
3. BLUE43 细化执行计划：100%

### 8.2 BLUE43 执行表（初始）

| Step | 内容 | 完成率 | 备注 |
|:--:|--|:--:|--|
| 1 | Planner-to-DAG 真实规划引擎 | 0% | 待实现 |
| 2 | DAG 执行证据保真 | 0% | 待实现 |
| 3 | 修正 autonomy_perf 指标语义 | 0% | 待实现 |
| 4 | 继续拆分 chat 编排缝合点 | 0% | 待实现 |
| 5 | 预测式 reroute 评分 | 0% | 待实现 |
| 6 | CapabilityBus 多因子选择 | 0% | 待实现 |
| 7 | 现实型 E2E 基准套件 | 0% | 待实现 |
| 8 | 元认知动作链路加固 | 0% | 待实现 |
| 9 | ACP/CLI 行为一致性 | 0% | 待实现 |

### 8.3 从“还差一半”到“100%闭环”的冲刺计划

说明：
1. 当前约处于 50% 左右（具备完整骨架与主链路，但关键语义与门禁尚未闭合）。
2. 以下将“剩余约 50%”拆成 4 个可验收冲刺包，合计补齐至 100%。

| 冲刺包 | 覆盖 Step | 增量完成率 | 累计完成率 | 必过门禁 |
|:--|:--|:--:|:--:|:--|
| Sprint-S1（语义纠偏） | 1,2,3 | +20% | 70% | 真实 p95、DAG 证据非空、Planner 非固定模板 |
| Sprint-S2（主链路降耦） | 4,5 | +12% | 82% | chat 主函数降至 <5000 行、预测式 reroute 生效 |
| Sprint-S3（选择与基准） | 6,7 | +10% | 92% | 多因子选路打分可观测、E2E 回放基准进 CI |
| Sprint-S4（一致性收口） | 8,9 | +8% | 100% | 元认知动作闭环、ACP/CLI 行为同边界 |

### 8.4 100% 判定标准（必须同时满足）

1. 代码闭环：Step1-9 全部实现并通过对应单测/集测。
2. 指标闭环：
   - `autonomy_perf.p95_latency_ms` 为真实百分位。
   - `avg_rounds_per_request` 基于真实自治报告统计。
   - reroute 原因码至少 3 类稳定可观测。
3. 质量门禁闭环：
   - `cargo check` 通过。
   - `cargo clippy` 零告警（含 3 profile）。
   - 基准门禁生效：p95 回归 >15% 或轮次膨胀 >20% 即失败。
4. 协议与入口闭环：
   - 5 协议行为一致通过验收。
   - ACP/CLI 同场景 stop_reason 与 round 边界一致。
5. 文档与治理闭环：
   - governance.status 中新增字段文档齐备。
   - BLUE43 完成率表回写到 100%，并附每步证据链接/测试记录。
6. MCP 专项闭环：
   - mcp stdio / mcp http 两条链路测试均为 100% 通过。
   - MCP 协议兼容性、错误码映射、一致性回归全部通过。

### 8.5 建议回写节奏（用于每轮更新）

1. 每完成一个 Step，更新执行表对应完成率与证据。
2. 每完成一个 Sprint，更新累计完成率（70/82/92/100）。
3. 若门禁未通过，不提升累计完成率，仅记录阻塞与修复计划。

### 8.6 MCP 专项完成率追踪（新增）

| 项目 | 当前完成率 | 目标完成率 | 证据 |
|:--|:--:|:--:|:--|
| MCP-1 协议握手/能力声明一致 | 0% | 100% | mcp 协议集成测试报告 |
| MCP-2 工具调用语义一致（stdio/http） | 0% | 100% | 双通道一致性测试 |
| MCP-3 流式与分块响应一致 | 0% | 100% | 流式回归测试 |
| MCP-4 超时/重试/取消语义一致 | 0% | 100% | 稳定性与容错测试 |
| MCP-5 鉴权与隔离（多用户） | 0% | 100% | multi-users 安全测试 |
| MCP-6 协议兼容与错误码映射 | 0% | 100% | 兼容矩阵与错误码回归 |

回写规则：
1. 任一 MCP 项目未达 100%，总完成率不得标记为 100%。
2. MCP 项目证据必须在同轮附上测试命令与通过摘要。

---

## 9. 结论

1. 系统基础强、架构接近目标，但端到端执行智能尚未达到钢铁侠战衣级。
2. 当前最高杠杆并非新增模块，而是提高现有模块的语义正确性与闭环执行强度。
3. BLUE43 方案按“可增量、可验证、可门禁”设计，可在低歧义前提下持续推进闭环。
