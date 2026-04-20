# BLUE35 — FUTURE 全量扫描剩余改造完整清单（同 BLUE26 规则）

更新时间：2026-04-20

本文沿用 BLUE26 的同一验收规则与收口口径：
- 三端一统（backend / vscode-addon / GUI）
- 主链路完整闭环
- 后端主链路功能完整
- 不留 warning
- 最小修改：仅改与目标直接相关内容；禁止为了过测试而做语义不完整改动
- 完成率必须回写

---

## 扫描结论（对标所有 FUTURE*.MD，对照 BLUE26-BLUE34 已完成项）

BLUE26–BLUE34 已覆盖 FUTURE2–FUTURE6 绝大部分主链路能力。
本轮全量扫描后发现仍有 **16 个工程可落地改造项** 未在任何 BLUE 中实现，归入 S1-S16；加上 S0（冻结）、S17（门禁收口）、S18（发布收口）共 **19 步（S0-S18）**。

各 FUTURE 文件剩余缺口摘要：

| 来源文件 | 剩余核心缺口 | 步骤 |
|---------|------------|------|
| FUTURE.MD §3.3 Skill 4 | 自我推理守卫：假设追踪、证据验证、强制再检验 | S4 |
| FUTURE.MD §3.3 Skill 7 | 启动时仓库上下文异步预加载 | S5 |
| FUTURE.MD §3.3 Skill 8 / §5B.8 | Prompt 架构 8 层分层优化 | S6 |
| FUTURE.MD §5B | Token 成本分层触发架构（L0-L5 + 门控 A/B/C/D） | S7 |
| FUTURE.MD §5C.2 | 双级任务调度器（Task Scheduler + Agent Worker Scheduler） | S8 |
| FUTURE.MD §5C.8 | 带抗饥饿的优先级队列调度策略 | S9 |
| FUTURE.MD §5C.12 | 分叉子代理进程隔离硬化（kill 信号、zombie 防护、per-child 资源限制） | S10 |
| FUTURE3.MD M5 补充 | 通用跨行业角色体系 + 行业自定义关键词映射 | S1, S2 |
| FUTURE4.MD M3 | 能力图谱完整闭环（节点、依赖、风险等级、可替代路径） | S11 |
| FUTURE4.MD M12 | 来源可追溯账本（外部方案 / 模型版本 / 训练数据 / 接入变更） | S12 |
| FUTURE5.MD M6 | 节点信誉系统（成功率、稳定性、成本效率、审查通过率） | S13 |
| future-last.md §2 | 云原生部署基线（Kubernetes Operator / mTLS） | S14 |
| future-last.md §3 | 合规框架工程落地（GDPR/HIPAA/SOC2 数据分类 + 审计索引） | S3 |
| future-last.md §7.1 | 开发者 SDK 基线（Rust client + Python binding） | S15 |
| future-last.md §7.2 | SKILL 市场元数据架构（注册、版本、认证、安全扫描） | S17（含入门禁步骤） |
| config/config.toml + src/orchestration/ | 三工作流并存体系（dev / general / custom）+ 通用 phase 标准集 | S16 |

明确不进入本轮（沿用 BLUE34 剔除原则）：
- consciousness_proxy_metrics / awakening_narrative — 不可验证叙事
- fully_automatic_subai_training_factory — 研究范式，高不确定性
- online_rl_auto_policy_update_in_production — 在线 RL 生产落地风险过高
- cross_region_supernode_full_mesh_global_scheduler — 收益/复杂度比不合适

---

## 扫描范围

- backend：src/**（协议层、执行编排、治理、记忆、工具链、智能体系）
- GUI：GUI/src/** + GUI/src-tauri/src/**
- addon：vscode-addon/src/**
- 契约：contracts/editor-capability-matrix.json
- 门禁脚本：scripts/**
- 配置：config/**

---

## 三种模式功能一致性规则（LOCAL / SIMPLE-SERVER / MULTI-USER-SERVER）

### 核心原则
1. **功能一致性**：三种模式的核心功能链路必须完全一致
2. **场景适配性**：按应用场景要求实现，避免过度或欠缺
3. **零隐藏问题**：不允许存在模式相关的隐蔽 bug 或行为不一致
4. **完整闭合**：所有步骤完成后不留 WARNING、冲突或未解决问题

### 具体规则

#### 1. 必须保持一致的核心功能（跨所有模式）
- **执行引擎**：工作流执行、任务分解、自动修复循环
- **代理系统**：Agent 注册、调用、路由、降级链
- **SKILL 系统**：SKILL 注册、管理、执行、生命周期
- **配置系统**：配置加载、验证、热重载、环境适配
- **监控系统**：指标收集、日志记录、性能监控、健康检查
- **错误处理**：错误分类、恢复策略、用户反馈
- **数据契约**：API 接口、数据结构、版本兼容性

#### 2. 按场景差异化的功能（合理差异化）
- **存储后端**：LOCAL/SIMPLE-SERVER: SQLite；MULTI-USER-SERVER: PostgreSQL
- **认证授权**：LOCAL: 简单本地认证；SIMPLE-SERVER: 基础 HTTP 认证；MULTI-USER-SERVER: 完整 RBAC
- **资源管理**：按场景从宽松到严格配额管理
- **高可用性**：按场景从单点到集群部署

#### 3. 禁止出现的问题（零容忍）
1. 不同模式下代码路径不一致导致的隐蔽 bug
2. 功能在不同模式下行为不一致
3. LOCAL 模式过度实现企业级功能
4. MULTI-USER-SERVER 模式功能不完整

#### 4. 质量保证要求
1. 所有模式下必须零警告编译（cargo check --all-features）
2. 核心功能必须有跨所有模式的集成测试
3. 发布前必须验证所有模式的功能一致性

#### 5. 实施检查清单
- [ ] 核心功能在所有模式下行为一致
- [ ] 差异化功能按场景合理实现
- [ ] 无隐藏问题或潜在冲突
- [ ] 编译零警告，测试全通过
- [ ] 发布验证通过所有模式

---

## BLUE35 实施步骤（S0-S18）

### Step 0：冻结目标与禁增范围（P0）

1. 冻结本轮目标为 S1-S16 所列 16 个改造项。
2. 冻结接口字段（禁止三端字段漂移）。
3. 超出项进入 BLUE36 backlog，不允许范围膨胀。

验收点：
- 本文件唯一目标清单已形成并标记，任何新增必须通过本文件变更流程。

---

### Step 1：通用跨行业角色体系（P0）

**来源：FUTURE3.MD M5 补充说明 — 未在任何 BLUE 中实现**

当前问题：
- `AgentRole` 为硬编码枚举：`Planner / Researcher / Coder / Tester / Reviewer`，仅限 IT/开发领域。
- 无法通过配置动态扩展角色。
- `AgentTaskEnvelope.role` 字段为字符串，但 `TaskRouter` 路由逻辑仅处理 5 个已知枚举值。

改造内容：
1. `AgentRole` 增加 `Custom(String)` 变体，向后兼容已有 5 个固定变体：
   ```rust
   pub enum AgentRole {
       Planner,
       Researcher,
       Coder,
       Tester,
       Reviewer,
       Custom(String),  // 新增：支持任意行业角色
   }
   ```
2. 建立 `RoleRegistry`（可通过 config 注入）：
   - 键：角色名称（字符串）
   - 值：`RoleDefinition { name, description, industry, allowed_tools, max_tool_calls, token_budget, timeout_seconds, keywords: Vec<String> }`
3. 在 `config.toml` 增加 `[role_registry]` 扩展点，支持声明式自定义角色：
   ```toml
   [role_registry.communicator]
   description = "沟通者：负责跨部门协调与信息传达"
   industry = "general"
   keywords = ["communicate", "coordinate", "liaison"]
   allowed_tools = ["send_message", "read_file"]
   max_tool_calls = 10
   token_budget = 3000
   timeout_seconds = 60
   ```
4. `TaskRouter::get_role_specs` 查询 `RoleRegistry`，未知角色返回 registry 中的自定义规格而非 panic。
5. `RoleSpecifications` 工厂方法改为查表（从注册表检索），5 个内置角色保留默认规格。

验收点：
- 可通过 `config.toml` 声明"规划者/执行者/审核者/研究者/沟通者/决策者/支持者"等跨行业角色并实际路由。
- `AgentRole::Custom("communicator")` 可参与 `HandoffContract`、`RoleOutput`、`RoutingDecision`。
- 现有 5 个硬编码角色行为不变（向后兼容）。

---

### Step 2：行业自定义角色关键词映射（P0）

**来源：FUTURE3.MD M5 补充 + src/acp/helpers/policy.rs `role_keywords_for` 硬编码**

当前问题：
- `role_keywords_for(role)` 用 `match` 硬编码，`_ => vec![]`（未知角色返回空，导致排名惩罚 -0.12）。
- 无法从配置或注册表注入自定义关键词。

改造内容：
1. `role_keywords_for` 改为先查 `RoleRegistry`（S1 建立），若存在则返回 registry 中的 `keywords`，否则回退硬编码。
2. `RoleRegistry` 持有全局单例（`OnceLock<RoleRegistry>`），在 `AppConfig` 加载时初始化。
3. `rank_execution_agents` 中对 `Custom` 角色不再无条件给 `-0.12`，而是按 registry 关键词计算匹配分。
4. 在 `governance.status` 中暴露 `role_registry_custom_count`（已注册自定义角色数量）。

验收点：
- 自定义角色 `communicator` 配置关键词 `["communicate","liaison"]` 后，agent 名称含这些词时匹配分 ≥ 0.35，而非 -0.12。
- `governance.status.role_registry_custom_count` 准确反映已注册自定义角色数量。

---

### Step 3：合规框架工程落地（P1）

**来源：future-last.md §3.2 + BLUE26 S17 多租户审计（仅部分覆盖）**

当前问题：
- 审计日志结构中缺少 `data_classification`（数据分类）和 `compliance_tags`。
- 无 GDPR/HIPAA 数据保留策略接口。
- 无合规检查门禁钩子（compliance gate hook）。

改造内容：
1. 审计日志结构增加字段：
   ```rust
   pub struct AuditEntry {
       // 现有字段...
       pub data_classification: Option<String>, // "public" | "internal" | "confidential" | "restricted"
       pub compliance_tags: Vec<String>,         // ["gdpr", "hipaa", "pii"] 等
       pub retention_policy: Option<String>,     // "7d" | "90d" | "1y" | "forever"
   }
   ```
2. 在 `config.toml` 增加 `[compliance]` 节：
   ```toml
   [compliance]
   enabled = false
   standards = ["gdpr"]          # 启用的合规标准列表
   default_data_classification = "internal"
   audit_retention_days = 90
   pii_fields = ["email", "phone", "user_id"]   # 需脱敏字段列表
   ```
3. 在 `governance.status` 增加 `compliance_framework_profile`：`{ enabled, standards, audit_retention_days, pii_field_count }`。
4. 在 `release_readiness` gate 增加 `compliance_framework_gate`（当 `compliance.enabled = true` 时必须通过合规自检）。
5. 合规自检：扫描审计日志中 PII 字段是否已按策略脱敏；high-risk 操作是否均含 `evidence_id`。

验收点：
- `compliance.enabled = true` 时，`governance.status.compliance_framework_profile.enabled = true`。
- 审计日志条目含 `data_classification` 和 `compliance_tags`。
- `release_readiness` gate 中 `compliance_framework_gate.ready` 可正确反映合规状态。

---

### Step 4：自我推理守卫（P1）

**来源：FUTURE.MD §3.3 Skill 4 — 未在任何 BLUE 中实现**

当前问题：
- Agent 输出中无"假设声明"（assumption statements）。
- 无证据指针验证逻辑。
- Agent 可在低置信度下输出结论而不触发再检验。

改造内容：
1. `AgentTaskResult` 增加可选字段：
   ```rust
   pub struct AgentTaskResult {
       // 现有字段...
       pub assumptions: Option<Vec<String>>,          // 本轮假设列表
       pub evidence_refs: Option<Vec<String>>,        // 每条假设对应的证据引用
       pub weak_evidence_flags: Option<Vec<String>>,  // 低证据支撑的假设标记
       pub reexamine_triggered: bool,                  // 是否触发了再检验
   }
   ```
2. `SelfRationalizationGuard`（新增 `src/governance/rationalization.rs`）：
   - 输入：`AgentTaskResult`，检查 `confidence < threshold`（默认 0.6）且 `evidence_refs` 为空时置 `weak_evidence_flags`。
   - 当 `weak_evidence_flags` 非空时，在 audit log 中标记 `requires_reexamination = true`。
   - 在 `full_auto` 模式下，`weak_evidence_flags` 非空时阻断输出并触发重问（最多 1 次重问，受 token 预算控制）。
3. 在 `governance.status` 增加 `self_rationalization_guard_profile`：`{ enabled, reexamine_triggered_count, weak_evidence_blocked_count }`。

验收点：
- `confidence < 0.6` 且无 evidence_refs 时，`weak_evidence_flags` 非空，audit log 含 `requires_reexamination = true`。
- `full_auto` 模式下触发再检验不超过 1 次，token 预算受控。
- `governance.status.self_rationalization_guard_profile.enabled = true`。

---

### Step 5：启动仓库上下文预加载（P1）

**来源：FUTURE.MD §3.3 Skill 7 — 未在任何 BLUE 中实现**

当前问题：
- Agent 任务开始时无项目级背景信息预加载（README、构建命令、约定规范）。
- 每轮任务均需消费 token 重建项目上下文，成本浪费。

改造内容：
1. 新增 `StartupContextLoader`（`src/orchestration/startup_context.rs`）：
   - 异步加载并缓存：README（取前 2000 chars）、`Cargo.toml` / `package.json` 构建命令、最近 5 条 commit message、`.editorconfig` / 代码风格规则。
   - 结果缓存为 `StartupContext { loaded_at, readme_excerpt, build_commands, recent_commits, style_rules }`。
   - 使用 `OnceLock<StartupContext>` 保证单进程内只加载一次。
2. `AppConfig` 新增 `[startup_context]` 节：
   ```toml
   [startup_context]
   enabled = false
   readme_max_chars = 2000
   recent_commits = 5
   ```
3. 将 `startup_context` 摘要注入 `AgentTaskEnvelope.evidence`（作为可选字段追加）。
4. 在 `governance.status` 增加 `startup_context_profile`：`{ enabled, loaded, readme_chars, commit_count }`。

验收点：
- `startup_context.enabled = true` 时，进程启动后 `StartupContext` 异步加载完成，不阻塞主请求链。
- `governance.status.startup_context_profile.loaded = true`。
- `AgentTaskEnvelope.evidence` 含 startup_context 摘要（可配置关闭）。

---

### Step 6：Prompt 架构 8 层分层优化（P1）

**来源：FUTURE.MD §3.3 Skill 8 / §5B.8 — 未在任何 BLUE 中实现**

当前问题：
- 系统 prompt 与任务 prompt 无明确分层，静态内容逐轮重复发送，token 浪费。
- 无 prompt 层 hash 缓存机制。

改造内容：
1. 定义 `PromptLayer` 枚举及 `LayeredPromptBuilder`（`src/orchestration/prompt_layers.rs`）：
   ```
   Layer 0: system.role      — 静态角色指令（按 AgentRole 不同）
   Layer 1: system.mode      — 静态模式规则（ask/edit/agent/full_auto）
   Layer 2: system.phase     — 静态阶段指引（planning/coding/review/delivery）
   Layer 3: system.conventions — 静态项目约定与风格规则
   Layer 4: task.objective   — 动态任务目标与验收标准
   Layer 5: task.constraints — 动态预算、范围、工具限制
   Layer 6: task.evidence    — 动态证据指针与已有产物引用
   Layer 7: turn.context     — 当前轮对话状态与最新决策
   ```
2. 静态层（L0-L3）计算 SHA-256 hash，相同 hash 时不重新序列化（复用 token count 估算缓存）。
3. `AgentTaskEnvelope` 携带已构建的 `LayeredPrompt`，各 agent vendor 在 `build_messages` 时从 `LayeredPromptBuilder` 组装。
4. 在 `governance.status` 增加 `prompt_layer_profile`：`{ enabled, static_layers_cached, dynamic_layers_built, estimated_token_savings }`。

验收点：
- 同一 agent role + mode + phase 下，连续两次请求静态层 hash 命中，不重新计算。
- `prompt_layer_profile.static_layers_cached > 0`。
- 各层 token 估算可单独计量（L0-L3 静态层 vs L4-L7 动态层）。

---

### Step 7：Token 成本分层触发架构（P0）

**来源：FUTURE.MD §5B — 未在任何 BLUE 中实现**

当前问题：
- 所有请求均直接进入 model invocation，无按复杂度升级的分层过滤。
- 无 L0（快速拒绝）→ L1（缓存复用）→ L2（廉价分类）→ L3（上下文压缩）→ L4（主生成）→ L5（高风险验证）的明确门控链路。

改造内容：
1. 定义 `RequestLayer` 枚举（L0-L5）和 `LayerGateDecision { pass_through, block_reason, escalate_to }`。
2. 在主请求链 `handle_task_execute` 前增加分层评估器：
   - **L0（快速拒绝/路由）**：schema 校验、消息数/字符预算超限 → 零 token 返回。
   - **L1（缓存复用）**：normalized hash 命中 → 零 token 返回。
   - **L2（廉价分类）**：启发式规则估计复杂度（low/medium/high）、所需模式（ask/edit/agent/full_auto）、工具必要性；仅在启发式不确定时调用低成本分类器。
   - **L3（上下文压缩）**：history 截断、summary 压缩、vector 检索 top-k 动态收窄。
   - **L4（主生成）**：按 L2 决策选择模型档位，默认低档位，置信度失败后升级。
   - **L5（验证升级）**：仅高风险输出 / write 操作 / 低置信 / full_auto 触发双审查。
3. 门控条件（Gate A-D）：
   - Gate A（L1→L2）：无有效缓存命中
   - Gate B（L2→L3）：复杂度 ≥ medium OR 置信度 < threshold
   - Gate C（L3→L4）：压缩与检索后请求仍未解决
   - Gate D（L4→L5）：输出风险高 OR 验证失败 OR mode = full_auto
4. Prometheus 计数器：`acp_gate_a_pass_total`、`acp_gate_b_block_total`、`acp_layer5_invocations_total` 等。
5. `governance.status` 增加 `layered_token_trigger_profile`：`{ enabled, l0_reject_count, l1_cache_hit_count, l5_invocation_count, avg_escalation_level }`。

验收点：
- 缓存命中请求不触发 L4/L5，`l1_cache_hit_count` 递增。
- `full_auto` 请求必须经过 Gate D，`l5_invocation_count` 递增。
- Prometheus 门控计数器可观测。

---

### Step 8：双级任务调度器（P1）

**来源：FUTURE.MD §5C.2 — 仅存在概念，无形式化双级实现**

当前问题：
- 无 Level-1 Task Scheduler（用户任务 + workflow 调度、租户公平性、全局并发上限）与 Level-2 Agent Worker Scheduler（角色 worker 在任务内调度、per-role 限制、fan-out/join）的形式分离。

改造内容：
1. `TaskScheduler`（Level 1，`src/orchestration/scheduler.rs`）：
   - 队列：每租户/会话独立队列，全局并发上限 `global_max_concurrent_tasks`。
   - 公平性：round-robin 跨租户调度，防止单一租户独占。
   - 接口：`submit(task) -> task_id`、`cancel(task_id)`、`status(task_id)`。
2. `AgentWorkerScheduler`（Level 2，`src/orchestration/worker_scheduler.rs`）：
   - 角色 worker 池：每个 `AgentRole` 独立 pool，`max_workers_per_role`。
   - 接口：`assign(role, task_fragment) -> worker_handle`、`join_all(handles)`。
   - fan-out：同一任务可同时分叉给多个不同角色 worker（受 `max_workers_per_task` 限制）。
3. 在 `AppConfig` 增加 `[scheduler]` 节：
   ```toml
   [scheduler]
   global_max_concurrent_tasks = 32
   max_workers_per_task = 4
   max_workers_per_role = 8
   ```
4. `governance.status` 增加 `dual_level_scheduler_profile`：`{ enabled, l1_queue_depth, l2_active_workers, l2_fan_out_count }`。

验收点：
- 超过 `global_max_concurrent_tasks` 的任务被排队，不被直接丢弃。
- Level-2 fan-out 后 join 全部 worker handle，汇总结果正确。
- `dual_level_scheduler_profile.enabled = true`。

---

### Step 9：带抗饥饿的优先级队列调度策略（P1）

**来源：FUTURE.MD §5C.8 — 未在任何 BLUE 中实现**

当前问题：
- 队列调度无优先级维度，高频小任务可能永久阻塞大任务。

改造内容：
1. 在 Level-1 `TaskScheduler`（S8）中引入多维优先级队列：
   - 优先级维度：`user_urgency`（用户传入）、`task_risk`（评估值）、`estimated_token_cost`（低成本高优）、`deadline_proximity`（截止时间近优）。
   - 综合得分：`priority_score = w1 * urgency + w2 * (1/cost) + w3 * deadline_factor + aging_bonus`。
2. 抗饥饿策略：等待超过 `starvation_aging_threshold_seconds`（默认 30s）的任务 `aging_bonus` 随等待时间线性增加，防止长任务永久被抢占。
3. 在 `config.toml [scheduler]` 增加：
   ```toml
   starvation_aging_threshold_seconds = 30
   priority_weights = { urgency = 0.4, cost = 0.2, deadline = 0.3, aging = 0.1 }
   ```
4. `governance.status` 增加 `priority_queue_profile`：`{ aging_threshold_s, max_wait_time_s, starvation_events_prevented }`。

验收点：
- 连续提交 10 个小任务 + 1 个大任务时，大任务在 `starvation_aging_threshold_seconds` 内必然获得调度机会。
- `starvation_events_prevented` 计数器可观测。

---

### Step 10：分叉子代理进程隔离硬化（P0）

**来源：FUTURE.MD §5C.12 — 当前 BLUE26 S13 仅有逻辑隔离，无进程级硬化**

当前问题：
- 并行分叉的子代理无 per-child token / wall-clock / RPC 配额独立限制。
- 子代理 panic / 超时不能防止污染 parent 状态。
- 无 zombie child 防护机制。

改造内容：
1. `ForkIsolation`（`src/orchestration/fork_isolation.rs`）：
   - 每个分叉子代理分配独立 `ChildBudget { max_tokens, max_wall_clock_ms, max_rpc_calls }`。
   - 子代理超时时发送 kill signal（通过 tokio task cancel token），不影响 sibling。
   - 子代理 output 在 merge 前经 schema 验证；无效输出标记 `{ status: "failed", reason: "schema_violation" }` 而非 silent drop。
2. Zombie 防护：所有 fork handle 必须通过 `join_all` 收割；超时未完成的 handle 强制 abort + 计入 `zombie_reaped_count`。
3. `ForkIsolation` 为每个 child 生成唯一 `child_id`，写入 trace，含 `fork_point`、`join_point`、`child_token_usage`、`child_wall_clock_ms`。
4. `governance.status` 增加 `fork_isolation_profile`：`{ enabled, zombie_reaped_count, schema_violation_rejected_count, avg_child_token_usage }`。

验收点：
- 子代理超时时 parent 继续执行，不 panic，`zombie_reaped_count` 递增。
- schema 无效输出被标记 `failed`，不被 merge，`schema_violation_rejected_count` 递增。
- trace 中 `child_id`、`fork_point`、`join_point` 字段存在。

---

### Step 11：能力图谱完整闭环（P1）

**来源：FUTURE4.MD M3 — BLUE34 S11 仅建立 capability_discovery_registry_baseline（注册表基线），图谱语义未实现**

当前问题：
- `capability_discovery_registry` 仅有能力名称列表，无节点依赖、风险等级、可替代路径。

改造内容：
1. `CapabilityGraph`（`src/intelligence/capability_graph.rs`）：
   - 节点：`CapabilityNode { id, name, version, risk_level: RiskLevel(Low/Medium/High/Critical), status: CapabilityStatus(Active/Deprecated/Experimental) }`。
   - 边：`CapabilityEdge { from, to, edge_type: Requires | Replaces | Enhances }`。
   - 支持：拓扑遍历（依赖顺序）、可替代路径查询（`find_alternatives(id)`）、风险传播计算（依赖方风险 ≥ 被依赖方风险时告警）。
2. `capability_graph` 在 `AcpServer` 启动时从 `config.toml [capabilities]` 加载节点与边定义。
3. `governance.status` 增加 `capability_graph_profile`：`{ node_count, edge_count, high_risk_node_count, deprecated_node_count }`。
4. `release_readiness` 增加 `capability_graph_gate`：有 `Critical` 风险节点处于 `Active` 状态时 gate 失败。

验收点：
- `capability_graph_profile.node_count > 0`。
- `find_alternatives` 对已知可替代路径返回正确列表。
- Critical 风险 Active 节点导致 `capability_graph_gate.ready = false`。

---

### Step 12：来源可追溯账本（P1）

**来源：FUTURE4.MD M12 — 未在任何 BLUE 中实现**

当前问题：
- 外部方案来源、模型版本、训练数据引用、接入变更无统一可查的审计账本。

改造内容：
1. `ProvenanceLedger`（`src/observability/provenance.rs`）：
   - 条目：`ProvenanceEntry { id: Uuid, timestamp, entry_type: ProvenanceType(ExternalSource|ModelVersion|TrainingDataRef|IntegrationChange), source_uri, version, sha256_hash: Option<String>, actor, evidence_id: Option<String>, description }`。
   - 持久化：写入 SQLite（local）或 PostgreSQL（server），表名 `provenance_ledger`。
   - 查询：`query_by_type`、`query_by_actor`、`query_by_evidence_id`（支持从证据 ID 反查来源链）。
2. 所有 skill 注册、capability 接入、外部 provider 变更时自动写入 `ProvenanceLedger`。
3. `governance.status` 增加 `provenance_ledger_profile`：`{ enabled, entry_count, last_entry_ts }`。
4. `release_readiness` 增加 `provenance_ledger_gate`：`enabled = true` 时 `entry_count > 0` 才通过。

验收点：
- skill 注册时自动生成 `ProvenanceEntry { entry_type: IntegrationChange }`。
- `query_by_evidence_id` 可追溯到对应来源条目。
- `provenance_ledger_profile.enabled = true`。

---

### Step 13：节点信誉系统（P1）

**来源：FUTURE5.MD M6 — 未在任何 BLUE 中实现**

当前问题：
- agent 排名仅依赖历史顺序 + 角色关键词匹配 + rotation，无基于实际执行质量的信誉评分。

改造内容：
1. `NodeReputation`（`src/intelligence/reputation.rs`）：
   - 信誉维度：`success_rate`（任务成功率）、`stability_score`（输出稳定性，方差倒数）、`cost_efficiency`（token/成功任务）、`review_pass_rate`（通过审查比例）。
   - 综合信誉分：`reputation = 0.4*success + 0.2*stability + 0.2*(1/cost_norm) + 0.2*review_pass`，范围 [0,1]。
   - 按 `agent_name` 存储，定期（每 `reputation_update_interval_cycles` 次请求）批量更新。
2. `rank_execution_agents`（`src/acp/helpers/policy.rs`）中将 `reputation_score * 0.25` 纳入最终得分计算（原 `history_order_score` 权重相应下调）。
3. `config.toml [reputation]` 节：
   ```toml
   [reputation]
   enabled = false
   update_interval_cycles = 50
   min_samples_required = 5    # 样本不足时不使用信誉分
   ```
4. `governance.status` 增加 `node_reputation_profile`：`{ enabled, tracked_agent_count, top_agent, bottom_agent }`。

验收点：
- 执行 ≥ 5 次任务后，信誉分影响 `rank_execution_agents` 排名。
- `node_reputation_profile.tracked_agent_count > 0`。
- 信誉分最高 agent 排名优先于纯顺序排名。

---

### Step 14：云原生部署基线（P2）

**来源：future-last.md §2 / §5 — 未在任何 BLUE 中实现**

当前问题：
- 无 Kubernetes 部署清单，无 mTLS 配置，无容器健康检查端点标准化。

改造内容：
1. 新增 `deploy/k8s/` 目录，包含：
   - `deployment.yaml`：go-on Deployment，含 resource requests/limits、livenessProbe/readinessProbe（对接 `/health` 端点）。
   - `service.yaml`：ClusterIP + NodePort。
   - `configmap.yaml`：挂载 `config.toml`。
   - `secret.yaml`（模板）：API key 环境变量注入示例。
2. 在 `src/acp/server.rs` 中，`/health` HTTP 端点返回标准 JSON：
   ```json
   { "status": "ok", "uptime_seconds": 123, "version": "0.x.y" }
   ```
3. 在 `config.toml [runtime]` 增加 `mtls_enabled = false` 和 `tls_cert_path / tls_key_path / ca_cert_path` 配置项（不强制，但文档化 mTLS 启用路径）。
4. `governance.status` 增加 `cloud_native_profile`：`{ k8s_manifests_present, health_endpoint_ready, mtls_enabled }`。

验收点：
- `deploy/k8s/deployment.yaml` 存在且 `kubectl apply` 可执行（dry-run 通过）。
- `/health` 端点返回 `{ "status": "ok" }`（不强依赖 k8s 环境，本地也可测）。
- `cloud_native_profile.health_endpoint_ready = true`。

---

### Step 15：开发者 SDK 基线（P2）

**来源：future-last.md §7.1 — 未在任何 BLUE 中实现**

当前问题：
- 无官方 Rust client library 供外部项目依赖。
- 无 Python binding 基线。

改造内容：
1. 新增 `sdk/rust/` crate（`sdk/rust/Cargo.toml`）：
   - 提供 `GoOnClient { endpoint, api_key }` 及方法：`initialize()`, `task_execute(task)`, `governance_status()`, `shutdown()`。
   - 底层通过 JSON-RPC over HTTP 与 go-on server 通信。
   - 发布为独立 crate，不依赖 go-on 内部 `src/` 模块。
2. 新增 `sdk/python/` 目录，含 `go_on_client.py`：
   - 实现相同方法集（Python 3.9+）。
   - 含 `pyproject.toml`（setuptools）。
3. 在 `sdk/README.md` 提供最小使用示例。
4. `governance.status` 增加 `developer_sdk_profile`：`{ rust_sdk_present, python_sdk_present, sdk_version }`。

验收点：
- `sdk/rust/` crate 可独立 `cargo build`。
- `sdk/python/go_on_client.py` 可在有 server 运行时执行 `initialize()` 并返回正确结果。
- `developer_sdk_profile.rust_sdk_present = true`。

---

### Step 16：三工作流并存体系（dev / general / custom）（P0）

**通用 phase 标准集设计**

推荐三套内置工作流，三者可在同一进程中通过配置切换共存，不互相排斥：

**① 开发工作流（Dev，现有默认）：**
`planning → coding → review → delivery`

**② 通用工作流（General，跨行业推荐）：**
`gathering → thinking → executing → validating → closing`

| Phase | 含义 | 典型角色 |
|-------|------|----------|
| gathering | 信息收集、需求澄清、背景调研 | researcher, communicator |
| thinking | 分析论断、方案设计、决策制定 | planner, decision_maker |
| executing | 执行实施、输出产物、行动落地 | executor, supporter |
| validating | 验证检核、评估质量、风险扫描 | reviewer, auditor |
| closing | 收尾交付、总结复盘、归档留存 | communicator, planner |

**③ 自定义工作流（Custom）：**
完全由 `config.toml [phases.*]` 声明，无任何内置约束；phase 列表、`default_phase`、每个 phase 的 agents/options 均由用户定义。

**④ 自适应（Auto，推荐默认）：**
`workflow_type = "auto"` —— 系统按以下**固定优先级链**自动选择工作流类型，无需用户手动切换：

| 优先级 | 判断信号 | 推断结果 | 说明 |
|--------|---------|---------|------|
| P1（最高）| 请求携带的 `AgentRole` 解析为 `Custom(*)` 且 `industry != "dev"` | General | 角色明确非开发领域 |
| P2 | 请求携带的 `AgentRole` 为内置 dev 角色（Coder/Tester/Reviewer） | Dev | 角色明确是开发领域 |
| P3 | StartupContext（S5）检测到仓库含代码特征文件（`Cargo.toml`、`package.json`、`go.mod`、`pom.xml`、`*.py`、`*.ts`） | Dev | 项目是代码仓库 |
| P4 | StartupContext 已加载但未检测到任何代码特征文件 | General | 项目是非代码工作目录 |
| P5（兜底）| 以上均无法判断（StartupContext 未启用、角色未指定） | Dev | 保持现有默认行为 |

**推断结论仅影响 workflow_type；config 中的 `[phases.*]` 自定义配置始终优先于预设 phase 选项。**

**当前问题：**
- `FlowConfig` 无 `workflow_type` 字段，无法声明工作流类型。
- `default_phase = "coding"` 在代码与配置多处硬编码，通用工作流应为 `"executing"`，自定义工作流应读取 phases 列表第一项。
- 无 `WorkflowRegistry` 统一管理三套预设，dev/general 的 phase 描述、默认 phase、角色提示完全缺失。
- 三端（GUI / addon / contract）无工作流类型感知，无法向用户呈现当前所处工作流与 phase。

**改造内容：**

1. `WorkflowType` 枚举（`src/core/config.rs`）：
   ```rust
   #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
   #[serde(rename_all = "lowercase")]
   pub enum WorkflowType {
       Auto,    // 自适应推断（推荐，优先级链见下文）
       Dev,     // planning / coding / review / delivery
       General, // gathering / thinking / executing / validating / closing
       Custom,  // 完全由 [phases.*] 配置决定
   }
   impl Default for WorkflowType {
       fn default() -> Self { Self::Auto }  // 新装置默认 Auto，旧 config 无该字段时仍 fallback Dev
   }
   ```

2. `WorkflowDetector`（`src/orchestration/workflow_registry.rs`）——Auto 模式的推断器：
   ```rust
   impl WorkflowDetector {
       /// 按 P1-P5 优先级链推断有效 WorkflowType（仅在 Auto 模式下调用）
       pub fn detect(role: Option<&AgentRole>, startup_ctx: Option<&StartupContext>) -> WorkflowType {
           // P1: Custom 非 dev 角色 → General
           if let Some(AgentRole::Custom(name)) = role {
               if registry_industry(name) != "dev" { return WorkflowType::General; }
           }
           // P2: 内置 dev 角色 → Dev
           if matches!(role, Some(AgentRole::Coder | AgentRole::Tester | AgentRole::Reviewer)) {
               return WorkflowType::Dev;
           }
           // P3/P4: StartupContext 仓库指纹
           if let Some(ctx) = startup_ctx {
               return if ctx.has_code_repo { WorkflowType::Dev } else { WorkflowType::General };
           }
           // P5: 兜底
           WorkflowType::Dev
       }
   }
   ```
   `StartupContext.has_code_repo`：存在 `Cargo.toml` / `package.json` / `go.mod` / `pom.xml` / `.git` + 至少一个 `*.rs|*.ts|*.py|*.go|*.java` 文件时为 `true`。

3. `FlowConfig` 增加字段（向后兼容：旧 config 无该字段时 serde default = `Auto`，运行时 P5 兜底为 Dev）：
   ```rust
   pub struct FlowConfig {
       pub name: String,
       #[serde(default)]
       pub workflow_type: WorkflowType,  // 新增，默认 Auto
       pub phases: Vec<String>,
   }
   ```

4. `WorkflowPreset`（`src/orchestration/workflow_registry.rs`）：
   ```rust
   pub struct WorkflowPreset {
       pub phases: Vec<&'static str>,
       pub default_phase: &'static str,
       pub phase_descriptions: &'static [(&'static str, &'static str)],
   }
   impl WorkflowPreset {
       pub fn dev() -> Self { /* planning/coding/review/delivery, default=coding */ }
       pub fn general() -> Self { /* gathering/thinking/executing/validating/closing, default=executing */ }
   }
   ```

5. `AppConfig::effective_default_phase(role, startup_ctx)` 新增方法：
   - `Auto` → 先调用 `WorkflowDetector::detect()` 得到有效类型，再按有效类型走下面分支
   - `Dev` → `"coding"`（保持现有行为）
   - `General` → `"executing"`
   - `Custom` → `config.default_phase`，若为空则取 `flow.phases[0]`

6. `config.toml [flow]` 增加 `workflow_type` 字段：
   ```toml
   [flow]
   name = "Autopilot Adaptive"
   workflow_type = "auto"   # auto（推荐）| dev | general | custom
   phases = ["planning", "coding", "review", "delivery"]
   ```
   `auto` 模式下 `phases` 字段作为 Custom 覆盖保留，若非空则覆盖预设 phase 列表（Advanced 用法）。
   `General` / `Dev` 内置预设不需要在 config.toml 中重复声明 `[phases.*]`（可覆盖但非必须）；`Custom` 工作流必须在 `[phases.*]` 中完整声明。

7. `PhaseRouter`（`src/orchestration/flow.rs`）更新路由逻辑：
   - `Auto`：每个请求入口处调用 `WorkflowDetector::detect()`，结果缓存在请求上下文中（同一任务内不重复推断）。
   - `Dev` / `General`：从 `WorkflowPreset` 读取 phase 列表，与 config 中自定义 options 合并。
   - `Custom`：完全从 `AppConfig.phases` 读取，不使用任何预设。
   - Fallback：请求的 phase 不在当前 phase 列表中时，fallback 到 `effective_default_phase()`，而非 panic 或返回空。

8. `governance.status` 增加 `workflow_profile`：
   ```json
   {
     "configured_workflow_type": "auto",
     "effective_workflow_type": "dev",
     "detection_signal": "code_repo_fingerprint",
     "phase_count": 4,
     "default_phase": "coding",
     "available_workflow_types": ["auto", "dev", "general", "custom"]
   }
   ```
   `detection_signal` 记录推断依据（`role_industry` / `role_dev_builtin` / `code_repo_fingerprint` / `no_code_fingerprint` / `fallback`），便于审计和调试。

9. 三端支持：
   - `contracts/editor-capability-matrix.json` 增加 `configured_workflow_type`、`effective_workflow_type`、`detection_signal` 字段。
   - GUI：工作流类型选择下拉（auto / dev / general / custom）；auto 时额外显示推断依据 badge（如 "auto › dev [code repo]"）；当前 phase 进度指示器高亮活跃 phase。
   - vscode-addon：状态栏显示 `[auto›dev|general|custom] › phase_name`，点击可覆盖为手动固定值。

**验收点：**
- `workflow_type = "dev"` 时行为与现有完全一致（向后兼容，0 regression）。
- `workflow_type = "auto"` + 仓库含 `Cargo.toml` → `effective_workflow_type = "dev"`，`detection_signal = "code_repo_fingerprint"`。
- `workflow_type = "auto"` + role 为 `Custom("communicator")` 且 `industry = "general"` → `effective_workflow_type = "general"`，`detection_signal = "role_industry"`。
- `workflow_type = "auto"` + StartupContext 未启用 → `effective_workflow_type = "dev"`，`detection_signal = "fallback"`。
- `workflow_type = "custom"` 时 `default_phase` 和 phase 列表完全由 config 决定，无内置约束。
- 切换 `workflow_type` 仅需修改 config.toml，进程重启后生效，无需代码改动。
- `governance.status.workflow_profile.detection_signal` 可审计，三端 contract smoke 断言通过。

---

### Step 17：三端验收与发布门禁（P0）

1. backend 主链：
   - `runtime_pack.rs`：新增 S1-S16 对应 `*_ready` 与 `*_profile`（共 16 项）。
   - `ops_pack.rs`：新增 S1-S16 对应 `*_gate`，含 gates/recommendations/summary/detail。
2. 三端 contract + smoke：
   - `contracts/editor-capability-matrix.json` 增加 `blue35S1` 至 `blue35S16` 共 16 个标志。
   - `vscode-addon/scripts/contract-smoke.js` 新增 16 条 assert。
   - `GUI/scripts/contract-smoke.mjs` 新增 16 条 assert。
3. 集成测试：
   - `tests/acp_runtime_rpc_integration.rs` 新增 governance/readiness/gate 断言覆盖 S1-S16。
   - 新增 `blue35_release_closure` gate 断言。
4. 编译门禁：
   - `cargo check --all-features`：0 warning，0 error。
   - `node vscode-addon/scripts/contract-smoke.js`：EXIT 0。
   - `node GUI/scripts/contract-smoke.mjs`：EXIT 0。
   - `cargo test --test acp_runtime_rpc_integration`：EXIT 0。

验收点：
- 所有门禁全部通过，0 warning，0 regression。

---

### Step 18：BLUE35 发布收口（P0）

1. 真实执行 gate：
   ```sh
   scripts/run-release-readiness-gate.sh config.production.toml
   ```
2. 落盘产物 `RELEASE_GATE_OUTPUT.txt`（含 BLUE35 结果）。
3. 本文件回写完成率、门禁结果与残余风险（若有）。
4. 超出本轮范围的新发现改造项记入 BLUE36 backlog。

验收点：
- 产物可审计、可复现、可追责。
- BLUE35 完成率 100%（S0-S18 全量）。

---

## 顶级能力增强建议（一次并入，不再拆批）

以下能力与 S1-S16 同步落地：

1. **角色配置热重载**
   - `RoleRegistry` 支持运行时热重载（监听 `config.toml` 变更），无需重启即可新增/修改角色。

2. **Token 成本归因看板**
   - 为每个 Gate 层（L0-L5）及每个 AgentRole 分别输出 Prometheus counter，便于按角色/层分析成本。

3. **信誉分可解释输出**
   - `NodeReputation` 在 audit log 中附带信誉维度详情（不仅返回综合分），便于诊断信誉分变化原因。

4. **SDK 版本契约测试**
   - `sdk/rust/` 和 `sdk/python/` 各自包含契约 smoke test，与 `contracts/editor-capability-matrix.json` 对齐。

5. **能力图谱可视化导出**
   - `CapabilityGraph` 支持 `export_dot()` 输出 Graphviz `.dot` 格式，便于工程师可视化依赖关系。

6. **合规标准可扩展**
   - `[compliance].standards` 支持自定义合规规则文件（`rules/*.json`），不硬编码 GDPR/HIPAA 逻辑。

7. **ProvenanceLedger 证据链签名**
   - 每条 `ProvenanceEntry` 生成 HMAC-SHA256 摘要（使用配置中的 `signing_key_env`），防止账本条目被篡改。

---

## 一次到顶硬验收标准（DoD）

1. `AgentRole::Custom(String)` 变体存在，向后兼容已有 5 个固定变体。
2. `RoleRegistry` 可从 `config.toml [role_registry]` 加载自定义角色并参与路由。
3. `role_keywords_for` 查询 `RoleRegistry`，自定义角色关键词正确影响 `rank_execution_agents` 得分。
4. `compliance_framework_profile` 在 `governance.status` 中存在，`compliance.enabled = true` 时合规自检通过。
5. `self_rationalization_guard_profile` 在 `governance.status` 中存在，低置信度输出触发 `weak_evidence_flags`。
6. `startup_context_profile.loaded = true`（当 `startup_context.enabled = true`），不阻塞主请求。
7. `prompt_layer_profile.static_layers_cached > 0`，静态层 hash 命中可观测。
8. `layered_token_trigger_profile` 存在，L1 缓存命中和 L5 调用均有计数器。
9. `dual_level_scheduler_profile.enabled = true`，Level-1 任务队列和 Level-2 worker pool 均活跃。
10. `priority_queue_profile.starvation_events_prevented` 计数器可观测，抗饥饿机制生效。
11. `fork_isolation_profile.zombie_reaped_count` 可观测，子代理超时不污染 parent 状态。
12. `capability_graph_profile.node_count > 0`，Critical 风险节点阻断 `capability_graph_gate`。
13. `provenance_ledger_profile.enabled = true`，skill 注册时自动写入账本。
14. `node_reputation_profile.tracked_agent_count > 0`，信誉分影响 agent 排名（≥5 次样本后）。
15. `cloud_native_profile.health_endpoint_ready = true`，k8s manifests 存在且 dry-run 通过。
16. `developer_sdk_profile.rust_sdk_present = true`，Rust SDK crate 可独立构建。
17. `workflow_profile.active_workflow_type` 准确反映当前工作流；`workflow_type = "dev"` 时 0 regression，`general` / `custom` 均可正确路由。
18. 三端 contract smoke 全绿（16 个 blue35S* 标志全部命中）。
19. `cargo check --all-features`：0 warning，0 error。
20. `cargo test --test acp_runtime_rpc_integration`：0 regression。
21. Release Gate 真实执行并落盘产物，BLUE35 完成率 = 100%。

---

## 风险与止损

1. **通用角色体系破坏现有枚举序列化**
   - 止损：`AgentRole::Custom(String)` 序列化为 `{ "Custom": "name" }`，JSON 反序列化时已知角色优先匹配固定变体，确保向后兼容。

2. **RoleRegistry 热重载引入并发问题**
   - 止损：使用 `Arc<RwLock<RoleRegistry>>`，热重载路径加写锁，读路径加读锁，不允许无锁访问。

3. **分层触发架构增加请求延迟**
   - 止损：L0-L2 必须为纯内存操作（无 I/O），总延迟增量 < 1ms；L2 启发式规则优先，分类器调用为最后手段。

4. **Prompt 分层引入 token 估算误差**
   - 止损：token 估算仅用于层级决策（non-critical path），实际 token 消耗仍以 provider 返回值为准；估算误差 > 20% 时记录 warning。

5. **双级调度器引入死锁风险**
   - 止损：Level-2 worker 获取锁顺序固定（按 `AgentRole` 枚举顺序）；所有 join 设置超时（`max_join_timeout_seconds`），超时后强制 abort。

6. **子代理隔离 kill signal 失效（tokio task cancel）**
   - 止损：Tokio cancel token 为协作式取消，在 await point 才生效；ToolRuntime 所有 `await` 点必须检查 cancel token；禁止在子代理中使用 `tokio::task::block_in_place` 逃逸 cancel。

7. **能力图谱循环依赖导致无限递归**
   - 止损：图谱加载时执行 DFS 环检测，发现环则拒绝加载并返回错误，不允许有环图谱进入运行时。

8. **ProvenanceLedger 写入频率过高影响主链路延迟**
   - 止损：ProvenanceLedger 写入为异步（通过 `mpsc` channel），主链路不等待写入完成；channel 满时丢弃并计数 `provenance_drop_count`。

9. **信誉系统冷启动惩罚新 agent**
   - 止损：`min_samples_required`（默认 5）内不使用信誉分，改用顺序排名；新 agent 初始信誉分为中值（0.5）而非 0。

10. **SDK 版本与 server 协议漂移**
    - 止损：SDK 含 `protocol_version` 字段，与 server `initialize` 返回版本做兼容性检查；不兼容时返回明确错误，而非静默行为偏差。

11. **云原生 manifests 与实际镜像脱节**
    - 止损：`deploy/k8s/deployment.yaml` 中镜像 tag 使用 `{{ .Values.image.tag }}`（占位符），CI 在构建时替换，禁止硬编码 `latest`。

12. **合规框架误判业务数据为 PII**
    - 止损：`compliance.pii_fields` 为白名单配置，默认为空；误报时运维人员可通过配置排除，不影响主链路。

13. **工作流切换导致 default_phase 推断错误**
    - 止损：`effective_default_phase()` 按优先级查找：1) config 显式设置 > 2) workflow_type 预设 > 3) phases[0]；三者均缺失时返回 `""` 并在启动时打印 WARN，不 panic。

14. **General 工作流下现有 dev-only phase（coding）被错误路由**
    - 止损：`PhaseRouter` fallback 逻辑在 phase 不属于当前工作流时，返回 `effective_default_phase()` 并在 audit log 中记录 `phase_fallback: true`；禁止 silent drop 或 panic。

15. **自定义工作流 phases 列表为空导致无法路由**
    - 止损：`AppConfig` 加载时对 `workflow_type = Custom` 且 `flow.phases` 为空的情况报 `ConfigWarning { severity: Critical }`，`production_strict = true` 时拒绝启动。

---

## 完成率回写（执行后更新）

总步骤: 19 (S0-S18)
已完成: 0
完成率: 0%（待实现）

| 步骤 | 能力键 | 来源 | 状态 |
|------|--------|------|------|
| S0 | scope_freeze | 本文件冻结 | ⏳ 待实现 |
| S1 | universal_cross_industry_roles | FUTURE3.MD M5 补充 | ⏳ 待实现 |
| S2 | industry_role_keyword_mapping | FUTURE3.MD M5 补充 + policy.rs | ⏳ 待实现 |
| S3 | compliance_framework_engineering | future-last.md §3.2 | ⏳ 待实现 |
| S4 | self_rationalization_guard | FUTURE.MD §3.3 Skill 4 | ⏳ 待实现 |
| S5 | startup_repository_context_loading | FUTURE.MD §3.3 Skill 7 | ⏳ 待实现 |
| S6 | prompt_architecture_8layer | FUTURE.MD §3.3 Skill 8 / §5B.8 | ⏳ 待实现 |
| S7 | layered_token_trigger_l0_l5 | FUTURE.MD §5B | ⏳ 待实现 |
| S8 | dual_level_task_worker_scheduler | FUTURE.MD §5C.2 | ⏳ 待实现 |
| S9 | priority_queue_anti_starvation | FUTURE.MD §5C.8 | ⏳ 待实现 |
| S10 | forked_subagent_process_isolation | FUTURE.MD §5C.12 | ⏳ 待实现 |
| S11 | capability_graph_full_closure | FUTURE4.MD M3 | ⏳ 待实现 |
| S12 | provenance_ledger | FUTURE4.MD M12 | ⏳ 待实现 |
| S13 | node_reputation_system | FUTURE5.MD M6 | ⏳ 待实现 |
| S14 | cloud_native_k8s_baseline | future-last.md §2/§5 | ⏳ 待实现 |
| S15 | developer_sdk_baseline | future-last.md §7.1 | ⏳ 待实现 |
| S16 | tri_workflow_coexistence_dev_general_custom | config/config.toml + src/orchestration/ | ⏳ 待实现 |
| S17 | tri_end_contract_and_gate | 三端验收 | ⏳ 待实现 |
| S18 | blue35_release_closure | 发布收口 | ⏳ 待实现 |

回写规则：
- 每完成一个步骤，状态从 `⏳ 待实现` 改为 `✅ 已完成`。
- 全门禁通过后将 `blue35_release_closure` 标记完成并更新完成率。
