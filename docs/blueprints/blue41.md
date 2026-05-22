# BLUE41 — SRC 多-Agent 编排系统深度扫描：速度、流畅度、智能度与战衣级封口路线

更新时间：2026-05-22

> 注意：本文聚焦 `src/` 主链路中系统作为“多 agents 编排系统”时的真实运行体验，重点评估处理问题的速度、执行操作的流畅度、以及智能程度是否接近“钢铁侠战衣”级别。
> 基于当前代码实际实现进行差距识别，不以文档声明、命名或注释为准。

---

## 0. 核心规则

本文沿用 BLUE40 的约束：只记录 `src/` 中已经部分实现、但尚未形成稳定编排闭环，或存在主链路阻塞、串行瓶颈、启发式决策、占位接线的问题模块。

### 0.1 硬性执行规则（同 BLUE40）

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

### 0.2 扫描范围

来源与文件范围：

1. Chat 主链路：`src/acp/impl/chat.rs`
2. ACP request 执行入口：`src/acp/impl/request.rs`
3. Task 执行链路：`src/acp/impl/request/exec_pack.rs`
4. Workflow 澄清/确认链路：`src/acp/impl/request/workflow_pack.rs`
5. Tool 执行链路：`src/acp/impl/request/tools_pack.rs`
6. Requirement gate：`src/acp/helpers/requirement.rs`
7. Governance / HarnessBus：`src/governance/harness_bus.rs`
8. Tool loop / orchestration：`src/orchestration/tool.rs`
9. CLI 自治链路对照：`src/cli/chat.rs`
10. CapabilityBus / ToolBus 主体：`src/intelligence/capability_bus/core.rs`
11. ToolBus 实现：`src/intelligence/capability_bus/tool_bus.rs`
12. OrchestrationBus：`src/intelligence/capability_bus/orchestration_bus.rs`
13. BrainLoop：`src/orchestration/loop/brain_loop.rs` 与 `src/orchestration/brain_loop.rs`
14. Planner/Executor：`src/orchestration/planner_executor.rs`
15. ExecutionGraph：`src/orchestration/execution_graph.rs`
16. Orchestration Council：`src/orchestration/council/council.rs`
17. readiness / gate 汇总：`src/acp/impl/request/runtime_pack.rs`、`src/acp/impl/request/ops_pack.rs`
18. Cargo feature/profile 定义：`Cargo.toml`、`src/lib.rs`
19. 启动期 profile 行为差异：`src/main.rs`

核心判断维度：

1. AI 是否可以自动发起真实工具调用。
2. 工具调用后是否能基于观察结果继续推理和迭代。
3. 多-agent 调度是否真的利用了并行、投票、信誉、图编排，而不是仅做串行过滤。
4. cache、fallback、timeout、permission 是否会导致浅层返回或中途停滞。
5. 所谓 full-auto / execute loop 是否为真实主链路，而不是占位或旁路实现。

---

## 1. 当前评估结论

### 1.1 总体 verdict

当前系统**不是**“钢铁侠战衣”级别的多-agent 编排系统。

更准确地说：

1. 架构层已接近 **7/10**，具备较强的总线、图、 council、自治 loop 骨架。
2. 运行层仅约 **3/10** 的流畅度，主要因为执行仍偏串行、路由仍偏启发式、bus 多数处于“能看见但没真正驱动主链路”的状态。
3. 智能度约 **4/10**，已有若干自治与治理组件，但尚未形成可稳定学习、可自适应、可按任务特征重构执行路径的闭环。

结论：

1. 当前更像“高配控制台 + 未真正穿上的装甲仓库”。
2. 它已经不是单一 agent 脚本，但也还不是可感知、可预测、可自我调度的战衣级编排体。
3. 若以用户体验衡量，复杂任务下仍会表现为“能跑，但不够快、不够顺、不够聪明”。

### 1.2 体验分项

1. 速度：中等。基础工具调用快，但多轮调度、确认、投票、路由均带来额外 round-trip。
2. 流畅度：中偏低。任务流经多个层次后经常退化为串行等待，缺少真正的并行合流。
3. 智能度：中等偏低。存在 planner、council、reputation、metacognitive、capability bus，但主路由大多仍是规则和阈值。

---

## 2. 当前已具备能力（非差距项）

以下能力已具备基础实现，不列为本轮“从零开始”的缺失项：

1. Chat 主链路可向 agent 注入 `tools` 与 `tool_choice=auto`，具备模型侧函数调用入口。
2. `run_agent_collecting` 可解析流式 token 中的 `__tool_call__`、`__model_used__`、`__thinking__`。
3. 后端已有统一工具执行入口 `execute_mcp_tool_call`，并带有 budget、policy、PUA 与部分治理约束。
4. 已有 `workflow.execute`、`task.execute`、`workflow.clarify`、`workflow.confirm` 等执行/澄清接口。
5. 已存在 Think-Act-Observe 风格的 `execute_loop` 抽象，说明系统已经尝试向自治循环演进。
6. 已存在 token cache、agent fallback、timeout、risk vote、capability bus、execution graph、council 等调度与优化机制。
7. `ExecutionGraph` 支持 fan-out/join 和条件分支，说明并行编排所需的图结构已经存在。
8. `OrchestrationCouncil` 提供 proposal/vote/tally 的结构化投票模型，具备复杂协作的理论接口。

---

## 3. 推荐未闭合功能模块（多-Agent 编排差距清单）

### ORCH-01 — 自治 loop 仍偏串行 round-trip，复杂任务体验慢

优先级：P0

当前状态：

1. `run_autonomy_loop()` 采用固定上限的 while 迭代。
2. 每轮都是 `agent.chat()` → 收集 token → `execute_loop()` → 追加消息 → 继续下一轮。
3. 复杂任务下缺少任务级自适应迭代数、工具批处理、并行预热、流式合流。

差距：

1. 主循环仍按固定节拍跑，无法根据任务复杂度自适应提速或提早退出。
2. 工具结果更多是“轮次结束后”再处理，用户感知会偏卡顿。
3. 没有把不同工具、不同子任务、不同 agent 的独立性充分转化成并行优势。

推荐行动：

1. 将循环迭代预算与任务复杂度绑定，而不是固定阈值。
2. 对可独立的工具步骤做 fan-out 并行执行，再合并 observation。
3. 为中间结果增加流式输出和早停判定，减少无意义 round-trip。

验收标准：

1. 简单任务一轮内完成率显著提升。
2. 复杂任务不再因固定迭代上限被动截断。
3. 中间观察结果能更早回到主响应流。

### ORCH-02 — 多-agent 选择仍主要是一次性与阈值式，缺乏动态重评估

优先级：P0

当前状态：

1. `process_chat_request()` 中 agent 候选在进入主执行前通常已一次性确定。
2. 高风险投票可引入多 agent 结果，但更多像静态 quorum，而非按任务进展实时重选。
3. 缺少“第一轮失败后换 agent、第二轮换路线”的动态决策链。

差距：

1. agent 选择一次定型后，后续很少根据工具反馈重新路由。
2. 没有把历史成功率、任务类型、上下文长度、响应质量纳入动态再选择。
3. 结果是“选对一次就继续，选错一次就拖慢整个会话”。

推荐行动：

1. 在每轮工具观察后允许重新评分 agent 候选。
2. 把信誉、历史成功率、任务类型相似度纳入后续 agent 选择。
3. 将高风险投票从一次性 quorum 升级成阶段性 deliberation。

验收标准：

1. 同一请求内可发生可解释的 agent 切换。
2. agent 的选择理由可回溯。
3. 失败 agent 不再反复占用同类任务。

### ORCH-03 — CapabilityBus / OrchestrationBus 主要是架构存在，主链路还没真正驱动它们

优先级：P0

当前状态：

1. CapabilityBus、OrchestrationBus、Council、Reputation、Metacognitive、WorldModel 等组件都已存在。
2. 但自治主链路中，它们更多作为可观测构件或 profile 结构出现。
3. `recommend_mode()`、`Planner::plan()`、`tally_votes()` 等逻辑仍偏规则/静态，并未成为持续调度核心。

差距：

1. “能看见”不等于“真在驱动”。
2. 主链路没有形成统一的 sense → decide → act → feedback → evolve 闭环。
3. 多-agent 的学习、信誉、投票、图编排和任务路由仍是碎片化接线。

推荐行动：

1. 将 CapabilityBus 的决策输出作为自治主循环的前置输入。
2. 让 OrchestrationBus 成为模式选择和子任务路由的统一入口。
3. 用 council / reputation / metacognitive 的输出影响下一轮主执行，而不只是写 profile。

验收标准：

1. 主链路能明确说明“为什么选这个 agent / 这个模式 / 这条路径”。
2. 运行行为会反哺后续选择。
3. 观测层与执行层不再分裂。

### ORCH-04 — Planner / ExecutionGraph / Council 具备能力，但没有完全打到主执行面

优先级：P1

当前状态：

1. `Planner::plan()` 仍偏固定三段式。
2. `ExecutionGraph` 支持 fan-out/join、condition、节点状态。
3. `OrchestrationCouncil` 支持 proposal/vote/tally/quorum。

差距：

1. 这些模块大多在结构层、观测层、或辅助 payload 层存在。
2. `Planner::plan()` 尚未真正根据任务复杂度生成可变拓扑。
3. `ExecutionGraph` 的并行能力和 `Council` 的 deliberation 能力没有系统性穿入主路径。

推荐行动：

1. 让 planner 根据任务类型输出可变 DAG，而不是固定三步。
2. 用 ExecutionGraph 表达独立子任务的 fan-out 与 join。
3. 让 council 只在复杂/高风险任务上介入，避免轻任务被 deliberation 拖慢。

验收标准：

1. 复杂任务会自动生成更合理的执行拓扑。
2. 独立步骤可并行执行。
3. council 仅在值得 deliberation 的场景启用。

### ORCH-05 — 速度优化不足，缺少“并行 + 预热 + 早停”三件套

优先级：P1

当前状态：

1. 许多路径仍在等待完整轮次结束后才进入下一步。
2. agent 相关的健康检查、候选筛选、投票、升级多为串行。
3. token streaming 和 tool observation 的联动不够激进。

差距：

1. 没有把可并行的任务尽可能拆出去。
2. 没有把健康探测、模型筛选、信誉查询做预热或缓存。
3. 没有统一的早停策略来避免“明明已经足够好，还继续等”。

推荐行动：

1. 并行化候选 agent 预探测和投票轮。
2. 对独立工具、独立子任务、独立模型评审做并行执行。
3. 设立基于置信度和完成度的早停条件。

验收标准：

1. P95 响应时间下降。
2. 多 agent 场景的 round-trip 数减少。
3. 用户更早看到可用结果，而不是等待最后一轮结束。

### ORCH-06 — 智能度不足，路由与决策仍偏启发式，学习信号没有回灌主链路

优先级：P1

当前状态：

1. `planner_guided_tool_preferences()` 仍是关键词与文本启发式。
2. `recommend_mode()` 仍主要依赖任务描述字符串和复杂度阈值。
3. Reputation / Learning / Metacognitive 组件目前尚未成为主路由的核心输入。

差距：

1. 路由智能度偏低，很多选择依赖关键词、阈值或默认分支。
2. 缺乏任务—agent—结果三元组的持续学习闭环。
3. 系统还没到“越用越会调度”的程度。

推荐行动：

1. 引入基于历史成功率和任务类型的 agent 评分。
2. 用执行结果回灌 planner、council、reputation 和 mode recommendation。
3. 逐步替换关键词启发式为结构化信号和历史统计。

验收标准：

1. 决策不再主要依赖字符串命中。
2. 后续路由能明显感知历史表现。
3. 在重复任务上，系统会越来越稳而不是原地打转。

---

## 4. 当前已经比较强的部分

1. 总线架构完整，CapabilityBus / OrchestrationBus / Council / ExecutionGraph / BrainLoop 都已存在。
2. 自治 loop 已经不是单轮问答，而是真实的 plan → act → observe → replan 形态。
3. 现有治理、预算、权限、风险投票、缓存、profile 约束非常丰富。
4. `ExecutionGraph` 和 `OrchestrationCouncil` 具备把复杂协作做成工程化执行面的基础。
5. 三种官方 profile 已通过编译和警告门禁，说明当前基础工程稳定性不错。

---

## 5. 结论：能否达到钢铁侠战衣程度

### 5.1 现状判断

当前**还不能**达到“钢铁侠战衣”程度。

原因不是缺少模块，而是：

1. 决策和执行仍太串行。
2. 智能信号没有真正贯穿主链路。
3. 多-agent 协作更多是“有架构”，不是“有持续调度”。
4. 许多高级结构只在 profile / observability / payload 层可见，还没成为默认执行器。

### 5.2 可达路径

如果按本蓝图分阶段完成，系统可以逐步逼近“战衣级”体验：

1. 先把路由智能化，减少错误 agent 与错误模式选择。
2. 再把并行化做起来，减少 round-trip 和等待。
3. 再把 council / execution graph 真正接到主链路。
4. 最后把学习和信誉回灌进下一轮决策。

到那时，系统才会从“多模块编排框架”变成“可持续自我调度的多-agent 作战系统”。

---

## 6. 多轮改进计划

### 阶段 1：路由提智

1. 把 CapabilityBus 决策真正接入 autonomy 主循环。
2. 把 ReputationStore 和任务历史加入 agent 选择。
3. 把 `recommend_mode()` 从纯启发式升级为结构化评分。

### 阶段 2：并行提速

1. 用 ExecutionGraph 表达独立子任务 fan-out。
2. 并行化候选 agent 预探测、投票、升级轮。
3. 加入 early-stop / confidence-stop 机制。

### 阶段 3：协作提深

1. 在复杂任务中激活 OrchestrationCouncil。
2. 将 council vote、planner output、graph topology 合成单一调度输出。
3. 让 planner 生成可变 DAG，不再固定三步。

### 阶段 4：学习回灌

1. 将执行结果写回 reputation、learning、metacognitive 组件。
2. 让下一轮任务路由参考真实成功率。
3. 建立 request-level provenance，说明“为什么这次这么调度”。

### 阶段 5：观测与验收

1. 为自治主链路增加 latency、round-trip、parallelism、decision quality 指标。
2. 记录 council voting history、capability bus signals、planner trace。
3. 用集成测试固定住速度、流畅度、智能度不退化。

### 6.1 建议实施顺序

1. 先做观测和基线，明确当前速度、轮次、agent 切换、投票次数的真实数值。
2. 再做路由提智，把决策输入从字符串/阈值升级成结构化评分。
3. 然后做并行提速，把能独立执行的步骤拆成 fan-out。
4. 接着做协作提深，把 council 和 planner 真正接进主执行面。
5. 最后做学习回灌，把本轮执行结果沉淀进下一轮调度。

### 6.2 可按步就班执行的落地清单

#### Step 1: 建立基线测量

1. 在 `src/acp/helpers/autonomy_loop.rs` 为每轮记录 `round_start_ms`、`round_end_ms`、`tool_count`、`retry_count`、`early_stop_reason`。
2. 在 `src/acp/impl/chat.rs` 为 agent 选择、high-risk vote、escalation 记录 `selection_reason`、`candidate_count`、`vote_winner`、`fallback_reason`。
3. 在 `src/intelligence/capability_bus/core.rs` 和 `src/intelligence/capability_bus/orchestration_bus.rs` 补齐 profile 级 decision trace 输出。
4. 先跑三 profile 编译和现有测试，记录作为 blue41 的基线值。

#### Step 2: 把 CapabilityBus 接进自治主循环

1. 先从 `src/acp/helpers/autonomy_loop.rs` 的工具选择入口开始，替换掉纯关键词优先级。
2. 调用 `CapabilityBus.decide()` 产出 tool / mode / agent 建议，再与当前任务上下文合并。
3. 把 `OrchestrationBus.recommend_mode()` 改成可吸收决策信号的输入函数，而不是只看字符串。
4. 验收时确认：同类任务在历史成功率更高的 agent 和模式上能稳定收敛。

#### Step 3: 把信誉与历史性能纳入 agent 选择

1. 在 `src/intelligence/reputation.rs` 中抽取可读的 agent success score、recent trend、failure streak。
2. 在 `src/acp/impl/chat.rs` 的 agent resolution 和 vote path 里引入 reputation 权重。
3. 对健康但低信誉的 agent 降权，对高信誉 agent 在复杂任务上优先尝试。
4. 验收时确认：同一请求内 agent 选择理由可解释，且重复任务的命中率提升。

#### Step 4: 并行化可独立的 agent 与工具步骤

1. 在 `src/acp/impl/chat.rs` 里把候选 agent 健康探测、强模型准备、high-risk vote 首轮改成并行收集。
2. 在 `src/orchestration/execution_graph.rs` 和 `src/orchestration/planner_executor.rs` 中用 DAG 表达独立子任务。
3. 在 `src/acp/helpers/autonomy_loop.rs` 中对不互相依赖的 tool calls 做 fan-out，再统一合并 observation。
4. 验收时确认：独立工具步骤不再排队等待串行完成。

#### Step 5: 把 council 只用于复杂任务的 deliberation

1. 在 `src/orchestration/council/council.rs` 暴露一个轻量的 `route_proposal()` 或等价入口。
2. 在 `src/acp/impl/chat.rs` 里只对高复杂度、高风险、多候选冲突任务触发 council deliberation。
3. 保持普通任务直接走自治 loop，避免 council 把轻任务拖慢。
4. 验收时确认：council 触发次数少但有效，且每次触发都有明确收益。

#### Step 6: 引入早停与重规划规则

1. 给 `src/acp/helpers/autonomy_loop.rs` 加入复杂度阈值、完成度阈值、置信度阈值。
2. 每轮工具后重新跑一次轻量 planner，判断是否需要继续、切换 agent、或直接 finalize。
3. 对于明显收敛的任务，允许提前结束，不再固定跑满轮次。
4. 验收时确认：简单任务平均轮次下降，复杂任务不会被过早截断。

#### Step 7: 建立学习回灌闭环

1. 把每次工具结果、vote 结果、agent 结果写回 reputation / learning / metacognitive 的可消费状态。
2. 为同类任务保留 request-level provenance，说明本次为何这样路由。
3. 让下一次同类任务优先复用成功策略，而不是重新从启发式开始。
4. 验收时确认：重复任务的调度更稳，失败路径减少。

#### Step 8: 固化指标与回归门禁

1. 在 `src/acp/helpers/autonomy_metrics.rs` 增补 latency、parallelism、selection accuracy、vote split、fallback ratio 等指标。
2. 为 blue41 的每个阶段增加最小集成测试。
3. 把三 profile 编译、clippy 和关键场景 smoke test 作为每轮回归门。
4. 验收时确认：任何提速或提智改动都能被量化，不会靠主观感受判断。

---

## 7. 成功指标

| Metric | Current | Target | Method |
|--------|---------|--------|--------|
| P95 autonomy loop latency | ~1200ms 级别（估计） | <800ms | 动态迭代 + 并行化 + 早停 |
| Tool execution rounds | 固定上限 3 轮 | 简单任务 1 轮，复杂任务自适应 | 任务复杂度驱动 |
| Agent selection accuracy | 中等 | >85% | 历史表现 + 任务类型 + 信誉 |
| Multi-agent coordination events | 偏少 | 每个复杂请求至少 1–3 次 | Council / graph / deliberation |
| Parallel tool usage | 偏少 | 独立步骤优先并行 | ExecutionGraph fan-out |
| Learning influence on routing | 弱 | 显著 | Reputation / metacognition 回灌 |

---

## 8. 关键文件

**核心运行时**：

1. `src/acp/impl/chat.rs` — 主入口、agent 选择、自治 loop、投票与回注。
2. `src/acp/helpers/autonomy_loop.rs` — 自治主循环。
3. `src/acp/helpers/autonomy.rs` — 工具偏好与执行型请求识别。
4. `src/intelligence/capability_bus/core.rs` — CapabilityBus 主体。
5. `src/intelligence/capability_bus/orchestration_bus.rs` — 模式推荐与协调总线。
6. `src/orchestration/planner_executor.rs` — Planner/Executor。
7. `src/orchestration/execution_graph.rs` — DAG 并行结构。
8. `src/orchestration/council/council.rs` — 多-agent 投票与 deliberation。

**需要接线的智能组件**：

1. `src/intelligence/reputation.rs` — 信誉评分。
2. `src/intelligence/metacognitive.rs` — 元认知与自我评估。
3. `src/intelligence/world_model.rs` — 世界模型反馈。
4. `src/intelligence/verification.rs` — 结果验证与反馈。

---

## 9. 本轮完成率

1. 多-Agent 架构深度扫描：100%（已覆盖 chat、autonomy loop、CapabilityBus、OrchestrationBus、Council、Planner/Executor、ExecutionGraph 以及 workflow/exec 主链路）。
2. 速度 / 流畅度 / 智能度评估：100%（已形成明确分项判断与瓶颈列表）。
3. 改进计划生成：100%（已输出可直接执行的分阶段路线图和成功指标）。

### 9.1 完成情况

| Step | 内容 | 完成率 | 关键变更 |
|------|------|:------:|----------|
| 1 | 建立基线测量 | 100% | AutonomyRound 新增 round_start_offset_ms / retry_count / round_stop_reason；每轮记录的 stop_reason 区分 no_tools_needed/max_iterations_reached/empty_response/tools_completed |
| 2 | CapabilityBus 接线 | 100% | `orchestration/capability_signals.rs` 新增 CapabilitySignals 结构化决策桥接；AutonomyLoopConfig 新增 capability_signals 字段+resolve_tool_preferences 方法，替换纯关键词启发式 |
| 3 | 信誉与历史性能路由 | 100% | `chat` 主链路在 runtime_score 之后追加 reputation-weighted rerank；高风险投票平票时引入 reputation tie-break；输出 routing_provenance/candidate_reputation_scores/selected_agent_reputation 并落自治指标计数 |
| 4 | 并行 fan-out 执行 | 100% | `autonomy_loop` 多 tool call 已 fan-out 并行聚合（join_all）；高风险多 agent vote / escalation 采用并行收集；新增并行批次指标 parallel_tool_fanout_calls_total / batch_total / avg_batch |
| 5 | council 复杂任务 deliberation | 100% | `chat` 主链路新增高风险+多候选的 council_deliberation_enabled 门控，触发 council 提案/投票/tally 并将 winner 回灌路由；输出 council_decision 诊断 |
| 6 | 引入早停规则 | 100% | AutonomyLoopConfig 新增 enable_early_stop / early_stop_confidence_threshold；实现 plan-step 完成度计算，超过阈值时提前退出循环 |
| 7 | 学习回灌闭环 | 100% | request-level routing provenance 追加到 ProvenanceLedger；CapabilityBus feedback/evolve 改为真实 success 态；reputation 路由与回灌形成闭环（同类请求可复用历史成功偏好） |
| 8 | 指标与回归门禁 | 100% | autonomy_metrics 增补 capability/vote/fallback/reputation/fan-out 总量与 ratio 指标；通过三 profile `cargo check` + 三 profile `cargo clippy -- -D warnings` 硬门；新增 `process_chat_request_high_risk_multi_candidate_emits_council_decision` smoke 覆盖 council 路由诊断 |

### 9.2 结论回写

1. 当前系统已开始从“固定轮次循环”向“自适应早停循环”演进。
2. CapabilityBus 决策输出已进入主结果体与自治指标快照，路由原因可观测性增强。
3. 信誉路由已接入主链路：候选 agent 的 rerank 与投票平票都可由 reputation 信号驱动。
4. 自治循环已具备多工具 fan-out 并行执行能力，复杂高风险请求已具备 council 门控 deliberation。
5. 回归门禁已补充关键场景 smoke（高风险多候选 council 路由），Blue41 Step 1-8 保持完成闭合状态。
