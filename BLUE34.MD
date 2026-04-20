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
