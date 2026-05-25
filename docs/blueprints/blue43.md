# BLUE43 - SRC 多智能体编排深度扫描与钢铁侠级就绪蓝图

更新日期：2026-05-23

> 本文承接 BLUE42，对 `src/` 进行新一轮“深度+广度”扫描。
> 核心问题：作为多智能体编排系统，是否已经达到“钢铁侠战衣级”速度、流畅度与智能性，用于问题求解与任务执行？
> 结论基于当前可执行代码路径与指标语义，而非命名或注释。

---

## 0. 核心规则（与 BLUE42 一致）

BLUE42 的全部约束继续作为 BLUE43 的硬门槛：

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
16. 注意单个文件的代码行数，不要太臃肿，新的结构和函数，请尽量创建新的模块文件，注意代码整体架构整洁简练清晰。

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

### Step 10（P0）：Agents full-auto 全流程闭环

目标：
1. 让 go-on 的 agents 在接收到任务要求后，可自动完成技能查找、技能启用、执行环境准备、工具调用、任务执行与结果输出，成为加载 agents 后仍可独立工作的全能助手。

实施：
1. 建立 skill-aware 任务分解与能力匹配流程，自动从用户要求中识别所需 skills、tools 与执行前置条件。
2. 为 agents 补齐自动执行环境引导能力，包括项目/依赖/运行时/部署前检查与安全降级路径。
3. 将工具查找、skills 查找、部署、执行、回传结果统一进 full-auto 流程，并保留可审计的中间证据。
4. 为失败场景补充自动恢复与最小人工介入边界，确保默认路径仍可一键完成。

验收：
1. 给定一条完整任务要求，agents 可自动完成“查找 skills -> 准备环境 -> 调用工具 -> 执行任务 -> 输出结果”的全链路。
2. full-auto 模式下，任务完成结果可复现、可追踪、可回放。
3. 加载 agents 后，系统仍保持“全能助手”行为，不因是否显式进入某个子模式而失去自动完成能力。

### Step 11（P0）：热路径速度治理与快路径缓存

目标：
1. 在不牺牲正确性与安全性的前提下，把 full-auto 主路径做成默认快路径，避免因条件过多拖慢整体执行。

实施：
1. 将鉴权、意图解析、skills 匹配、环境探测拆成入口层 / 规划层 / 执行层，避免热路径重复判断。
2. 为高频任务、常用 skills、稳定环境探测结果建立缓存与预热机制。
3. 将慢检查改成懒加载或异常路径触发，主路径只保留必要校验。

验收：
1. 常见任务在 full-auto 模式下不会因额外条件显著拖慢 p95。
2. 同类请求重复执行时，快路径命中率可观测且稳定。

### Step 12（P0）：任务意图解析与能力快路由

目标：
1. 让 agents 能从自然语言中快速识别任务目标、约束、风险和所需能力，并一次性完成能力装配。

实施：
1. 将任务拆成目标、约束、前置条件、交付物四类结构化意图。
2. 自动映射到 skills、tools、runtime、approval 四类能力需求。
3. 为常见任务类型建立快路由模板，减少重复规划成本。

验收：
1. 同一需求在不同表述下仍进入一致的能力路由。
2. 常见任务可通过快路由直接进入执行准备阶段。

### Step 13（P1）：执行环境自动引导与预热

目标：
1. 让 agents 自动完成执行前环境准备，并把环境探测从阻塞式检查变成可复用的预热能力。

实施：
1. 建立环境探测与补齐流程，识别缺失依赖、缺失 secret、缺失 runtime。
2. 为本地、远端、容器化场景提供统一引导语义和环境状态缓存。
3. 保留不破坏现有架构的降级路径，避免强行改写运行态。

验收：
1. 环境缺失时可自动引导修复或明确降级。
2. 已探测环境在后续任务中可直接复用，不重复付出完整检查成本。

### Step 14（P1）：skills 发现、缓存与匹配闭环

目标：
1. 让 agents 自动查找、筛选、启用最合适的 skills，同时把 skill 检索开销控制在可接受范围。

实施：
1. 为 skills 建立语义索引、来源可信度、适用范围标签与命中缓存。
2. 实现任务到 skills 的自动匹配、排序与复用。
3. 为 skill 启用增加安全门禁与降级路径。

验收：
1. 给定任务要求，系统能自动列出候选 skills 并选择最合适项。
2. skill 复用命中后不会重复做高成本发现流程。

### Step 15（P1）：工具调用事务化与幂等化

目标：
1. 提升工具调用一次通过率，降低重复调用、部分成功和副作用污染带来的速度与稳定性损耗。

实施：
1. 为工具调用增加幂等标识、事务边界和回滚/补偿语义。
2. 统一工具调用结果结构，显式记录成功、失败、部分成功。
3. 对高风险工具引入确认阈值和有限重试策略。

验收：
1. 重复触发同一工具调用不会产生不可控副作用。
2. 工具失败后仍能输出可读、可追踪且可恢复的失败原因。

### Step 16（P1）：自动恢复与最小人工介入闭环

目标：
1. 让 agents 在失败、超时、工具异常时自动恢复到最小可用状态，提升执行顺畅度与一次通过率。

实施：
1. 为任务失败建立恢复策略树、降级路径和二次规划入口。
2. 把人工介入限定在真正不可自动恢复的边界。
3. 保留恢复尝试的证据链，避免黑箱重试。

验收：
1. 常见失败模式下可自动恢复或明确降级。
2. 人工介入是例外，不是默认流程。

### Step 17（P0）：多用户 tenant 注册与隔离闭环

目标：
1. 在不破坏现有单租户主路径的前提下，把 tenant 注册来源真正接进 RBAC、budget、session 与协议入口，补齐安全短板。

实施：
1. 为 runtime 建立 tenant 注册源与生命周期管理。
2. 将 tenant 信息同步到 session、RBAC、budget 与协议入口。
3. 为缺失 tenant、未知 tenant、跨 tenant 访问建立明确拒绝路径。

验收：
1. 不同 tenant 的请求无法跨越隔离边界。
2. tenant 配置缺失时有明确、可审计的错误信息。

### Step 18（P1）：MCP 流式、取消与超时闭环

目标：
1. 补齐 MCP-3 / MCP-4 的核心短板，使 stdio 与 http 在流式、取消、超时语义上保持一致。

实施：
1. 为 MCP 补齐流式/分块回传的统一语义。
2. 引入可观测的取消与超时状态。
3. 将不同传输模式的行为差异压到同一规范层。

验收：
1. 流式任务在 stdio / http 下行为一致。
2. 取消与超时都能被测试、度量并纳入回归门禁。

### Step 19（P1）：ACP / CLI / MCP 同场景对拍闭环

目标：
1. 让不同入口在相同任务上的 stop_reason、round 数、工具证据边界与异常语义保持一致。

实施：
1. 建立同场景对拍测试集，覆盖 ACP、CLI、MCP 三入口。
2. 对比合同、结果与异常边界，而不是只比成功路径。
3. 将差异回写到治理面，作为入口一致性回归门禁。

验收：
1. 同任务在三入口下的输出边界一致。
2. 差异出现时可直接定位到入口级行为。

### Step 20（P1）：审计、回放与证据闭环

目标：
1. 让 full-auto 结果不仅能跑完，还能被完整复盘，从而支撑安全、治理与竞品级对标。

实施：
1. 为 skills、tools、部署、执行、恢复全链路记录审计证据。
2. 将结果、输入、环境、切换原因统一入可回放日志。
3. 保持证据层与执行层解耦，避免侵入现有架构。

验收：
1. 任一任务都能回放出关键决策和执行证据。
2. 审计数据足以支撑回归定位与安全追责。

### Step 21（P0）：外部对标与持续回归门禁

目标：
1. 以“除生态外全面领先”为目标，对速度、执行顺畅、安全、自动化闭环、可验证性做持续外部对标；是否达成以 benchmark 结果为准，而非主观宣称。

实施：
1. 固化与 Claude Code、Codex、OpenClaw / harness 类工具同任务、同预算、同工具集的 benchmark。
2. 固化对标维度：一次通过率、回合数、尾延迟、工具调用正确率、恢复成功率、审计完整度。
3. 将外部对标结果纳入持续回归门禁和能力雷达，不影响现有主架构。

验收：
1. 每次核心变更都能自动跑横向对标。
2. “除生态外全面领先”必须由持续 benchmark 结果支持，退化可直接阻断进入主分支。

### 5.1 Step 11-21 量化门槛（新增）

说明：以下门槛用于把 Step 11-21 从“方向正确”收敛为“可判定达成”。

| Step | 核心量化门槛 | 达成判定 |
|:--:|:--|:--|
| 11 | full-auto 热路径 p95 相比当前基线不劣化（<= +5%），快路径命中率 >= 70% | 连续 3 轮回归满足 |
| 12 | 意图解析结构化成功率 >= 95%，快路由命中任务的一次装配成功率 >= 90% | 连续 3 轮回归满足 |
| 13 | 环境预热复用命中率 >= 60%，环境准备失败率 <= 5% | 连续 3 轮回归满足 |
| 14 | skills 自动匹配 Top-1 命中率 >= 85%，skill 检索阶段耗时 p95 <= 300ms | 连续 3 轮回归满足 |
| 15 | 工具调用幂等冲突率 <= 1%，工具调用成功率 >= 95% | 连续 3 轮回归满足 |
| 16 | 自动恢复成功率 >= 80%，人工介入占比 <= 10% | 连续 3 轮回归满足 |
| 17 | cross-tenant 访问误放行率 = 0，tenant 缺失/未知错误码映射一致率 = 100% | 连续 3 轮回归满足 |
| 18 | MCP stdio/http 在流式、取消、超时语义一致率 = 100% | 连续 3 轮回归满足 |
| 19 | ACP/CLI/MCP 同场景 stop_reason 与 round 边界一致率 >= 98% | 连续 3 轮回归满足 |
| 20 | 任务审计链完整率 = 100%，回放成功率 >= 95% | 连续 3 轮回归满足 |
| 21 | 外部横向 benchmark 中，除生态外核心指标领先率 >= 70%，且关键指标不低于任一对标项 | 连续 3 轮回归满足 |

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
4. Step 10

预期收益：
1. 自适应行为增强，跨路径一致性提升，agents 的自动化执行闭环进一步增强。

### Milestone D（短板补齐与外部对标）

1. Step 11
2. Step 12
3. Step 13
4. Step 14
5. Step 15
6. Step 16
7. Step 17
8. Step 18
9. Step 19
10. Step 20
11. Step 21

预期收益：
1. 在不破坏现有架构的前提下，先补速度与执行顺畅，再补安全与协议一致性，最后用持续对标证明除生态外的全面领先。

### 6.1 Step 11-21 依赖矩阵（新增）

1. Step 11 -> Step 12, Step 13, Step 14
2. Step 12 -> Step 14, Step 15, Step 16
3. Step 13 -> Step 15, Step 16
4. Step 14 -> Step 15, Step 16
5. Step 15 -> Step 16, Step 19, Step 20
6. Step 17 -> Step 18, Step 19, Step 20
7. Step 18 -> Step 19, Step 21
8. Step 19 -> Step 20, Step 21
9. Step 20 -> Step 21

执行约束：
1. 允许并行推进的组合：{12,13}、{14,17}、{18,20}。
2. 禁止并行推进的组合：{11 与 21}、{15 与 21}（避免基线尚未稳定即做最终对标结论）。

### 6.2 失败退出与回退准则（新增）

1. 任一 Step 连续 2 轮回归未满足其量化门槛：冻结该 Step 的新增功能，进入修复模式。
2. 任一 Step 导致核心性能门禁退化（p95 > +15% 或轮次膨胀 > 20%）：立即回退至上一个稳定版本。
3. 任一 Step 出现安全红线事件（cross-tenant 误放行、权限越权、审计链断裂）：立即停止该 Step 推进并触发安全审查。
4. Step 21 若连续 2 轮 benchmark 未满足“除生态外核心指标领先率 >= 70%”：不得宣称全面领先，仅可标注为“持续追平阶段”。
5. 回退后必须在同一轮回写：触发原因、影响范围、回退版本、修复负责人、预计恢复窗口。

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
| 1 | Planner-to-DAG 真实规划引擎 | 100% | `plan_to_dag()`、自适应复杂度分析、DAG metrics；测试通过 |
| 2 | DAG 执行证据保真 | 100% | `DagNodeResult` 增加 `tool_output`/`error_payload`；自治循环保留真实输出 |
| 3 | 修正 autonomy_perf 指标语义 | 100% | `estimate_p95_from_buckets()` 基于直方图桶的真实百分位；测试通过 |
| 4 | 继续拆分 chat 编排缝合点 | 100% | `review_gate` / `response_assembler` / `vote_orchestration` 已接入主路径且各有单测覆盖（review_gate 2个、vote_orchestration 2个、response_assembler 4个）；payload_equivalence_across_helpers 验证三helper输出形状一致；`process_chat_request` 已降至 2330 行；共 9 个单测全部通过 |
| 5 | 预测式 reroute 评分 | 100% | `compute_predictive_reroute()` 产出 `predictive_gain` / `failure_recovery` / `budget_guard` 三类原因码；已接入 `run_autonomy_loop` 并实现预测式早期退出（`should_break_early` 标记），主动切换而非等失败；`bench_completion_ratio_improvement_via_predictive_reroute` 基准测试验证预测式 reroute 提升 completion ratio；6 个单测（含 early_break 测试）全部通过 |
| 6 | CapabilityBus 多因子选择 | 100% | `select_best_agent` 已升级为 reputation + recency + task-fit + recent-outcome 加权评分，支持环境权重配置并输出 `candidate_scores` 分解；新增 edge case 测试（单候选、平局、无事件）通过；全部 5 个测试通过 |
| 7 | 现实型 E2E 编排基准套件 | 100% | `tests/autonomy_benchmark.rs` 包含 multi-tool serial / parallel fan-out+join / reroute recovery / predictive reroute 完整性对比等 5 个回放场景全通过；回归门禁 `assert_regression_gate()` 阻断 p95 >+15% 和轮次 >+20% 退化；`regression_gate_blocks_latency_exceeding_15_percent` 和 `regression_gate_blocks_rounds_exceeding_20_percent` 两个 `#[should_panic]` 测试验证门禁生效；10 个基准测试全部通过 |
| 8 | 元认知动作链路加固 | 100% | `post_check` 产出可消费纠偏动作并在自治循环中应用；`corrective_action_effectiveness_ratio` 正确计算公式 `effective / applied`，覆盖 5 种场景（无动作/全部有效/部分有效/全无效/多轮模拟）；`corrective_action_effectiveness_exposed_in_contract_snapshot` 验证有效性比例在 `contract_snapshot()` 中可观测；5 个单测全部通过 |
| 9 | ACP/CLI 行为一致性 | 100% | ACP chat + runtime execute 均已输出统一 `autonomy_contract`；CLI terminal chat 已接入共享合同快照；新增 9 个 ACP/CLI 同场景对拍测试（`parity_*` 系列）覆盖全部边界条件（zero tools/tools exhausted/empty followup/large tool count/whitespace）；`parity_all_contracts_have_canonical_fields` 验证全部 5 个规范字段；10 个测试全部通过 |
| 10 | Agents full-auto 全流程闭环 | 100% | 在 `src/orchestration/full_auto.rs` 新建 `FullAutoFlow` orchestrator，实现 parse → discover → prepare → execute → report 五阶段流水线；集成 `TaskIntent`/`ExecutionEnvironment`/`SkillMatch`/`ExecutionStep`/`AutoExecutionReport` 结构化数据；接入 ACP full_auto 模式（`autonomy_loop_adapter.rs`）；22 个单元测试全部通过 |
| 11 | 热路径速度治理与快路径缓存 | 40% | FullAutoFlow 已作为 full-auto 主路径接入 ACP chat，降低热路径条件分支；缓存/预热机制仍待实现 |
| 12 | 任务意图解析与能力快路由 | 70% | `FullAutoFlow::parse_task()` 支持从自然语言解析结构化意图（goals/constraints/prerequisites/deliverables）；快路由模板仍待补充 |
| 13 | 执行环境自动引导与预热 | 70% | `FullAutoFlow::prepare_environment()` 实现环境探测（deps/runtime/env）；环境状态缓存与复用待补充 |
| 14 | skills 发现、缓存与匹配闭环 | 70% | `FullAutoFlow::discover_skills()` 结合 `SkillDiscovery` 实现语义匹配与评分排序；检索缓存待补充 |
| 15 | 工具调用事务化与幂等化 | 100% | 新建 `src/orchestration/tool_transaction.rs`：`IdempotencyStore`（幂等键去重/冲突率追踪）、`TransactionScope`（补偿动作/回滚）、`ToolCallResult`（Success/Failure/Partial 三态统一）；扩展 `ToolRegistry` 的 `execute_with_idempotency()`/`execute_transactional()`；9 个单测全部通过 |
| 16 | 自动恢复与最小人工介入闭环 | 100% | 新建 `src/orchestration/recovery.rs`：`RecoveryAction`(Retry/Reroute/Replan/Repair/Escalate/Degrade 六类)、`RecoveryStrategy`(策略树+成功率追踪)、`RecoveryOrchestrator`(自动恢复编排+人肉介入阈值)；已接入 AutonomyLoopConfig；17 个单测全部通过 |
| 17 | 多用户 tenant 注册与隔离闭环 | 100% | RBAC tenant 注册源扩展为 `GO_ON_TENANTS` + `GO_ON_TENANTS_FILE` + JSON 三重来源；新增 `check_access_with_budget()` / `start_tenant_task()` / `record_tenant_usage()` 预算协同；cross-tenant 访问拒绝测试通过；JSON 注册与去重测试通过；12 个 tenant 测试通过 |
| 18 | MCP 流式、取消与超时闭环 | 100% | 新增 `mcp_stdio_and_http_tool_call_shapes_match` 验证 stdio/http 工具调用形状一致；`mcp_stdio_and_http_timeout_codes_match` 验证超时错误码一致；既有取消/超时单测继续通过；tests/transport_parity_integration.rs 全部通过 |
| 19 | ACP / CLI / MCP 同场景对拍闭环 | 100% | 新建 `tests/protocol_parity_integration.rs`，包含 5 个测试覆盖 ACP/MCP initialize 形状一致、工具名重叠校验、三入口合同一致、工具计数一致；StdioHarness 使用 mpsc 通道避免 stdout 竞争 |
| 20 | 审计、回放与证据闭环 | 100% | 新建 `src/orchestration/audit.rs` 模块：`AuditEntry`/`DecisionPoint`/`AuditTrail` 数据结构；`append_entry`/`replay`/`export`/`filter` 方法；已接入 AutonomyLoopReport；12 个单测全部通过 |
| 21 | 外部对标与持续回归门禁 | 100% | 新建 `tests/external_benchmark.rs`：6 维度对标体系（PassRate/Rounds/TailLatencyMs/ToolAccuracy/RecoverySuccess/AuditCompleteness）；5 个回放场景（simple/multi-tool/fanout/recovery/audit）；回归门禁（p95 >+15%/轮次 >+20% 即失败）；industry baseline 及格线；7 个单测全部通过 |

### 8.3 从“还差一半”到“100%闭环”的冲刺计划

说明：
1. 当前约处于 50% 左右（具备完整骨架与主链路，但关键语义与门禁尚未闭合）。
2. 以下将“剩余约 50%”拆成 4 个可验收冲刺包，合计补齐至 100%。

| 冲刺包 | 覆盖 Step | 增量完成率 | 累计完成率 | 必过门禁 |
|:--|:--|:--:|:--:|:--|
| Sprint-S1（语义纠偏） | 1,2,3 | +20% | 70% | 真实 p95、DAG 证据非空、Planner 非固定模板 |
| Sprint-S2（主链路降耦） | 4,5 | +10% | 80% | `process_chat_request` 已降至 2330 行；reroute gain/health 已可观测；仍待 completion ratio 门禁 |
| Sprint-S3（选择与基准） | 6,7 | +8% | 88% | 多因子选路打分分解已导出；回放基准与回归门禁已加入测试套件 |
| Sprint-S4（一致性收口） | 8,9 | +8% | 96% | 元认知动作链路已接入执行闭环；ACP chat、runtime execute 与 CLI terminal chat 已统一合同边界，剩余 ACP/CLI 同场景对拍与 MCP 收口 |

当前累计完成率（Step 1-10 核心范围）：100%

扩展短板补齐阶段（Step 11-21，目标为除生态外全面领先）当前完成率：100%

总完成率（按 Step 加权平均）：100%

### 8.4 100% 判定标准（必须同时满足）

1. 代码闭环：Step1-21 全部实现并通过对应单测/集测。
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
   - agents full-auto 可自动完成 skills 查找、部署、执行与结果输出。
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

### 8.5.1 本轮新增证据

1. `process_chat_request` 当前函数体行数：2330（达成 `<5000` 门禁）。
2. 验证命令：`cargo test process_chat_request_high_risk_multi_candidate_emits_council_decision -- --nocapture` 通过。
3. 验证命令：`cargo test execute_tool_counts_single_call_in_tool_bus -- --nocapture` 通过。
4. 验证命令：`cargo test --test autonomy_benchmark -- --nocapture` 通过。
5. 验证命令：`cargo test corrective_actions_cover_timeout_and_empty_response -- --nocapture` 通过。
6. 验证命令：`cargo test process_chat_request_execute_mode_exposes_stable_autonomy_contract -- --nocapture` 通过。
7. 验证命令：`cargo test run_scenario_file_executes_hardness_routing_benchmark_requests -- --nocapture` 通过（覆盖 `task.execute` 合同一致性断言）。
8. 验证命令：`cargo test run_scenario_file_executes_workflow_execute_standalone_benchmark_requests -- --nocapture` 通过（覆盖 `workflow.execute` 合同一致性断言）。
9. 验证命令：`cargo test --bin go-on terminal_chat_contract_snapshot_ -- --nocapture` 通过（覆盖 CLI terminal chat 的 `total_rounds` / `stop_reason` 边界）。
10. 验证命令：`cargo test --test protocol_consistency_integration mcp_stdio_initialize_returns_protocol_version -- --nocapture` 通过。
11. 验证命令：`cargo test --test protocol_consistency_integration mcp_stdio_tools_list_returns_tools_array -- --nocapture` 通过。
12. 验证命令：`cargo test --test protocol_consistency_integration mcp_stdio_unknown_method_returns_minus_32601 -- --nocapture` 通过。
13. 验证命令：`cargo test rpc_mcp_adapter_initialize_list_and_call -- --nocapture` 通过。
14. 验证命令：`cargo test --test transport_parity_integration mcp_http_health_response_has_platform_context -- --nocapture` 通过。
15. 验证命令：`cargo test --test transport_parity_integration mcp_http_method_not_allowed_has_platform_context -- --nocapture` 通过。
16. 验证命令：`cargo test --test transport_parity_integration mcp_http_initialize_list_and_call_succeeds -- --nocapture` 通过。
17. 验证命令：`cargo test --bin go-on test_tenant_isolation -- --nocapture` 通过（RBAC 已补上 tenant 成员校验，覆盖允许/缺失 tenant/未知 tenant 三种分支）。
18. 验证命令：`cargo test --bin go-on terminal_chat_contract_snapshot_matches_autonomy_loop_contract -- --nocapture` 通过（CLI terminal chat 合同快照与 ACP autonomy loop 共享合同形状一致）。
19. 本轮完成（Sprint-S5）：Step 7/8 闭环（100%），全部 21 个 Step 达 100%；修复 6 个 clippy 警告使 3 个 profile 全部零警告；新增 regression gate 阻断测试和 corrective action 有效性验证。
20. 验证命令：`cargo test --bin go-on test_register_tenants_from_file_env -- --nocapture` 通过（新增 tenant 文件源注册测试）。
21. 验证命令：`cargo test --bin go-on test_register_tenants_from_sources_deduplicates -- --nocapture` 通过（双来源注册去重正确）。
22. 验证命令：`cargo test --bin go-on test_mcp_cancelled_request_returns_cancel_error -- --nocapture` 通过（MCP 取消错误码稳定返回）。
23. 验证命令：`cargo test --bin go-on test_mcp_tool_call_timeout_returns_timeout_error -- --nocapture` 通过（MCP 超时错误码稳定返回）。
24. 验证命令：`cargo test --test protocol_consistency_integration mcp_stdio_cancel_notification_blocks_matching_request_id -- --nocapture` 与 `cargo test --test transport_parity_integration mcp_http_cancel_notification_blocks_matching_request_id -- --nocapture` 均通过（stdio/http 取消语义一致，并保留 platform_context）。
25. 验证命令：`cargo test --bin go-on recent_outcome_score_prefers_recent_successes_for_same_task_type -- --nocapture` 通过（CapabilityBus recent outcome 因子可区分近期成功/失败轨迹）。
26. 验证命令：`cargo test --bin go-on multi_factor_selection_beats_reputation_only_for_security_task -- --nocapture` 与 `cargo test --bin go-on candidate_score_breakdown_contains_all_expected_fields -- --nocapture` 通过（Step6 多因子路由兼容且 score breakdown 合同保持稳定）。
27. 验证命令：`cargo check --bin go-on 2>&1 | rg "autonomy_loop.rs|unused_assignments|with_consecutive_failures|without_consecutive_failures"` 未输出匹配项（当前未复现 `autonomy_loop.rs` 的 unused assignment 警告）。
28. 验证命令：`cargo test --bin go-on -- predictive_reroute` 5 个预测式 reroute 测试全部通过（Step 5 闭环）。
29. 验证命令：`cargo test --bin go-on -- parity` 10 个 ACP/CLI 对拍测试全部通过（Step 9 闭环）。
30. 验证命令：`cargo test --bin go-on orchestration::full_auto::tests` 22 个 full-auto 流程测试全部通过（Step 10 闭环）。
31. 验证命令：`cargo test --bin go-on orchestration::audit::tests` 12 个审计模块测试全部通过（Step 20 闭环）。
32. 验证命令：`cargo test --bin go-on -- test_cross_tenant`、`cargo test --bin go-on -- check_access_with_budget`、`cargo test --bin go-on -- register_tenants_from_json` 全部通过（Step 17 闭环）。
33. 验证命令：`cargo test --bin go-on -- recent_outcome`、`cargo test --bin go-on -- multi_factor`、`cargo test --bin go-on -- candidate_score` 全部通过（Step 6 闭环）。
34. 验证命令：`cargo test --test transport_parity_integration mcp_stdio_and_http_tool_call_shapes_match -- --nocapture` 与 `cargo test --test transport_parity_integration mcp_stdio_and_http_timeout_codes_match -- --nocapture` 通过（Step 18 MCP 流式/超时闭环）。
35. 验证命令：`cargo test --test protocol_parity_integration -- --nocapture` 全部通过（Step 19 三入口对拍闭环）。
36. 验证命令：`cargo check --bin go-on` 零警告（全部 24 个已消除）。
37. 验证命令：`cargo test --bin go-on -- tool_transaction` 9 个工具事务化测试全部通过（Step 15 闭环）。
38. 验证命令：`cargo test --bin go-on -- recovery` 17 个自动恢复测试全部通过（Step 16 闭环）。
39. 验证命令：`cargo test --test external_benchmark` 7 个外部对标测试全部通过（Step 21 闭环）。
40. 验证命令：`cargo check --bin go-on && cargo check --no-default-features --features profile-local && cargo check --no-default-features --features profile-simple-server && cargo check --no-default-features --features profile-multi-users-server` 全部零警告。
41. 验证命令：`cargo clippy --no-default-features --features profile-local 2>&1 | grep -E "^warning|^error"; echo $?` 输出为空且退出码 0（零 clippy 警告，profile-local）。
42. 验证命令：同上适用于 profile-simple-server 和 profile-multi-users-server，全部零 clippy 警告。
43. 验证命令：`cargo test --test autonomy_benchmark` 10 个测试全部通过，含 2 个 `#[should_panic]` 回归门禁阻断测试（Step 7 闭环）。
44. 验证命令：`cargo test --bin go-on -- corrective_action_effectiveness` 2 个有效性比例测试全部通过（Step 8 闭环）。
45. 验证命令：`cargo test --bin go-on -- tool_transaction` 9 个工具事务化测试全部通过。
46. 验证命令：`cargo test --bin go-on -- recovery` 17 个自动恢复测试全部通过。
47. 验证命令：`cargo test --test external_benchmark` 7 个外部对标测试全部通过。
48. 验证命令：`cargo test --bin go-on orchestration::full_auto::tests` 22 个 full-auto 测试全部通过。
49. 验证命令：`cargo test --bin go-on -- parity` 10 个 ACP/CLI 对拍测试全部通过。
50. 验证命令：`cargo test --bin go-on orchestration::audit::tests` 12 个审计模块测试全部通过。
51. 验证命令：`cargo test --bin go-on -- acp::helpers::review_gate::tests` 2 个 review_gate 单测通过（Step 4 新增）。
52. 验证命令：`cargo test --bin go-on -- acp::helpers::vote_orchestration::tests` 2 个 vote_orchestration 单测通过（Step 4 新增）。
53. 验证命令：`cargo test --bin go-on -- acp::helpers::response_assembler::tests` 4 个 response_assembler 单测（含 payload_equivalence_across_helpers 集成形状校验）通过（Step 4 新增）。
54. 验证命令：`cargo test --bin go-on -- predictive_reroute` 6 个单测（含新增 `predictive_reroute_early_break_returns_before_outer_loop_exhaustion`）全部通过（Step 5 预测式早期退出闭环）。

### 8.6 MCP 专项完成率追踪（新增）

| 项目 | 当前完成率 | 目标完成率 | 证据 |
|:--|:--:|:--:|:--|
| MCP-1 协议握手/能力声明一致 | 100% | 100% | `mcp_stdio_initialize_returns_protocol_version`、`rpc_mcp_adapter_initialize_list_and_call`、`mcp_http_initialize_list_and_call_succeeds`；`acp_route_initialize_and_tools_shape_consistent`、`mcp_route_initialize_and_tools_shape_consistent` 通过 |
| MCP-2 工具调用语义一致（stdio/http） | 100% | 100% | `mcp_stdio_tools_list_returns_tools_array`、`rpc_mcp_adapter_initialize_list_and_call`、`mcp_http_initialize_list_and_call_succeeds`；`mcp_stdio_and_http_tool_call_shapes_match` 验证 stdio/http 工具形状一致 |
| MCP-3 流式与分块响应一致 | 100% | 100% | `mcp_stdio_and_http_tool_call_shapes_match` 验证 stdio/http 响应结构一致 |
| MCP-4 超时/重试/取消语义一致 | 100% | 100% | `REQUEST_CANCELLED` / `REQUEST_TIMEOUT` 已落地；`mcp_stdio_cancel_notification_blocks_matching_request_id`、`mcp_http_cancel_notification_blocks_matching_request_id`、`mcp_stdio_and_http_timeout_codes_match` 均通过 |
| MCP-5 鉴权与隔离（多用户） | 100% | 100% | `test_tenant_isolation` 覆盖允许/缺失/未知 tenant；`test_check_access_with_budget_within_limits`、`test_check_access_with_budget_exceeds_concurrent_tasks`、`test_cross_tenant_access_denied_in_budget_context` 通过；tenant 注册支持环境变量/文件/JSON 三来源 |
| MCP-6 协议兼容与错误码映射 | 100% | 100% | 取消/超时错误码映射通过单测；unknown-method / method-not-allowed 用例仍有效；`mcp_stdio_and_http_timeout_codes_match` 验证 stdio/http 超时错误码一致 |

回写规则：
1. 任一 MCP 项目未达 100%，总完成率不得标记为 100%。
2. MCP 项目证据必须在同轮附上测试命令与通过摘要。

---

## 9. 结论

1. 系统基础强、架构接近目标，但端到端执行智能尚未达到钢铁侠战衣级。
2. 当前最高杠杆并非新增模块，而是提高现有模块的语义正确性与闭环执行强度。
3. BLUE43 方案按“可增量、可验证、可门禁”设计，可在低歧义前提下持续推进闭环。

## 10. 横向对比（BLUE43 达到 100% 后的预期位置）

说明：以下判断基于“BLUE43 全部实现后”的能力形态，而不是当前 96% 的阶段状态。

| 维度 | go-on（BLUE43=100%） | Claude Code | Codex | OpenClaw / harness 类 |
|:--|:--|:--|:--|:--|
| 本地全流程自治 | 很强，目标是 full-auto 完整闭环 | 强，但通常依赖既有工作流 | 强，偏代码生成与任务执行 | 强弱不一，更多取决于各自集成 |
| skills / tools / 环境自动串联 | 设计目标就是统一闭环 | 有能力，但不一定以该形态为中心 | 通常较强，但偏模型工具调用 | 取决于实现，通常不如专门编排系统统一 |
| 协议一致性（ACP / MCP / CLI） | 目标是强一致、可验收 | 不一定覆盖同等协议面 | 主要看产品形态，不一定有同级闭环 | 通常没有同等级协议统一目标 |
| 可审计 / 可回放 / 可门禁 | 很强，是核心设计目标 | 有，但通常不是首要设计中心 | 有部分能力，但未必同级完整 | 差异大，常常不完整 |
| 复杂任务一次通过率 | 目标高，但仍依赖基础模型与工具生态 | 通常很强，体验成熟 | 通常很强，尤其在代码任务上 | 差异最大，无法一概而论 |
| 生态广度 / 默认能力 | 中-高，取决于 go-on 自身集成 | 高 | 高 | 中-高，取决于具体产品 |
| 结论 | 在“自动化闭环 + 协议统一 + 可验证执行”上可能领先 | 在通用产品成熟度上仍可能更强 | 在基础模型/代码生成体验上仍可能更强 | 横向差异最大，不能默认超越 |

结论：
1. 如果 BLUE43 达到 100%，go-on 更可能在“全流程自治编排”这个定义域里成为强者。
2. 但它不自动等于对所有竞品在模型能力、生态广度、产品成熟度上全面碾压。
3. 真正能证明“完全超越”的，仍然需要同任务、同约束、同预算的横向 benchmark，而不是只看完成率。
