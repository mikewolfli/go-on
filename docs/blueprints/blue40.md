# BLUE40 — SRC 全量深度扫描：AI 自主调用工具与自主完成工作闭环蓝图

更新时间：2026-05-21

> 注意：本文聚焦 `src/` 主链路中 AI 是否具备“自动分析问题、自动调用工具、自动完成工作而非阻塞或浅尝即止”的真实闭环能力。
> 基于当前代码实际实现进行差距识别，不以文档声明、命名或注释为准。

---

## 0. 核心规则

本文仅记录 `src/` 中已经部分实现、但尚未形成稳定自治闭环，或存在主链路阻断、浅层执行、占位实现的问题模块。

### 0.1 硬性执行规则（同 BLUE39）

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
12. BrainLoop：`src/orchestration/loop/brain_loop.rs` 与 `src/orchestration/brain_loop.rs`
13. Planner/Executor：`src/orchestration/planner_executor.rs`
14. ExecutionGraph：`src/orchestration/execution_graph.rs`
15. Orchestration Council：`src/orchestration/council/council.rs`
16. readiness / gate 汇总：`src/acp/impl/request/runtime_pack.rs`、`src/acp/impl/request/ops_pack.rs`
17. Cargo feature/profile 定义：`Cargo.toml`、`src/lib.rs`
18. 启动期 profile 行为差异：`src/main.rs`

核心判断维度：

1. AI 是否可以自动发起真实工具调用。
2. 工具调用后是否能基于观察结果继续推理和迭代。
3. 复杂任务是否会被 requirement/confirm 流程结构性阻断。
4. cache、fallback、timeout、permission 是否会导致浅层返回或中途停滞。
5. 所谓 full-auto / execute loop 是否为真实主链路，而不是占位或旁路实现。

---

## 1. 当前已具备能力（非差距项）

以下能力在 `src/` 中已具备基础实现，不列为本轮“从零开始”的缺失项：

1. Chat 主链路可向 agent 注入 `tools` 与 `tool_choice=auto`，具备模型侧函数调用入口。
2. `run_agent_collecting` 可解析流式 token 中的 `__tool_call__`、`__model_used__`、`__thinking__`。
3. 后端已有统一工具执行入口 `execute_mcp_tool_call`，并带有 budget、policy、PUA 与部分治理约束。
4. 已有 `workflow.execute`、`task.execute`、`workflow.clarify`、`workflow.confirm` 等执行/澄清接口。
5. 已存在 Think-Act-Observe 风格的 `execute_loop` 抽象，说明系统已经尝试向自治循环演进。
6. 已存在 token cache、agent fallback、timeout、risk vote、capability bus 等调度与优化机制。

---

## 2. 推荐未闭合功能模块（自治能力差距清单）

### AUTON-01 — 工具调用后未回注模型，缺少真正多轮自治闭环

优先级：P0

当前状态：

1. `run_agent_collecting` 可从流式输出中收集 `__tool_call__`。
2. 工具会在 agent 输出结束后由后端执行。
3. 工具结果会被直接拼接回最终文本响应。

差距：

1. 工具执行结果没有重新回注给模型继续推理。
2. 不存在稳定的 `reason -> tool -> observe -> replan -> tool -> finalize` 主链路循环。
3. AI 只能“调一次工具并展示结果”，不能基于结果继续完成后续工作。

推荐行动：

1. 将工具执行从 `run_agent_collecting` 的末尾拼接逻辑提升为主循环状态机。
2. 建立统一的 `ToolObservation` / `AssistantAction` / `AssistantFinal` 契约。
3. 每轮工具结果都必须重新送回模型，直到完成、失败或达到显式预算上限。
4. 为多轮自治循环增加迭代次数、耗时、工具步数、停止原因等治理指标。

验收标准：

1. AI 可在单次请求中连续执行多步工具链路。
2. 第二轮及后续轮次明确消费上一轮工具观察结果。
3. 最终输出基于完整工具迭代，而非单轮模型文本加工具附录。

---

### AUTON-02 — Requirement gate 为硬阻断，复杂任务无法自动续跑

优先级：P0

当前状态：

1. `task.execute` 在进入计划与执行前会调用 requirement gate。
2. 当任务复杂、多模块、需要验证或存在安全顾虑时，gate 会判定需要 clarification/confirmation。
3. `workflow.confirm` 还要求外部显式提供 `ready_to_confirm=true`。

差距：

1. 当前流程以“返回错误并附带 next_step”作为主行为，而不是自动进入 clarify/confirm 子流程。
2. 复杂任务会在执行前被结构性阻断，表现为“卡住等待确认”。
3. 系统没有形成“自动澄清最小必要信息后继续执行”的自治闭环。

推荐行动：

1. 将 requirement gate 从硬错误改造成可恢复状态机。
2. 当信息缺口较小且无高风险时，允许自动生成 clarification round 并继续推进。
3. 引入 `auto_confirmable` / `requires_human_confirmation` / `clarification_in_progress` 等显式状态。
4. 仅将真正高风险或存在用户决策分歧的任务升级为人工确认阻断。

验收标准：

1. 中低风险复杂任务不再直接失败退出。
2. clarify/confirm 能回流至 `task.execute` 主链路自动续跑。
3. 阻断仅发生在策略定义的高风险任务上。

---

### AUTON-03 — Token cache 可短路实际执行，导致浅层返回

优先级：P0

当前状态：

1. `process_chat_request` 在高置信 cache 命中时会直接跳过 agent execution。
2. 命中后仍会向流式观察者发出 cached response，看起来像一次正常回答。

差距：

1. 当任务依赖当前工作区状态、文件变化或工具副作用时，cache 短路会直接跳过分析与执行。
2. 代码修改、排错、验证类任务存在“命中旧答案”的风险。
3. 当前策略更偏向节约 token，而不是保证任务真实完成。

推荐行动：

1. 为 cache 命中条件引入 workspace state / artifact fingerprint / tool side-effect guard。
2. 对代码修改、诊断、执行、workflow/task 类任务默认降低 cache 复用等级或直接禁用跳过执行。
3. 将 cache 命中分为“可直接复用回答”和“仅可复用上下文草稿”两类。

验收标准：

1. 需要真实执行的任务不会被 cache 直接短路。
2. 命中缓存时可区分只读问答与会改动状态的执行型任务。
3. governance.status 可观测 cache short-circuit 次数与拒绝原因。

---

### AUTON-04 — Full-auto 路径存在占位式工具抽取，未达到生产级自治

优先级：P1

当前状态：

1. Chat 链路中已经接入 `execute_loop` 抽象。
2. 系统会尝试根据响应提示选择 preferred tools。

差距：

1. `extract_tool_calls_from_response` 当前仅通过是否包含 `tool`、`function`、`call` 关键字返回 `simulated_tool_call`。
2. 这不是结构化动作抽取器，无法稳定驱动真实自治 loop。
3. 现有 full-auto 设计更像验证框架或占位逻辑，而不是主生产路径。

推荐行动：

1. 用结构化 action schema 替换字符串包含式工具抽取。
2. 将 `execute_loop` 的动作来源统一为模型显式 action plan 或协议化 tool call。
3. 为 full-auto 模式补齐真实 trace、失败分类、重试策略与回退策略。

验收标准：

1. 不再出现 `simulated_tool_call` 这类占位动作进入主链路。
2. full-auto 触发的每一步都有真实工具、真实输入、真实输出、真实停止原因。
3. 集成测试可覆盖至少一个多步自治成功案例与一个失败回退案例。

---

### AUTON-05 — 工具治理存在默认放行退化路径，自治行为缺乏稳定边界

优先级：P1

当前状态：

1. `execute_mcp_tool_call` 已具备 budget、policy、sandbox 等治理逻辑。
2. HarnessBus 支持 RBAC 判定。

差距：

1. 当 RBAC enforcer 未配置时，当前逻辑会直接允许所有工具。
2. 不同部署环境下会出现自治能力强弱不一致、边界不一致的问题。
3. 默认放行使“自动完成所有工作”与“受控安全执行”之间缺少稳定合同。

推荐行动：

1. 将 RBAC 未配置从“默认允许”改为“显式 deployment policy 决定”。
2. 区分只读工具、低风险写工具、高风险执行工具的默认权限集。
3. 将 permission denials、fallback policy、sandbox level 纳入治理状态输出。

验收标准：

1. 不同 profile 下工具权限模型清晰一致。
2. 无 RBAC 时也有明确且可观测的最小权限策略。
3. 工具被拒绝时可解释、可审计、可重试。

---

### AUTON-06 — 自治能力碎片化分布，CLI/ACP/CapabilityBus 三套实现未统一

优先级：P0

当前状态：

1. CLI 聊天链路在工具执行后，会将工具结果回送给 agent 做 follow-up。
2. ACP chat 主链路会执行工具，但只把结果拼接到响应文本中。
3. CapabilityBus / ToolBus / BrainLoop 也具备独立的自治执行抽象。

差距：

1. 项目中存在多套“更完整自治”实现，但没有统一成为唯一主执行面。
2. CLI 比服务端更接近真正闭环，说明主路径选型发生了分叉。
3. 结果是功能能力散落在不同入口，用户真实使用的 ACP/chat 路径反而拿不到最完整自治能力。

推荐行动：

1. 明确唯一自治主引擎，并将 CLI、ACP、task/workflow 都汇聚到同一 loop runtime。
2. 把 CLI 中“工具结果回注 agent follow-up”的能力上移成共享 runtime，不再保留平行逻辑。
3. 建立自治主链路契约测试，禁止不同入口行为漂移。

验收标准：

1. CLI、ACP、task.execute、workflow.execute 的自治行为一致。
2. 工具调用后的 follow-up 不再依赖某个入口的私有实现。
3. 不再出现“某入口能闭环、主入口只能单轮”的行为分叉。

---

### AUTON-07 — 多-agent 架构大量存在于构件层/观测层，未形成真实执行编排

优先级：P1

当前状态：

1. `OrchestrationCouncil`、`ExecutionGraph`、`Planner/Executor`、`BrainLoop`、`ToolBus` 等模块均有完整结构体与测试。
2. `CapabilityBusProfile`、`governance.status` 等对这些能力有状态暴露。
3. `process_chat_request` 中确实存在高风险多 agent vote 等局部多 agent 逻辑。

差距：

1. 很多多-agent 模块主要停留在独立库、profile enrich、或可观测快照层，没有被 ACP 主执行链稳定调用。
2. `Planner::plan` 在 chat 主链路里目前仅用于生成轻量 observability plan，而非驱动真实执行。
3. `ExecutionGraph` 仍带大量 `planned wiring` 标记，说明复杂 DAG 编排未正式接入。
4. `OrchestrationCouncil` 主要表现为可独立使用的投票容器，不是当前任务执行的主协调器。

推荐行动：

1. 将多-agent 架构分层为“观测模块”和“执行模块”，明确哪些是生产主线，哪些是储备架构。
2. 把真实任务执行统一接入 Planner -> Orchestrator -> Tool Loop -> Review 的单一编排总线。
3. 在主链路中显式接入 Council / Graph / Planner，而不是仅暴露 profile 计数。

验收标准：

1. 多-agent 模块不再只是 profile 中的状态数字。
2. 至少一条 ACP/task/workflow 主链路由统一编排层驱动。
3. 多-agent 协同结果可以从请求入口追踪到具体编排节点与投票/分支决策。

---

### AUTON-08 — readiness/gate 指标大量是布尔链推导，不能证明真实自治可用

优先级：P1

当前状态：

1. `runtime_pack.rs`、`ops_pack.rs` 暴露大量 `*_ready`、`*_gate`、`brain_loop_ready` 等字段。
2. 这些字段被聚合到 runtime / ops 状态输出中。

差距：

1. 多个 readiness 字段是由前置布尔值递推组合出来的，并不是对真实主链路执行的验证结果。
2. 这类状态更像“架构声明满足度”，不是“AI 真能自动完成任务”的行为证据。
3. 容易造成外部感知误差：面板看起来 ready，但真正执行路径仍是浅层或旁路实现。

推荐行动：

1. 将 readiness 指标拆分为“静态接线状态”和“动态行为验证状态”。
2. 对 brain loop、tool loop、multi-agent council 等引入真实 smoke 或 replay 校验结果。
3. governance.status 中新增 behavior-backed 指标，如 `autonomy_loop_success_rate`、`tool_followup_enabled`、`clarification_resume_success_rate`。

验收标准：

1. readiness 不再只是布尔代数链。
2. 关键自治能力都有真实行为校验来源。
3. 面板中的 ready 可以直接映射到主链路可用性。

---

### AUTON-09 — Auto-repair 是运行时重试/重放，不是 AI 驱动的诊断式修复

优先级：P1

当前状态：

1. `task.execute` / `workflow.execute` 已包含 auto_repair 开关、repair context、repair history。
2. 失败后可以进入 repair loop 并再次执行 runtime subtasks。

差距：

1. 当前 auto-repair 主要是对失败子任务的重跑与策略调整，不是基于模型诊断结果的实质修复。
2. repair loop 并没有自然连接到统一的“分析失败原因 -> 选择工具/修改计划 -> 再执行”的自治链路。
3. 这会表现为“会重试，但不一定更聪明”，容易给用户造成“只是跑跑看”的观感。

推荐行动：

1. 将 repair loop 接入统一自治引擎，让失败原因反哺下一轮计划和工具选择。
2. 区分 retry、reroute、replan、repair 四类动作，而不是都落成 rerun。
3. 在 repair history 中记录 AI 诊断结论、修复动作、结果差异。

验收标准：

1. auto_repair 不再只是重复执行相同路径。
2. 每轮 repair 都能说明为什么修、修了什么、为什么有效或无效。
3. 失败任务的恢复率可观测，且与纯重试区分统计。

---

### AUTON-10 — 断点恢复与幂等命中可能让执行“看似完成”，但没有继续推进真实工作

优先级：P2

当前状态：

1. `task.execute` 带有幂等缓存，命中后直接返回已缓存响应。
2. 执行链路会持久化 checkpoint、`resume_eligible`、repair history 等元数据。

差距：

1. 幂等命中虽然防重复执行，但也会让外部感知变成“请求被立即回答”，而非继续推进当前真实状态。
2. checkpoint / resume 目前更偏执行产物保留，不等于自治 loop 会自动从中断点恢复。
3. 若没有细粒度状态指纹，重复 task 描述可能命中旧结果，进一步加重“简单跑跑就结束”的观感。

推荐行动：

1. 将幂等键与工作状态、checkpoint 代际、artifact 指纹绑定，而不是只绑 task 文本与 phase。
2. 把 checkpoint/resume 接入真实自治恢复路径，而不是仅返回 `resume_eligible` 标志。
3. 增加“命中缓存但存在待续执行”的区分状态，避免误报完成。

验收标准：

1. 幂等响应不会吞掉应继续推进的执行。
2. checkpoint/resume 可以实际恢复自治流程。
3. 用户能区分“复用结果”和“继续执行”。

---

## 3. 多轮深度扫描结果（Round 1 - Round 4）

### 3.1 Round 1 — 主聊天链路与工具调用闭环

扫描范围：

1. `src/acp/impl/chat.rs`
2. `src/acp/impl/request/tools_pack.rs`
3. `src/acp/helpers/requirement.rs`
4. `src/acp/impl/request/exec_pack.rs`
5. `src/acp/impl/request/workflow_pack.rs`

结论：

1. 主聊天链路具备真实工具调用能力。
2. 但工具执行结果没有回注模型形成多轮思考。
3. requirement gate 与 confirm 流程会直接阻断复杂任务执行。
4. token cache 与 task idempotency 都可能将真实执行短路为快速返回。

本轮新增判断：

1. “能自动调用工具”已经成立。
2. “能基于工具结果持续推进任务直到完成”尚未成立。
3. 主阻断点在 chat/tool/requirement/cache 四条链上。

### 3.2 Round 2 — 自治实现碎片化对比：CLI vs ACP vs Tool Loop

扫描范围：

1. `src/cli/chat.rs`
2. `src/orchestration/tool.rs`
3. `src/acp/impl/chat.rs`

结论：

1. CLI 路径已经实现“工具执行后将结果回送 agent follow-up”的更完整闭环。
2. ACP 服务端主链路没有复用这条能力，而是只把工具结果附加到文本末尾。
3. `execute_loop` 具备 Think-Act-Observe 抽象，但 ACP 接入仍依赖占位式工具抽取。

本轮新增判断：

1. GO-ON 缺的不是自治思路，而是自治主引擎统一。
2. 目前最像“钢铁侠战衣”的路径并不是用户主用的 ACP/chat 路径。
3. 如果不先统一执行面，后续继续叠功能只会加重系统分裂。

### 3.3 Round 3 — 多-agent 架构是否真正接入执行主链路

扫描范围：

1. `src/intelligence/capability_bus/core.rs`
2. `src/intelligence/capability_bus/tool_bus.rs`
3. `src/orchestration/council/council.rs`
4. `src/orchestration/planner_executor.rs`
5. `src/orchestration/execution_graph.rs`
6. `src/orchestration/loop/brain_loop.rs`

结论：

1. 项目中确实存在非常丰富的多-agent、多总线、多编排组件。
2. 其中相当一部分已经具备结构化 API、测试和 profile 输出。
3. 但这些模块大多未成为 ACP/chat/task/workflow 的统一生产执行面。
4. `Planner::plan` 在 chat 主链路里仅用于 observability 计划展示，不驱动真实执行。
5. `ExecutionGraph` 明显仍处于 planned wiring 状态。
6. `BrainLoop` 在 HarnessBus 中当前主要用于 profile 暴露，而非主任务执行。

本轮新增判断：

1. 现在的多-agent 架构更像“装甲仓库”，不是“已穿上的战衣”。
2. 系统已经有很多零件，但它们还没有被铆成一个统一、可实战的自治外骨骼。
3. 最后一轮集中修复前，必须先完成“唯一主引擎收口”。

### 3.4 Round 4 — readiness、repair、checkpoint 是否造成假闭环

扫描范围：

1. `src/acp/impl/request/runtime_pack.rs`
2. `src/acp/impl/request/ops_pack.rs`
3. `src/acp/impl/request/exec_pack.rs`
4. `src/governance/harness_bus.rs`

结论：

1. 大量 `*_ready`、`*_gate`、`brain_loop_ready` 字段主要是布尔链推导，不是行为级验证。
2. auto_repair 已存在，但更接近 rerun/replay，而不是 AI 诊断式修复。
3. checkpoint、resume_eligible、idempotency 已存在，但还没有自然闭合成真实自治恢复链。
4. 因此系统容易在状态层“显得很完整”，但在行为层仍然表现为浅跑或阻断。

本轮新增判断：

1. 不能再把 readiness 面板当作自治能力充分证据。
2. 后续修复必须优先改行为闭环，再回填观测指标，而不是继续增加 ready 字段。
3. “钢铁侠战衣”目标的关键不是多几个 bus，而是让 bus 真正穿到主链路上。

### 3.5 Round 5 — 编译选项 / profile / feature 差异是否导致自治能力不一致

扫描范围：

1. `Cargo.toml`
2. `src/lib.rs`
3. `src/main.rs`
4. `src/intelligence/capability_bus/core.rs`
5. `src/intelligence/capability_bus/tool_bus.rs`
6. `src/orchestration/mod.rs`
7. `src/agents/mod.rs`

编译验证：

1. `cargo check -q --no-default-features --features profile-local`
2. `cargo check -q --no-default-features --features profile-simple-server`
3. `cargo check -q --no-default-features --features profile-multi-users-server`

结论：

1. 三种官方 profile 当前均可编译通过，不存在“某一官方 profile 直接编译阻塞”的问题。
2. 但三种 profile 下自治能力并不等价，存在“可编译但行为退化”的差异。
3. `profile-local` 会对 cache/vector 初始化失败采取 adaptive continue 策略，这意味着它更容易进入“少一条腿也先跑起来”的降级模式。
4. `profile-simple-server` 与 `profile-multi-users-server` 对 cache/vector 初始化更严格，行为更接近“缺依赖即报错”。
5. `sub-bus-tool`、`sub-bus-orchestration`、`sub-bus-observability` 等是自治主能力的重要编译开关，当前官方 profile 都会带上，但代码本身仍保留 `not(feature = ...)` 退化分支。
6. `CapabilityBus::execute_tool` 在没有 `sub-bus-tool` 时会直接返回 `ToolBus not available in this profile`，说明自治执行层对 feature 仍有硬依赖，只是当前三种官方 profile 恰好覆盖了它。
7. `distributed_memory`、remote skill import、部分 protocol/memory 扩展只在 `profile-multi-users-server` 下成立，因此多-agent 的跨节点形态本身就不是三 profile 等价能力。

本轮新增判断：

1. 现在的问题不是“官方 profile 编不过”，而是“官方 profile 行为闭环不一致，且代码库保留了大量 feature 降级分支”。
2. 如果以后有人引入新的 feature 组合，或拆分 sub-bus 编译开关，自治主链路很容易重新掉进 `not(feature=...)` 的退化分支。
3. 真正的修复目标应是：官方支持矩阵内不允许任何自治关键路径退化成阻塞、空工具、假 ready 或自适应绕过主能力。

### 3.6 编译差异矩阵（自治视角）

| 维度 | profile-local | profile-simple-server | profile-multi-users-server |
|------|---------------|-----------------------|----------------------------|
| 官方编译状态 | 通过 | 通过 | 通过 |
| `sub-bus-tool` | 有 | 有 | 有 |
| `sub-bus-orchestration` | 有 | 有 | 有 |
| `sub-bus-observability` | 有 | 有 | 有 |
| `sub-bus-memory` | 无 | 有 | 有 |
| `sub-bus-protocol` | 无 | 有 | 有 |
| `sub-bus-distributed-memory` | 无 | 无 | 有 |
| cache init 失败 | 可继续运行（adaptive） | 失败即中断 | 失败即中断 |
| vector init 失败 | 可继续运行（adaptive） | 失败即中断 | 失败即中断 |
| 分布式记忆 / 跨节点协同 | 无 | 无 | 有 |
| 自治风险 | 易进入降级运行但不显性失败 | 能力较完整但仍有主链路碎片 | 能力最全但复杂度最高 |

矩阵解读：

1. `profile-local` 最容易出现“能跑，但少组件、少记忆、少持久化”的软退化。
2. `profile-simple-server` 是当前最接近“单机完整自治运行时”的 profile。
3. `profile-multi-users-server` 提供最多总线和分布式能力，但这不等于主链路已经把这些能力真正穿起来。
4. 因此“所有官方 profile 均可编译”不代表“所有官方 profile 均无自治阻塞”。

### 3.7 编译选项相关新增修复原则

1. 官方支持矩阵只允许三种 profile，禁止出现未声明 feature 组合下的自治承诺。
2. 对自治关键路径，官方 profile 内不允许存在 `ToolBus not available`、空工具集、空 orchestrator、占位 planner 这类软阻塞。
3. `profile-local` 的 adaptive fallback 必须可观测，并且不能把主链路悄悄降成“简单跑跑”。
4. `profile-simple-server` 与 `profile-multi-users-server` 应作为自治闭环验证硬门，保证完整执行行为一致。
5. `governance.status` 必须区分“feature present”和“behavior active”，避免编译特性存在但行为没接线。

---

## 4. 当前结论：能调工具，但还不能稳定自主完成整项工作

### 3.1 已达到的层级

当前系统已经达到：

1. 模型可声明工具调用。
2. 后端可执行真实工具。
3. 任务/工作流/澄清/确认等辅助链路已经存在。
4. 具备一定的缓存、路由、超时、fallback、治理基础设施。

### 3.2 尚未达到的层级

当前系统尚未达到：

1. 多轮自治执行闭环。
2. 工具观察结果驱动的持续推理与重规划。
3. 对复杂任务的自动澄清后续跑。
4. 对执行型任务避免 cache 浅层命中。
5. full-auto 主链路生产级闭合。

### 3.3 结论判断

结论：GO-ON 当前已具备“自动调用工具的基础能力”，但还不具备“稳定自动分析问题并自动完成所有工作”的生产级自治闭环。

更准确地说：

1. 现在是“可自动触发工具”，不是“可持续自治执行直到任务完成”。
2. 现在是“存在自治相关模块”，不是“自治主链路已经闭合”。
3. 现在的主要风险不是完全缺功能，而是关键闭环断在工具回注、确认门禁、缓存短路和占位逻辑上。

---

### 4.1 总体判断升级

经过多轮扫描，当前结论较第一版进一步收紧：

1. GO-ON 已经具备“自动调用工具”的多个真实实现。
2. 但这些实现没有统一成 ACP/chat/task/workflow 的唯一自治主引擎。
3. 多-agent 组件丰富，但大量仍停留在架构储备、观测聚合、profile enrich 或独立容器层。
4. 真正导致用户感知“阻塞”或“简单跑跑就完”的，不只是单点 bug，而是执行面分裂、门禁阻断、缓存短路和假闭环指标叠加。
5. 编译 profile 虽然都能过，但 profile-local 的 adaptive 降级、feature-gated 的退化分支、以及多 profile 行为差异，都会放大这种问题。
6. 如果直接在现状上继续补 bus、补 gate、补 profile，只会让系统更重、更像包袱，而不是钢铁侠战衣。

### 4.2 收口原则

后续一次性修复必须遵循：

1. 唯一自治主引擎。
2. 唯一工具观察回注机制。
3. 唯一 clarify/confirm/repair/resume 状态机。
4. readiness 以行为验证为准，不再以布尔链为准。
5. 三个官方 profile 的自治行为必须等价到“无阻塞、无假完成、无空转”。

---

## 5. 迭代建议（3 周）

第 1 周：

1. 完成 AUTON-01，建立多轮工具执行状态机。
2. 为自治 loop 增加治理指标、trace 与停止原因输出。
3. 补齐多步工具调用集成测试。

第 2 周：

1. 完成 AUTON-02，将 requirement gate 改为可恢复状态机。
2. 打通 `workflow.clarify` / `workflow.confirm` 到 `task.execute` 的自动续跑。
3. 完成中低风险自动澄清策略与高风险人工确认策略分流。

第 3 周：

1. 完成 AUTON-03，对执行型任务收紧 cache 短路策略。
2. 完成 AUTON-04，替换占位式 tool extraction。
3. 完成 AUTON-05，收紧默认工具权限与 profile 级权限合同。
4. 完成 AUTON-06 ~ AUTON-10 的主引擎收口与行为指标替换。
5. 补齐三种官方 profile 的自治行为一致性测试与 feature regression 测试。

---

## 6. 完成定义（Definition of Done）

每项功能必须满足：

1. cargo check 通过。
2. cargo clippy 在对应目标集零 warning。
3. 多轮自治链路有可执行集成测试覆盖。
4. governance.status 可观测自治 loop 次数、停止原因、cache short-circuit、permission denial、clarification state。
5. backend、GUI、vscode-addon 对自治状态字段理解一致。
6. 不再依赖占位动作名、字符串包含式抽取或错误即终止的确认门禁。
7. CLI、ACP、task/workflow 不再各自维护一套自治逻辑。
8. readiness/gate 指标有真实行为校验来源。
9. 三种官方 profile 下自治主链路行为一致，不因 cache/vector/tool/orchestration feature 差异而阻塞或空转。
10. 文档与变更回写到对应 BLUE 文件。

---

## 7. 关键证据索引（多轮扫描）

1. `src/acp/impl/chat.rs`：`process_chat_request`、`run_agent_collecting`、`extract_tool_calls_from_response`
2. `src/acp/impl/request/exec_pack.rs`：`handle_task_execute`
3. `src/acp/impl/request/workflow_pack.rs`：`handle_workflow_confirm`、`handle_workflow_clarify`
4. `src/acp/helpers/requirement.rs`：`evaluate_requirement_gate`
5. `src/acp/impl/request/tools_pack.rs`：`execute_mcp_tool_call`
6. `src/governance/harness_bus.rs`：RBAC、brain profile、token gate、governance profile
7. `src/orchestration/tool.rs`：`execute_loop`
8. `src/cli/chat.rs`：`run_agent_with_tools`、`agent_followup`
9. `src/intelligence/capability_bus/core.rs`：`execute_tool`、profile enrichment、tool bus / council wiring
10. `src/intelligence/capability_bus/tool_bus.rs`：`execute_tool`、`agent_tool_match`
11. `src/orchestration/loop/brain_loop.rs`：legacy note、plan/execute/reflect/replan loop
12. `src/orchestration/planner_executor.rs`：`Planner::plan`、`Executor::execute`
13. `src/orchestration/execution_graph.rs`：`planned wiring` DAG 编排
14. `src/orchestration/council/council.rs`：proposal/vote/tally 多-agent 容器
15. `src/acp/impl/request/runtime_pack.rs`、`src/acp/impl/request/ops_pack.rs`：ready/gate 布尔链
16. `Cargo.toml`、`src/lib.rs`：profile 与 sub-bus feature 矩阵
17. `src/main.rs`：profile-local adaptive fallback 与 server profile 严格初始化差异

---

## 8. 一次性修复前的总优先级顺序

1. 先统一自治主引擎，再修功能点。
2. 先打通工具结果回注和多轮 loop，再处理 repair/resume。
3. 先把 clarify/confirm 从硬阻断改成状态机，再讨论更复杂的多-agent council 协作。
4. 先用行为指标替换 ready/gate 假闭环，再扩展 observability 面板。
5. 先清理主链路分叉（CLI vs ACP vs CapabilityBus），再做 profile 完善。
6. 先把三种官方 profile 的自治行为收平，再开放更细粒度 feature 组合承诺。

---

## 9. 本轮完成率

1. Round 1 主聊天链路与工具闭环扫描：100%（已完成主链路复扫并修复占位式工具抽取）
2. Round 2 CLI/ACP/Tool Loop 差异扫描：90%（requirement 状态载荷补齐 clarification/confirm 显式字段，入口语义继续收敛）
3. Round 3 多-agent 编排模块接线扫描：68%（新增 execution_plan 与 tool trace 对齐分析，编排节点与执行轨迹关联增强）
4. Round 4 readiness / repair / checkpoint 假闭环扫描：93%（repair 循环新增 resolved/improved/unresolved 计数与恢复率指标，行为验证更完整）
5. Round 5 编译选项 / profile / feature 差异扫描：100%（三 profile 编译/静态检查已复验）
6. 三种官方 profile 编译验证：100%（local/simple-server/multi-users-server 全通过）
7. 多轮证据整合与优先级归类：98%（repair 结果分类与恢复率已接入自治指标，证据链可解释性进一步提升）
8. 代码修复实施：95%（已推进 AUTON-01/02/03/05/06/07/08/09/10，继续收口 AUTON-07 深层编排契约）

### 9.1 跨平台验证补充（本轮）

1. macOS target（aarch64-apple-darwin）：`cargo check` 通过。
2. Linux target（x86_64-unknown-linux-gnu）：当前机器未安装 rustup target，交叉编译未执行完成。
3. Windows target（x86_64-pc-windows-msvc）：当前机器未安装 rustup target，交叉编译未执行完成。
4. 三 profile 严格门：`cargo clippy --no-default-features --features profile-{local|simple-server|multi-users-server} -- -D warnings` 全通过。
5. 已安装 targets：`aarch64-apple-darwin`、`wasm32-unknown-unknown`、`wasm32-wasip1`；Linux/Windows target 需补装后可执行同等交叉编译验证。

### 9.2 AUTON-01~10 完成率（本轮回写）

1. AUTON-01：89%（工具观察后 follow-up 链路稳定，主聊天执行闭环继续增强）
2. AUTON-02：79%（requirement gate 响应补齐 auto_confirmable/clarification/human-confirmation 状态字段，阻断语义更清晰）
3. AUTON-03：92%（执行型请求 cache bypass 已落地并在主链路生效）
4. AUTON-04：90%（占位式 tool-call 抽取已替换为显式标记抽取）
5. AUTON-05：79%（execute_mcp_tool_call 已接入 HarnessBus 准入门，RBAC/预算/沙箱拒绝分类可观测）
6. AUTON-06：88%（CLI/ACP follow-up helper 已共享，分叉继续收敛）
7. AUTON-07：74%（新增 execution_plan 与 tool-loop trace 对齐分析并纳入输出，编排接线可追踪性提升）
8. AUTON-08：92%（autonomy metrics 新增 repair 循环结果计数与有效恢复率，行为验证覆盖面继续扩大）
9. AUTON-09：88%（repair_history 新增 cycle 恢复率与 replan_required 提示，repair 从重试转向可诊断）
10. AUTON-10：91%（task.execute 幂等命中已区分 continuation pending 并返回恢复 next_step，避免误报完成）

