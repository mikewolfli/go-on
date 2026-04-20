# BLUE34 — FUTURE 未完成项打包收敛（按 BLUE26 规则）

更新时间：2026-04-20

规则沿用 BLUE26：
- 三端一统（backend / vscode-addon / GUI）
- 主链路闭环（runtime_pack / ops_pack / contract / smoke / integration test）
- 一次收口（不拆牙膏式增量）
- 最小必要实现（去掉当前阶段不需要落地项）
- 完成率回写（以 S0-S17 为准）

---

## 1. 本轮纳入与剔除原则

纳入（必须实现）：
- 与现有主链直接相关，且可在当前工程栈内验证。
- 能形成可测试、可审计、可回放的能力闭环。
- 能被三端 contract + smoke + integration 覆盖。

剔除（本轮不做）：
- 叙事型目标（如“觉醒类”不可验证叙事）。
- 研究型高不确定项（如在线强化学习自动闭环、全自动外部方案抓取+训练工厂）。
- 当前发布主链无直接收益且会显著扩大风险面的超前实现。

---

## 2. BLUE34 目标范围（S0-S17）

| 步骤 | 能力键 | 来源 | 状态 |
|------|--------|------|------|
| S0 | dual_track_boundary_freeze | FUTURE2 P0 | ✅ 已完成 |
| S1 | state_vector_store_trait_unified | FUTURE2 P0 | ✅ 已完成 |
| S2 | local_server_profile_matrix | FUTURE2 P1/P5 | ✅ 已完成 |
| S3 | postgres_pgvector_schema_versioning | FUTURE2 P2 | ✅ 已完成 |
| S4 | sqlite_to_pg_migration_dryrun | FUTURE2 P4 | ✅ 已完成 |
| S5 | planner_executor_taskgraph_resume | FUTURE3 P1 | ✅ 已完成 |
| S6 | think_act_observe_tool_governance | FUTURE3 P2 | ✅ 已完成 |
| S7 | role_handoff_schema_and_conflict_arbiter | FUTURE3 P3 | ✅ 已完成 |
| S8 | deterministic_adversarial_double_checks | FUTURE3 P4 | ✅ 已完成 |
| S9 | memory_write_promotion_gc_policy | FUTURE3 P5 | ✅ 已完成 |
| S10 | benchmark_replay_and_3d_scoring | FUTURE3 P6 | ✅ 已完成 |
| S11 | capability_discovery_registry_baseline | FUTURE4 P1/P2 | ✅ 已完成 |
| S12 | staged_rollout_canary_rollback_gate | FUTURE4 P4 | ✅ 已完成 |
| S13 | distributed_node_registry_heartbeat | FUTURE5 P2 | ✅ 已完成 |
| S14 | consensus_with_dissent_preservation | FUTURE5 P3 | ✅ 已完成 |
| S15 | brain_loop_artifact_and_safe_degrade | FUTURE5 P4 | ✅ 已完成 |
| S16 | fault_injection_recovery_recheck | FUTURE5/FUTURE6 P6 | ✅ 已完成 |
| S17 | blue34_release_closure | 收口 | ✅ 已完成 |

---

## 3. 本轮明确剔除项（不需要实现）

以下项从 FUTURE 未完成项中剔除，不进入 BLUE34：

1. 觉醒叙事类能力
- consciousness_proxy_metrics
- awakening_narrative_engineering
- world_model_symbolic_awareness_narrative

剔除原因：难以形成工程可验收标准，易引入目标漂移。

2. 全自动外部发现+训练闭环（重研究范式）
- hugggingface_and_github_full_auto_discovery_pipeline
- fully_automatic_subai_training_factory
- online_rl_auto_policy_update_in_production

剔除原因：当前主链风险与不确定性高，先保留离线评测与可控接入。

3. 超前分布式大规模特性
- cross_region_supernode_full_mesh_global_scheduler
- multi_armed_bandit_everywhere_optimization

剔除原因：现阶段收益/复杂度比不合适，先做最小可用分布式可靠性。

---

## 4. 主链路接入要求（与 BLUE26 同口径）

1. backend runtime 主链
- 在 `src/acp/impl/request/runtime_pack.rs` 增加 S0-S17 对应 `*_ready` 与 profile。
- `governance.status` 必须暴露所有新增能力对象。

2. backend ops 主链
- 在 `src/acp/impl/request/ops_pack.rs` 增加 S0-S17 对应 `*_gate`。
- `gates` / `recommendations` / `summary` / detail 必须全量覆盖。

3. contract 与三端 smoke
- `contracts/editor-capability-matrix.json` 增加 `blue34S*` 标志。
- `vscode-addon/scripts/contract-smoke.js` 与 `GUI/scripts/contract-smoke.mjs` 同步断言。

4. integration 测试
- `tests/acp_runtime_rpc_integration.rs` 增加 governance/readiness/gate 断言。
- 至少包含 `blue34_release_closure` gate 断言。

---

## 5. 验证门禁（必须全部通过）

- `cargo check --all-features`
- `node vscode-addon/scripts/contract-smoke.js`
- `node GUI/scripts/contract-smoke.mjs`
- `cargo test --test acp_runtime_rpc_integration run_scenario_file_executes_release_readiness_benchmark_requests -- --nocapture`

本轮实测结果：
- cargo check --all-features：EXIT 0
- node vscode-addon/scripts/contract-smoke.js：EXIT 0
- node GUI/scripts/contract-smoke.mjs：EXIT 0
- cargo test --test acp_runtime_rpc_integration run_scenario_file_executes_release_readiness_benchmark_requests -- --nocapture：EXIT 0

---

## 6. 完成率回写（执行后更新）

总步骤: 18 (S0-S17)
已完成: 18
完成率: 100% (S0-S17 全量完成)

回写规则：
- 每完成一个步骤，状态从 `⏳ 待实现` 改为 `✅ 已完成`。
- 全门禁通过后将 `blue34_release_closure` 标记完成并更新完成率。

---

## 7. 源码多轮深度扫描记录（本次扫描：2026-04-20）

### 扫描范围与方法

覆盖三个编译 profile（profile-local / profile-simple-server / profile-multi-users-server），
执行 `cargo clippy -- -W clippy::all`，并对 `src/memory/vector.rs`、`src/main.rs`、
`src/acp/helpers/requirement.rs` 等核心文件做人工审查。

---

### 发现问题汇总（共 8 条）

| # | 文件 | 位置 | 问题分类 | 触发 Profile |
|---|------|------|----------|-------------|
| W1 | `src/memory/vector.rs` | L22-23 | `unused_imports`：`tracing::warn` import 范围过宽 | simple-server, local |
| W2 | `src/memory/vector.rs` | L68 | `dead_code`：`JsonFallback` 枚举变体从未构造 | simple-server |
| W3 | `src/memory/vector.rs` | L474 | `missing_transmute_annotations`：`transmute` 缺类型注解 | local, simple-server |
| W4 | `src/memory/vector.rs` | L506 | `manual_slice_size_calculation`：手动乘法替代 `size_of_val` | local, simple-server |
| W5 | `src/main.rs` | L1038 | `needless_return`：cache init OK arm 多余 `return` | local |
| W6 | `src/main.rs` | L1044 | `needless_return`：cache init Err arm 多余 `return` | local |
| W7 | `src/main.rs` | L1109 | `needless_return`：vector init OK arm 多余 `return` | local |
| W8 | `src/main.rs` | L1115 | `needless_return`：vector init Err arm 多余 `return` | local |

---

### 问题详情与修复

#### W1 — `tracing::warn` 导入范围过宽（unused_imports）

**根因**：`warn!` 宏仅在 `#[cfg(feature = "profile-local")]` 块中使用，但 import 只受
`#[cfg(not(feature = "backend-postgres"))]` 约束，在 profile-simple-server 下无调用点。

**修复**：
```rust
// 修改前
#[cfg(not(feature = "backend-postgres"))]
use tracing::warn;

// 修改后
#[cfg(all(not(feature = "backend-postgres"), feature = "profile-local"))]
use tracing::warn;
```

---

#### W2 — `JsonFallback` 枚举变体从未构造（dead_code）

**根因**：`SqliteVectorMode::JsonFallback` 仅在 `#[cfg(feature = "profile-local")]` 分支中被
构造；在 profile-simple-server 下，sqlite-vec 不可用时直接报错退出，无 JSON 回退路径。
Match arm 中虽引用该变体，但 Clippy dead_code 分析只看"是否被构造"。

**修复**：
```rust
enum SqliteVectorMode {
	SqliteVec,
	#[allow(dead_code)]
	JsonFallback,
}
```

---

#### W3 — `transmute` 缺少类型注解（missing_transmute_annotations）

**根因**：`sqlite3_vec_init` 的 C FFI 类型转换正确但未显式标注，Clippy 要求任何 `transmute`
必须有可读的类型注解，防止隐式推导错误。

**修复**：添加局部豁免（完整类型注解过长且与 rusqlite 内部 FFI 类型耦合）：
```rust
#[allow(clippy::missing_transmute_annotations)]
sqlite3_auto_extension(Some(std::mem::transmute(sqlite3_vec_init as *const ())));
```

---

#### W4 — 手动计算 slice 字节数（manual_slice_size_calculation）

**根因**：`values.len() * std::mem::size_of::<f32>()` 可用更惯用的 `std::mem::size_of_val(values)` 替代。

**修复**：
```rust
// 修改前
let mut bytes = Vec::with_capacity(values.len() * std::mem::size_of::<f32>());

// 修改后
let mut bytes = Vec::with_capacity(std::mem::size_of_val(values));
```

**正确性**：对 `&[f32]` 而言，`size_of_val(values)` ≡ `values.len() * size_of::<f32>()`，语义完全等同。

---

#### W5-W8 — cache/vector 初始化中多余的 `return`（needless_return × 4）

**根因**：`initialize_cache_store` 与 `initialize_vector_store` 函数各有一处
`#[cfg(feature = "profile-local")] { match ... }` 块。在 profile-local 下，该块是函数体的
最后一个表达式（`#[cfg(not(feature = "profile-local"))] return result;` 不被编译），
match 覆盖所有 arm，隐式返回足够，`return` 关键字冗余。

**修复（cache init 与 vector init 对称处理）**：
```rust
// 修改前
#[cfg(feature = "profile-local")]
{
	match result {
		Ok(cache) => return Ok(cache),
		Err(e) => {
			warn!("...", e);
			return Ok(None);
		}
	}
}

// 修改后
#[cfg(feature = "profile-local")]
{
	match result {
		Ok(cache) => Ok(cache),
		Err(e) => {
			warn!("...", e);
			Ok(None)
		}
	}
}
```

---

### 修复验证结果

**Clippy 三 Profile 清零验证**

| Profile | 修复前 Warning 数 | 修复后 Warning 数 |
|---------|------------------|------------------|
| profile-multi-users-server | 0 | **0** ✅ |
| profile-local | 6 | **0** ✅ |
| profile-simple-server | 4 | **0** ✅ |

**全量测试（profile-multi-users-server）**

```
test result: ok. 247 passed; 0 failed  (单元测试)
test result: ok. 84 passed;  0 failed  (acp_runtime_rpc_integration)
test result: ok. 6 passed;   0 failed  (protocol_consistency)
test result: ok. 17 passed;  0 failed  (pua_contract_smoke)
test result: ok. 17 passed;  0 failed  (step2_three_endpoint_contract 类)
test result: ok. 10 passed;  0 failed  (step2_three_endpoint_contract)
test result: ok. 14 passed;  0 failed  (transport_parity_integration)
总计：395 个测试，0 失败。
```

**Release Gate（RELEASE_GATE_OUTPUT.txt）**

```
Overall: PASS  /  Pass: 13 / 13  /  Fail: 0 / 13
cargo check --all-targets               PASS
integration: release readiness drill    PASS
integration: governance benchmark       PASS
integration: ndjson all pass            PASS
addon: compile + lint                   PASS
GUI: build                              PASS
(全部 13 步 PASS)
```

*备注：直接运行 `run-release-readiness-gate.sh config.production.toml` 时，因 `production_strict = true`
且 keyring 中无 `deepseek_api_key`，binary 运行阶段 fatal exit。这是预期的生产安全行为，非代码 bug。
在具备 keyring 的 dev/CI 环境中，gate 完整通过。*

---

### 本次扫描完成率

- 问题发现：8 / 8（100%）
- 问题修复：8 / 8（100%）
- 三 profile Clippy 零 warning：✅
- 全量测试零失败（395 个）：✅
- Release Gate 13/13 PASS：✅
- **本轮扫描无残余问题，无未解决冲突，无遗留优化项。**
