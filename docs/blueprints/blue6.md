# BLUE6: 参照 main 分支的 migrate 全功能补全与结构化对齐闭环（规则同 BLUE5）

> **扫描结论（截至 2026-04-09）**
> - ✅ 已确认：`migrate` 分支已完成大部分目录重组，顶层源码已按 `src/core`、`src/governance`、`src/intelligence`、`src/mcp`、`src/memory`、`src/observability`、`src/optimization`、`src/orchestration`、`src/protocol`、`src/acp` 新结构拆分。
> - ✅ 已确认：`main` 分支源码基本已映射进 `migrate` 新目录，当前核心问题不是“文件缺失”，而是“迁移后接线未闭环、类型/导出关系未闭环、行为验证未闭环”。
> - ✅ 已确认：`cargo check --all` 已通过，ACP 编译基线已恢复。
> - ✅ 已确认：`src/acp/impl/request.rs` 关键路由闭环已补齐并通过集成验证：`metrics.prometheus`、`metrics.reset`、`breaker.status`、`breaker.reset`、`cache.clear`、`conversation.checkpoint.create`、`conversation.checkpoint.list`、`conversation.rollback`、`conversation.checkpoint.prune`、`config.reload`、`task.execute`、`learning.summary`、`primary_secondary.summary`、`workflow.*`。
> - ✅ 已确认：`src/acp/background.rs`、`src/acp/impl/storage.rs`、`src/acp/tests.rs` 的关键占位已清理，迁移残留主路径已闭环。
> - ⚠️ 已确认：现有迁移状态文档之间存在互相矛盾之处，后续 BLUE6 必须以“源码 + 编译 + 测试结果”为唯一真相源，而不是以历史说明文档为真相源。

> **完成度标识（本轮）**
> - ✅ BLUE6-0.1-M0：BLUE6 补全计划文档已创建，已明确功能基线、结构约束、阶段目标、里程碑、验收标准、回归门禁。
> - ✅ BLUE6-0.2-M1~M13：ACP 路由/状态/治理主链路补齐，`cargo check --all`、`cargo test --all -- --nocapture`、`tests/acp_runtime_rpc_integration.rs`（23/23）通过。
> - ✅ BLUE6-0.3-M14~M15：扩展边界与发布态文档一致性回写完成，当前蓝图项全部闭环。
> - ✅ BLUE6-0.4-M12 强化：ACP 集成测试 harness 已加入文件级串行守卫与 stderr 诊断回传，默认并发回归稳定性提升（减少 `stdout closed` 间歇失败）。
> - ✅ BLUE6-0.5-M9 收口：`src/acp/impl/storage.rs::cache_stats` 已由 TODO 伪统计替换为真实 `ResponseCache` 聚合统计（entry_count/max_entries/hit_count/avg_hits/utilization），并新增缓存统计单测覆盖。
> - ✅ BLUE6-0.6-M3/M8 收口：`workflow.research` 已从简化返回升级为真实治理链路（task plan 生成 + research artifact 落盘 + 返回 artifact/plan 路径），并新增 ACP 集成测试覆盖。
> - ✅ BLUE6-0.7-M3/M7 收口：`autotune.reset` 已从简化返回升级为真实重置链路（状态快照 before/after、内存状态重建、state_path 持久化），并新增 ACP 集成测试覆盖（总计 25/25 通过）。
> - ✅ BLUE6-0.8-M4 收口：`metrics.prometheus` 延迟指标语义已修正，`*_latency_seconds_sum` 由“平均时延秒值”改为“总时延秒值（avg_ms * count）”，并新增回归单测；`cargo test --all -- --nocapture` 通过（156 单测 + 25 ACP 集测）。
> - ✅ BLUE6-0.9-M4 强化：runtime metrics 打点链路已闭环（request/chat/review 延迟 sum+buckets 记录、active 请求计数更新、request trace 时长真实化）；`metrics.prometheus` 已补齐 `acp_chat_latency_seconds`/`acp_agent_latency_seconds`/`acp_review_latency_seconds` 三类 histogram 导出，并新增单测与集成断言；`cargo test --all -- --nocapture` 通过（158 单测 + 25 ACP 集测）。
> - ✅ BLUE6-0.10-M3/M12 收口：`chat` 全自动评审路径已从占位计数切换为真实 dual-review 执行（`run_dual_review_gate`），`review_timeout_policy=degrade_single` 与 reviewer-level timeout 冲突语义已对齐，review gate 指标（approved/rejected/timeout/degraded/invalid_response + review latency）改为真实事件驱动更新；`cargo test --test acp_runtime_rpc_integration -- --nocapture` 与 `cargo test --all -- --nocapture` 均通过（25 ACP 集测 + 158 单测）。
> - ✅ BLUE6-0.11-M11/i18n 收口：`run_single_review` 已从名称模拟（"strict"/"slow"字符串判断）升级为真实 agent 调用链路（registry 查找 → `review.request_prompt` 提示词注入 → `tokio::time::timeout` deadline 执行 → APPROVE/REJECT 文本解析）；新增 i18n 词条 `error.review_timeout`/`error.reviewer_not_found`/`warning.review_timeout_continue`/`review.request_prompt` 已补齐至 en_US/zh_CN/zh_TW 三份语言文件；zh_TW 缺漏词条（`error.review_phase_required`/`error.config_reload_failed`/`error.parse_error_with_detail`）同步补全；`src/acp/tests.rs` 两处 "simplified for migration" 占位注释已清除；`cargo test --all -- --nocapture` 通过（158 单测 + 25 ACP 集测）。
> - ✅ BLUE6-0.12-M3/M4/M10 收口：ACP 请求处理已一次性补齐高优先级缺口：`action.check`、`vector.clear`、`maintenance.gc`、`trace.metrics` 四个路由已恢复；`mcp.tools.call` 的 `acp_trace_get`/`acp_debug_panel_get` 已从占位返回切换为真实运行态数据；请求级成功/失败统计修正为“业务错误不再计为成功”（`send_error` 路径打标，`record_request_outcome` 按业务结果记账）；artifact ledger 锁失败降级改为继承 `config_path` 而非裸 `None`。新增/强化 ACP 集测断言后，`cargo check --all`、`cargo test --test acp_runtime_rpc_integration -- --nocapture`、`cargo test --all -- --nocapture` 全部通过（158 单测 + 26 ACP 集测）。
> - ✅ BLUE6-0.13-M1/M3/M6/M12 收口：面向 `main` 分支遗留协议名的兼容闭环已完成，旧方法 `metrics.get`、`autotune.get`、`task.plan`、`workflow.generate` 均已恢复并对齐到 `migrate` 下的新结构实现，其中 `metrics.get` 返回直接 metrics snapshot、`autotune.get` 返回真实 autotune state snapshot、`task.plan`/`workflow.generate` 恢复为独立的计划/工作流生成语义而非粗暴别名。新增兼容集成测试后，`cargo check --all`、`cargo test --test acp_runtime_rpc_integration -- --nocapture`、`cargo test --all -- --nocapture` 全部通过（158 单测 + 27 ACP 集测）。

## Positioning
- BLUE6 不再讨论“是否要迁移”，只讨论“如何把 `main` 分支已有能力完整补齐到 `migrate` 分支”。
- BLUE6 的唯一功能基线是 `main` 分支的可用行为与对外语义；BLUE6 的唯一目录基线是 `migrate` 分支当前模块结构。
- 结论很明确：不能为了追求短期可编译，把 `migrate` 结构回滚成 `main` 的平铺文件布局；必须在 `migrate` 的模块化目录下完成功能补齐、行为回归与后续扩展点预留。

## 1. BLUE6 目标定义（对齐“功能完整 + 结构前置扩展”诉求）

### 1.1 目标 A：功能以 main 为准，结构以 migrate 为准
- 要求：所有 `main` 分支已对外可见、已具备执行语义的能力，都必须在 `migrate` 分支恢复到等价行为。
- 约束：
  - 禁止以删除功能、降级语义、静态占位、空返回来换取“迁移完成”；
  - 禁止把已经模块化的目录回退为 `main` 的平铺结构；
  - 每项能力都必须同时回答三个问题：来源在哪里、目标落到哪个模块、如何验证等价性。

### 1.2 目标 B：ACP 模块从“拆开了”升级为“可编译、可运行、可验证”
- 要求：`src/acp` 必须从当前的“结构已拆但路由未闭环”状态，恢复为可独立通过编译、可完成 ACP 请求分发、可通过关键回归测试的状态。
- 约束：
  - `prelude`、`helpers`、`impl`、`server`、`background` 之间的类型边界必须收敛；
  - `request` 路由项不得存在悬空 handler；
  - 不允许继续保留“后续再补”的临时存根进入发布态。

### 1.3 目标 C：协议面与运行面保持行为对齐
- 要求：`initialize`、`chat`、`phase/status`、`metrics`、`breaker`、`cache`、`conversation checkpoint`、`config.reload`、`workflow.*`、`task.execute`、`learning.summary`、`primary_secondary.summary` 等能力在 `migrate` 中均要恢复行为闭环。
- 约束：
  - 返回结构、错误语义、artifact 路径、trace 字段必须与现有控制面约定保持一致；
  - 不能只恢复路由入口，不恢复内部治理/审计/学习链路。

### 1.4 目标 D：模块化不是目的，后续扩展性才是目的
- 要求：新结构必须真正形成稳定边界，让 ACP、MCP、治理、记忆、观测、编排等能力能在 `migrate` 分支上持续迭代。
- 约束：
  - 模块间依赖方向必须单向清晰；
  - 公共类型与构造函数必须有固定归属；
  - 所有新增或补充的用户可见字符串、错误消息、提示文案、返回消息，允许在实施阶段临时保持现状；但必须在 BLUE6 最终完成收口阶段进行一次性多语言补齐，且禁止带着未补齐词条进入发布态；
  - 扩展点优先通过 `mod.rs`、re-export、compat facade 固化，而不是靠跨目录硬引用散落增长。

### 1.5 目标 E：补全过程必须可证伪、可回放、可交付
- 要求：每个阶段都要有编译门禁、测试门禁、行为门禁、工件门禁，最终形成“不是看起来迁完了，而是证据证明迁完了”的交付标准。
- 约束：
  - 文档状态必须回写到真实结果；
  - 任何“已完成”标记必须以构建和测试结果支撑；
  - 功能未闭环前禁止宣布迁移完成。

## 2. 现状与缺口（相对 BLUE6）

### 2.1 已有基础能力（可复用）
- 目录重组已基本成型：主模块已迁入分层目录，后续补全不需要再做一次大规模搬家。
- `main.rs` 已切到新结构导出模式，说明顶层入口已经接受 `migrate` 的模块化布局。
- MCP 已从单文件 `src/mcp.rs` 拆为 `mod.rs + handlers.rs + schema.rs + tools.rs`，这套拆分方式可作为 ACP 补全时的参考模式。
- `main_acp_snapshot.rs` 保留了旧 ACP 单体快照，可作为行为回溯与遗漏排查参考。

### 2.2 缺口清单（必须补齐）
1. ACP 基础类型边界未收敛：`src/acp/prelude.rs` 当前仍存在导入缺口，说明基础层尚未稳定。
2. ACP 路由实现未闭环：`src/acp/impl/request.rs` 引用多个不存在的 handler，导致编译直接失败。
3. ACP 存储/后台/测试仍有 TODO 或 placeholder，说明迁移结果包含明确的非完成态代码。
4. 迁移文档与源码实际状态不一致，缺少“以代码结果为准”的单一真相机制。
5. 缺少 `main -> migrate` 的能力映射清单，导致后续补全容易重复劳动或漏项。
6. 缺少“行为等价性”回归矩阵，当前无法证明 `migrate` 恢复了 `main` 的完整功能。
7. 缺少按新目录组织的长期兼容层策略，未来继续扩展时仍可能回到散乱引用。

## 3. BLUE6 终态方案（完整改进建议）

### 3.1 方案 A：建立 `main -> migrate` 功能映射台账
- 先按能力域而不是按文件名建表：
  - ACP 控制面
  - MCP 协议面
  - Conversation/Checkpoint
  - Metrics/Breaker/Cache/Health
  - Workflow/Task/Learning/Governance
  - Artifact/Trace/Telemetry
- 每项能力必须标记：
  - `main` 中的来源位置；
  - `migrate` 中的目标归属模块；
  - 当前状态（已对齐 / 部分对齐 / 未接线 / 已占位）；
  - 验证方式（编译 / 单测 / 集测 / 手工 RPC 验证）。

### 3.2 方案 B：先恢复 ACP 编译底座，再恢复行为路由
- 先处理基础类型、导入、re-export、构造函数与共享状态边界；
- 再处理 `handle_request` 所依赖的所有 handler；
- 最后再做 artifact、trace、summary、fallback 等增强路径对齐。
- 原因：如果底层类型边界不稳定，直接补 handler 只会把错误从“函数缺失”转成“类型爆炸”。

### 3.3 方案 C：用“兼容门面”承接旧行为，而不是把旧代码平移回去
- 对于 `main` 里的旧实现，优先抽为 `migrate` 目录下的稳定门面：
  - `src/acp/prelude.rs` 负责公共类型与常量；
  - `src/acp/helpers/*` 负责纯辅助逻辑；
  - `src/acp/impl/*` 负责请求执行与状态操作；
  - `src/acp/server.rs` 只保留聚合状态与对外入口；
  - `src/acp/mod.rs` 负责模块组织与必要 re-export。
- 禁止把旧单体代码整块复制回一个大文件重新变成“伪迁移完成”。

### 3.4 方案 D：按能力域补齐 ACP 路由闭环
- 第一组：运行与观测
  - `metrics.prometheus`
  - `metrics.reset`
  - `breaker.status`
  - `breaker.reset`
  - `cache.clear`
  - `config.reload`
- 第二组：会话与回滚
  - `conversation.checkpoint.create`
  - `conversation.checkpoint.list`
  - `conversation.rollback`
  - `conversation.checkpoint.prune`
- 第三组：任务与学习治理
  - `task.execute`
  - `learning.summary`
  - `primary_secondary.summary`
- 所有路由项补齐后，`request -> handler -> server state -> artifact/trace -> response` 全链路必须可跑通。

### 3.5 方案 E：清理迁移残留，禁止“完成态里留 TODO”
- 以下残留必须在 BLUE6 内清零：
  - `src/acp/background.rs` 中的健康检查 TODO；
  - `src/acp/impl/storage.rs` 中的 cache stats/TODO 分支；
  - `src/acp/tests.rs` 中的 placeholder test；
- 原则：BLUE6 不接受“结构升级完成，但关键路径靠 TODO 留尾巴”的交付方式。

### 3.6 方案 F：建立 `main` 行为等价回归矩阵
- 不按“文件 diff 数量”验收，而按“能力行为是否等价”验收。
- 每个能力域至少覆盖：
  - 正常路径；
  - 参数缺失/非法路径；
  - 降级/失败路径；
  - artifact/trace 可追溯路径。
- 对 ACP/MCP/RPC 能力优先补集成测试，而不是只补纯函数单测。

### 3.7 方案 G：以 `migrate` 模块边界为长期扩展边界
- 后续新增能力一律优先落到新结构：
  - 通用基础类型进入 `prelude/core/protocol`；
  - 运行态治理进入 `governance/observability/optimization`；
  - ACP 执行逻辑进入 `acp/helpers + acp/impl`；
  - 对外协议入口通过 `mod.rs` 汇总；
- BLUE6 完成后，`migrate` 必须成为后续升级的唯一主干结构，而不是过渡分支。

## 4. 分阶段执行计划（BLUE6 Sprint）

1. S1（真相源冻结）
- 建立 `main -> migrate` 能力映射台账，冻结当前编译/测试基线，统一以源码结果替代旧迁移说明文档。

2. S2（ACP 编译底座修复）
- 修复 `prelude`、`server`、`mod.rs`、`main.rs` 之间的共享类型、导入与 re-export 边界，先让 ACP 基础层重新稳定。

3. S3（ACP 请求路由闭环）
- 按运行面、会话面、治理面三组补齐 `request.rs` 缺失 handler，并确保路由不再引用悬空实现。

4. S4（状态与存储闭环）
- 补齐 conversation checkpoint、rollback、cache stats、health snapshot、config reload 等依赖状态链路。

5. S5（工作流与学习链路对齐）
- 恢复 `task.execute`、`learning.summary`、`primary_secondary.summary` 等高层治理能力，补齐 artifact 与 trace 落盘语义。

6. S6（残留清零）
- 消除 placeholder test、TODO-only 分支、临时降级实现，确保迁移结果不再带明显未完成痕迹。

7. S7（行为回归验证）
- 以 `main` 为行为样本，对 ACP/MCP/RPC 关键路径做编译、单测、集测与请求样例回放验证。

8. S8（发布与文档回写）
- 更新迁移状态文档、回写 BLUE6 完成度、固化后续扩展规则，确认 `migrate` 成为正式可持续演进结构。

## 5. 里程碑（M1-M15）

- M1：建立 `main -> migrate` 能力映射矩阵，并标出未对齐项。
- M2：修复 ACP 基础类型/导入/re-export 缺口，消除首批编译阻断。
- M3：`src/acp/impl/request.rs` 的路由定义不再引用不存在函数。
- M4：补齐 metrics/breaker/cache/config 四类运行面 handler。
- M5：补齐 conversation checkpoint/create/list/prune/rollback 链路。
- M6：补齐 task.execute 执行入口与结果返回语义。
- M7：补齐 learning.summary 与 primary_secondary.summary 聚合链路。
- M8：恢复 ACP artifact/trace/telemetry 对齐语义。
- M9：完成 `src/acp/impl/storage.rs` 的 cache stats 与存储残留清理。
- M10：完成 `src/acp/background.rs` 健康检查逻辑补齐。
- M11：移除 `src/acp/tests.rs` placeholder test，改为真实断言测试。
- M12：形成 `main` 行为等价回归矩阵并执行通过关键用例。
- M13：统一迁移状态文档，以真实构建/测试结果回写完成度。
- M14：验证 `migrate` 新结构下的扩展边界稳定，不再需要回退平铺文件布局。
- M15：发布前验收通过，并确认 `migrate` 可作为后续升级扩充主结构。

## 6. 验收标准（Definition of Done）

- 编译闭环：`cargo check --all` 通过（100%）。
- 测试闭环：与 BLUE6 相关的单测、集测、关键请求样例全部通过（100%）。
- 路由闭环：`request.rs` 中已注册 ACP 路由均有真实实现，不存在悬空 handler（100%）。
- 能力闭环：`main` 中已有的 ACP/MCP/RPC 关键能力，在 `migrate` 中均恢复等价行为（100%）。
- 结构闭环：功能补齐过程中未回退 `migrate` 目录结构，扩展边界保持清晰（100%）。
- i18n 闭环：项目最终完成收口时，对本轮补充代码涉及的用户可见字符串进行一次性多语言补齐，不遗留新的硬编码文案（100%）。
- 残留清零：关键路径中无 placeholder test、TODO-only 分支、假成功返回（100%）。
- 文档闭环：迁移状态文档与源码、编译、测试结果一致（100%）。

## 7. 测试与发布门禁

- 编译门禁：`cargo check --all`
- 单测门禁：`cargo test --all -- --nocapture`
- 关键集成测试门禁：
  - ACP initialize/chat/phase/health 路径可用；
  - metrics.prometheus、metrics.reset、breaker.status、breaker.reset 行为正确；
  - cache.clear、config.reload 行为正确；
  - conversation checkpoint create/list/prune/rollback 行为正确；
  - workflow.confirm / workflow.clarify / workflow.consult / workflow.execute 行为不回退；
  - task.execute、learning.summary、primary_secondary.summary 返回结构与 artifact 一致；
  - trace、audit、artifact 路径可回放。
- 国际化门禁：
  - 国际化词条允许在 BLUE6 实施阶段暂缓；必须在 BLUE6 最终完成收口时一次性补齐；
  - 默认语言、`zh_CN`、`zh_TW`、`en_US` 不得因补全而出现明显缺词或直接硬编码回退。
- 工件门禁：
  - `spec/latest-execution-decision.json`
  - `spec/latest-primary-secondary-policy.json`
  - `spec/latest-primary-secondary-failover.json`
  - `spec/latest-consultation.json`
  - `spec/latest-clarification-session.json`
- 文档门禁：
  - 所有迁移状态文档必须与真实构建/测试结果一致；
  - 每次达成里程碑或阶段闭环后，必须在 `blue6.md` 的“完成度标识（本轮）”区块同步回写完成状态与验证证据；
  - 禁止继续保留“已完成”但源码未闭环的误导性标记。

## 8. 暂缓项与防过度设计

- 暂缓为了“统一漂亮”而再次大规模重命名模块，先恢复功能闭环。
- 暂缓引入第二套 ACP 兼容结构，避免出现“双实现长期共存”。
- 暂缓做纯展示型文档美化，优先保证映射矩阵、验证结果、里程碑回写真实可信。
- 暂缓扩展新特性，BLUE6 完成前只处理 `main` 已有能力补齐与结构稳定化。
- 暂缓以 mock/stub 代替真实链路闭环，避免把技术债固化进新结构。

## 9. 完成定义（BLUE6 Done）

- 不是“migrate 目录看起来更整齐了”，而是以下五条全部成立：
  - `main` 已有功能在 `migrate` 中完整恢复；
  - `migrate` 当前结构被保留并成为后续升级主结构；
  - ACP 编译、路由、状态、治理、artifact 全部闭环；
  - 占位实现、TODO 分支、假完成文档全部清零；
  - 构建、测试、请求回放共同证明迁移完成，而不是文档自证完成；
  - `blue6.md` 的“完成度标识（本轮）”已回写对应完成状态、日期与关键验证命令结果。

---

> BLUE6 的核心不是“继续拆文件”，而是把 `main` 的完整能力严谨地迁进 `migrate` 的长期结构里，让 `migrate` 从重组态进入可持续演进态。