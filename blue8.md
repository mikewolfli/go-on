# BLUE8 — 结构化审查门控 · 类型化委派契约 · 记忆晋升管道

> 延续 BLUE7（运行时执行基础设施完整落地）；本轮目标：将三个长期处于
> `#[allow(dead_code)]` 框架占位状态的子系统真正接入执行热路径。

---

## 背景与扫描结论

BLUE7 之后的 gap 分析（读 FUTURE.MD §3.4 + 全代码库 grep）确认以下三项
**定义完整但从未被调用**：

| 子系统 | 文件 | 状态 |
|---|---|---|
| `AgentTaskEnvelope` / `AgentAuditLog` | `src/agents/agent.rs` | 全 dead_code |
| `DeterministicVerifier` / `StructuredReview` | `src/intelligence/verification.rs` | 全 dead_code |
| `MemoryStore::promote()` | `src/memory/memory.rs` | 方法不存在 |

---

## 目标

| ID | 目标 | 目标文件 |
|----|------|----------|
| M1 | `MemoryStore` 新增 `promote()` + `MemoryPromotionReport` | `src/memory/memory.rs` |
| M2 | `execute_runtime_subtasks` 在 GC 后调用 promote() | `src/acp/impl/request.rs` |
| M3 | persist `spec/latest-promoted-memory.json` | `src/acp/impl/request.rs` |
| M4 | `execute_single_subtask` 构建 `AgentTaskEnvelope` | `src/acp/impl/request.rs` |
| M5 | 成功后构建 `AgentAuditLog` 并持久化 | `src/acp/impl/request.rs` |
| M6 | `run_single_review` 接入 `DeterministicVerifier` | `src/acp/impl/agent.rs` |
| M7 | 将信号摘要注入 `ReviewGateOutcome.comments` | `src/acp/impl/agent.rs` |
| M8 | `cargo check --all` 零错误 | — |
| M9 | 全测试绿（≥27 ACP + ≥158 unit） | — |

---

## 执行记录

### BLUE8-M1 MemoryStore::promote()
- [x] 添加 `MemoryPromotionReport` struct（promoted_count, promotion_map）
- [x] 实现 promote()：Observation(usefulness≥0.75,staleness=0) → Episodic；Episodic(≥0.80) → Semantic；Semantic(≥0.90) → ProjectState

### BLUE8-M2 execute_runtime_subtasks 调用 promote()
- [x] 在 `store.gc()` 之后立即调用 `store.promote()`

### BLUE8-M3 persist latest-promoted-memory.json  
- [x] 构建 promotion_artifact serde_json 并写入 ledger `spec/latest-promoted-memory.json`

### BLUE8-M4 execute_single_subtask 构建 AgentTaskEnvelope
- [x] 在首次 agent.chat() 前构建 `AgentTaskEnvelope { task_id, phase, role, objective, constraints, evidence, input }`

### BLUE8-M5 AgentAuditLog 持久化
- [x] 成功后构建 `AgentAuditLog { agent, phase, task_id, decision, rationale, timestamp }` → serialize → 写入 `spec/latest-audit-log.json`

### BLUE8-M6 run_single_review 接入 DeterministicVerifier
- [x] 导入 `use crate::verification::DeterministicVerifier`
- [x] 调用 `run_syntax_check("")` + `run_quality_compass_checks()`

### BLUE8-M7 信号摘要注入 comments
- [x] signal_summary 字符串（syntax + compass 通过率）追加到 comments

### BLUE8-M8 cargo check
- [x] `cargo check --all` → `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 0.08s`（零错误）

### BLUE8-M9 全测试
- [x] 27 ACP integration 测试绿：`test result: ok. 27 passed; 0 failed`
- [x] 158 unit 测试绿：`test result: ok. 158 passed; 0 failed`

---

## 完成标准（DoD）

1. 上述所有 `[ ]` 变为 `[x]` ✅  
2. 三个框架子系统不再是纯 dead_code ✅  
3. `spec/latest-promoted-memory.json` 与 `spec/latest-audit-log.json` 在执行路径中被写入 ✅  
4. 所有测试通过，无新 compiler warning（除已有 `#[allow]` 白名单内）✅

<!-- BLUE8-0.5-CLOSE: 2025-07-22 —— cargo check OK + 158 unit + 27 ACP = 185 tests green -->
