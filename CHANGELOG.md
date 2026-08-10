# Changelog

## [Unreleased] - 2026-08-10

### Round 43 — Super-Deep/Broad Scan XI: 统一链路与收敛 (2026-08-10, docs/log/log-20260810-4.md)

#### 链路级去重合并

- **MCP 客户端双实现合并**：`McpStdioClient`/`McpHttpClient` 收敛为 `McpClientCore` + 可插拔 `McpTransport`（stdio/http 共享 initialize/notify/request/list_tools/call_tool）；HTTP 复用进程级共享 client。
- **workflow 工具统一执行链**：`workflow_execute/ask/generate` 从 native MCP 特判移入 `execute_tool_call` 统一链（`is_bridge_special_tool` + match 臂），两臂（adaptive/native MCP）行为一致，含人读 `message` 字段与审计补齐。
- **tool fallback 链合并**：executor 与 registry 两套 fallback 收敛为 `run_fallback_chain_async`（hooks 不重复执行）。
- **任务分类器去重**：`Planner::analyze_task` 委托 `TaskRouter::analyze_task`，删除独立关键词表。
- **SSE 帧/信号监听复用**：MCP SSE 帧改 `write_sse_raw_event`；`extract_header_value` 转发壳删除；`try_consume` 委托 `try_consume_n`。
- **auto_migrate 三处触发收敛**为单一 5 分钟后台任务（请求路径只做单条 promote）。
- **`memory.ingest` 双实现/79 处 profile cfg 分支**：已审计记录（详见日志 §6），hub 手写 HTTP 解析与 cfg 矩阵列为设计债务观察。

#### 行为修复（真实 bug）

- **Delphi 收敛恒 false 修复**：`converged` 改为真实收敛标记（配双向测试）。
- **fault_tolerance 心跳语义修复**：新增 `has_reported`，注册但未上报的节点不再被误判 Offline（消除空闲节点故障 churn）；真实执行信号（成功 heartbeat/失败 fault）接入 `execute_single_subtask`。
- **echo_skill 广告-执行闭环**：注册 `EchoSkillAlias`（`echo_skill`），tools.list 广告名全部确定性可执行。
- **skill_market CLI 崩溃修复**：`restore_installations` 由 `blocking_write`（runtime 内必 panic）改 `.write().await`，`SkillMarketRegistry::new` 改 async，配回归测试。
- **自适应模型选择器 context 学习信号接通**：`record_result_with_context` 接入执行路径（时间/任务类型/延迟分桶）。
- **VoteMode::Legacy 真简单多数**（与 Weighted 行为区分，配测试）。
- **GLOBAL_VOTERS 改为可替换 RwLock**（hub 测试确定性，消除测试顺序/并行污染）。
- **语义缓存实际已接线确认**（put 存在于 fallback 路径；autonomy 主路径不填充列为非阻断观察）。

#### 安全加固

- **自进化循环门禁**：`runtime.evolution_enabled`（默认 false）——EvolutionLoop 不再无条件 AutoApproval 自动改写 `src/**/*.rs`；4 个 config toml + zed-config 显式登记。

#### 死代码/假保留清理

- 删除 `preferred_agent_from_request`、`review_overrides` 死字段；`ADAPTIVE_TEMPLATE` 死常量；`find_template` 死检查（`--setup` 不再误报模板缺失）；`adaptive` 死臂（dispatch 前已解析）；`AcpMethodNames`/unused re-export 审计（multimodal 29 项、acp/hub 数项）；`_fallback_marker` 运行时代码改 `cfg_attr`；决策时自证成功信号删除。
- `deny_unknown_fields` 从远程 skill index 移除（未知字段宽容 + schema_version 校验，远程容错）。

#### 三端/文档/部署

- **GUI 重试状态集对齐**：`is_retryable_status` = 408/429/全 5xx，与三 SDK/契约一致；契约补充 GUI RPC 错误码重试说明。
- **vscode reconnect 测试重写**：测真实 `backoffDelayMs`（1000ms 基/30s 上限，区间断言），删除过时 2000ms/300s 自实现。
- **skills/ 收敛**：删除 11 个符号链接（9 外部技能 + 2 坏链），目录数 = 34 = builtin_skills 数；skills/README 修正。
- **结构性测试迁移**：纯默认值断言迁入 `src/` 内联单元测试（defaults.rs/status.rs），tests/structural 保留真实子系统测试。
- **部署修复**：Dockerfile 创建 `/var/lib/go-on`；compose 卷路径对齐 + multi-users 注入 `GO_ON_PG_CONNECTION_STRING` 激活 PG；deploy.sh 建数据目录；k8s 抓取路径 `/metrics`；ps1 GUI smoke 读取 bug 修复。
- **config 幽灵键清理**：k8s configmap `i18n_enabled` 删除；README/zh-CN 统计对齐（444 模块/1513 lib/134 集成）；cookbook 三语 VS Code 键名修正；DEVELOPMENT_RULES 陈旧路径修正。

#### Verification

```
cargo check 4 profiles + workspace + GUI → zero errors/warnings
cargo clippy --all-targets -D warnings → zero warnings
cargo test --lib → 1513 passed / 0 failed
structural 11/11 · acp_runtime_rpc 27/27 · protocol_consistency 26/26 · transport_parity 8/8 · e2e 18/18 · cli 9/9 · config_validation 11/11 · pua_smoke 6/6 · protocol_parity 5/5 · openai_compat 3/3 · i18n 18/18
vscode tsc --noEmit → zero errors · Rust SDK check → zero warnings
```

### Round 41 — Super-Deep/Broad Scan + Dependency Audit (2026-08-10, docs/log/log-20260810-1.md)

#### Link Closure & Dedup

- **`$/cancel_request` / `session/cancel` 取消语义真实化**：SDK chat_stream 参数顶层 model/temperature/max_tokens 映射进 options；GUI/vscode 串行请求并行化（join!/Promise.all）。
- **Council 投票链接线**：`cast_vote`/`tally_votes` 真实参与路由决策；删除零调用 `record_vote_accuracy`。
- **SelfModelCore 假持久化清理**：删永不落盘 persistence 面，新增 `capability_stats()` 读取 API；`ExecutionRecorder` 孤儿 trait 删除。
- **metacognitive llm_agent 假注入链整体删除**（字段/setter/builder/调用点）。
- **熔断器状态机三份合并**为单一 `transition_breaker`，半开计时统一；hyper_resilience 双记录路径统一。
- **工具分类三份映射合并**为单一数据表（消除同一工具跨列表不一致）。
- **Cold tier 接通读取**：`search_by_session` warm 不足回退冷层；`get_from_cold` 新 API；shard 计数 O(1) 缓存。
- **fault_tolerance 状态可恢复**：生产接 `new_with_restore`；持久化路径 target/ → `.goon/fault_tolerance/`。
- **配置双源合并**：老格式顶层键（entry_auth_enabled 等）回填 `[runtime]`（显式值优先），不再静默丢弃。
- **i18n 启动断点修复**：全新安装也能初始化全局 I18N；startup i18n 与 skill 发现并行化。
- **.goon 路径统一**为 `resolve_goon_root` 单一解析；chat 模式复用 bootstrap SkillRegistry。
- 死代码清理：learning.rs 双遗忘循环、schema 迁移空 API、cache/semantic_cache/pua 零调用 pub fn、onboarding 双解析、i18n watcher 重复等。

#### Dependency Upgrades (audit all 84 direct deps)

- **reqwest 0.12 → 0.13**（feature `rustls-tls`→`rustls`，启用 `form`/`query`）
- **base64 0.22 → 0.23**
- **keyring 3 → 4**（`apple-native`/`sync-secret-service` → `v1` 兼容层）
- **criterion 0.5 → 0.8**（dev-only）
- **egui/eframe 0.34 → 0.36**（App trait `update`→`ui`；panel `show(ctx)`→`show(ui)`；`show_inside` 改名 `show`）
- **comrak 0.52 → 0.54**（GUI，与主 crate 统一）；**quick-xml 0.37 → 0.41**（mermaid-render）
- 约束内 197 项传递依赖更新至最新；zip 8 / merman alpha 链 / serde_yaml 因兼容性保留（记录在案）。

#### Verification

```
cargo check 4 profiles + --workspace → zero warnings
cargo clippy (local/multi-users/GUI) -D warnings → zero warnings
cargo test --lib → 1490 passed / 0 failed
acp_runtime_rpc 27/27 · protocol_consistency 26/26 · transport_parity 18/18 · protocol_parity 5/5 · structural 12/12 · pua_smoke 3/3
Rust SDK 21/21
```

### Round 42 — Super-Deep/Broad Scan + Unified Architecture Optimization (2026-08-10, docs/log/log-20260810-2.md)

#### Real Semantics & Dispatch

- **`$/cancel_request` real cancellation mechanism**: the ACP arm now mirrors the MCP `cancelled_requests` registry — a cancellation notification marks the request id, and in-flight token-collection loops abort with the standard -32800 code instead of merely logging and continuing.
- **`session/cancel` real semantics**: cancelled sessions are marked (`AcpSessionState.cancelled`) and reject new prompts, replacing the previous bare-error no-op.
- **RecoveryAction real dispatch**: `execute_recovery_plan` performs the observable effect of every action (RestartNode / FailoverToBackup / ScaleUp / Rebalance / NotifyOperator) and runs the post-recovery consistency check — automatic recovery either genuinely resolves the fault or reports the residual state.
- **postgres warm tier DSN fix + cold degrade**: under `backend-postgres`, `WarmStore` resolves the connection string from the environment instead of treating a SQLite file path as a PG DSN, and degrades to the cold tier when no connection string is reachable — memories are never silently lost.
- **prompt_layers wired end-to-end**: the `PromptAssembler` output is now consumed by agent selection instead of being assembled and discarded.
- **double forgetting loop unified**: the reinforce/demote loops in continuous learning no longer fight each other (the demotion path was unreachable).
- **council tally participates in routing**: `cast_vote` / `tally_votes` outcomes feed routing decisions instead of being decorative JSON.

#### Unification & Correctness

- **Three validation engines converged** into the single `ConfigValidator` + hard-gate pair.
- **Profile recommendation names real**: setup output names the actual profiles (`local` / `simple-server` / `multi-users-server` / `full`) instead of the non-existent minimal/balanced/full.
- **keyring async wrapper**: blocking keyring I/O (D-Bus / Keychain) moved behind `set_keyring_async` / `get_keyring_cached_async` so it never runs on a tokio worker.
- **Four profiles compile green** (local / simple-server / multi-users-server / full).

## [1.5.1] - 2026-08-07

### Version bump + errors/warnings sweep (2026-08-07, docs/log/log-20260807-1.md)

- All product versions aligned to **1.5.1** (workspace, GUI, VS Code addon, rust/python/typescript SDKs, crates, cookbook, README badges).
- Full-project errors/warnings sweep: `cargo check` (4 profiles + workspace) and `cargo clippy --all-targets -D warnings` zero warnings/errors; GUI `cargo check` zero warnings; `sdk/typescript` and `vscode-addon` `tsc --noEmit` zero errors.

## [1.5.0] - 2026-08-07

### Round 40 — Super-Deep/Broad Scan Rounds 1-6 (2026-08-07, docs/log/log-20260807-1.md)

#### SDK / Three-Client Contract

- **`sdk/nodejs` removed** (confirmed duplicate of `sdk/typescript`): `vscode-addon` keeps its hard dependency on `go-on-sdk-typescript`; TS SDK gained `configReload` (aligns with backend `config.reload`) and `health()` return type corrected to the real `ServerStatus` payload; `.gitignore`/`contracts/cross-client-sync.md`/cookbook index updated to drop Node.js references.

#### Anti-Fake-Metric Cleanup (principles #13/18)

- **`AutonomyLoopReport` constant-zero fields removed** (`planner_guidance_used`/`trace_alignment_coverage`/`corrective_actions_applied_total`/`corrective_action_effectiveness_ratio`): the ACP/CLI autonomy-loop path never applies corrective actions, so exposing constant 0/1.0 was a fake metric. `contract_snapshot()` now exposes the unified execution-evidence contract (rounds/tools/phase/duration/stop_reason); the workflow.execute runtime contract keeps its real repair-cycle counters.
- **`circuit_breaker_open` alert now has a real producer**: the 30s alert loop reads `HyperResilienceEngine.profile().open_circuits` (same source as the Prometheus exporter) instead of the always-zero `RuntimeMetrics` snapshot field; the snapshot's four non-owned signals are documented as filled externally.
- **`serial_work_ms` wired to real values** in `workflow.execute`/`task.execute` learning events (was 0); chat-distillation events document why 0 = no timing data.
- **`record_autonomy_loop_stop_reason` value-domain unified**: chat-path `completed`/`tools_executed`/`all_tools_failed` now map into the complete/failed buckets (completion ratio was always ~0); the producer-less `escalated` bucket removed.
- **`fallback_unhealthy_ratio` removed** (numerator and denominator measured unrelated events); the real `fallback_unhealthy_agent_total` counter stays.
- **Constant-fake fields deleted**: `build_change_bundle.test_coverage` (no test data source), `org_policy.exceptions.active_total` (never tracked), `cpu_usage_percent` now reads the real perf snapshot, `clarification_quality_score` fake 1.0 removed.
- **Clarification metrics derived, not faked**: the chat-distillation learning event now calls `resolve_learning_clarification_metrics` (same source as `task.execute`), deriving rounds/quality/change-count from the task's latest requirement contract; 0 when no contract exists.

#### Speed / Dedup

- **`dedup_skill_calls` shared helper** (`orchestration/tool/mod.rs`) unifies CLI chat and ACP `run_agent_collecting`; fixes an ACP behavior bug where dedup dropped non-skill tools.
- **`build_cli_principles` cache key upgraded** from skill count to a content fingerprint (name+description hash), so skill description edits invalidate the cache.
- **SafeGuard cancel reports real prompt tokens** (was a placeholder 0); unused `_response` param removed.
- **`recommend_parallelism_from_learning_bus` filters to real execution sources** so distillation placeholder speedups no longer dilute parallelism recommendations.
- **i18n `format_template` helper** deduplicates `get_formatted`/`tf`.
- **AppConfig process-level mtime cache** (single config load per request), healthcheck report 2s TTL, `start_drift_monitor` lazified, HTTP clients unified, dead harness/communication/memory-vacuum APIs deleted (Rounds 1-3).

#### Verification

```
cargo check 4 profiles + --workspace   → zero warnings
cargo clippy --all-targets -D warnings → zero warnings
cargo test --lib                     → 1590 passed / 0 failed
```

---

### Round 39 — Architecture Unification & Speed Pass (2026-08-06/07, docs/log/log-20260806-7.md)

#### Security / Correctness

- **ACP_METHODS ordering fixed**: `tool.approve` / `terminal/create` were inverted, making both handlers unreachable in ACP mode via `binary_search`; the sorted-invariant is now enforced by a regression test (`acp_methods_list_is_sorted_for_binary_search`).
- **`WriteFileTool::run_async` sandbox hole closed**: the async path bypassed the 50MB size limit and sensitive-system-path block; both paths now share `enforce_write_sandbox`.
- **`detect_audio_format` out-of-bounds panic fixed** (RIFF header check now requires ≥12 bytes).
- **Double base64 fixed**: `document_parser` already base64-encodes embedded images; `process_document` no longer encodes them a second time.
- **`RepoAnalyzer::clone` temp-dir lifetime fixed**: the cloned repo directory is kept alive by the `RepoContext` (`Arc<TempDir>`), `head_commit` is fetched from `git rev-parse`, `loc` counts real lines, and `extract_types` reuses the repo map (single scan).
- **`AcpErrorCode::from_code`/`is_acp_request` repaired** after an external commit corrupted them (invalid match patterns, missing function header).

#### Three-Client Contract (principle #2)

- Rust/Python/Node SDKs: `task.plan`/`task.execute` now send `{task}` (matching the backend; previously `description`/`plan_id` failed with -32602 or were silently dropped).
- All four SDK `HealthResponse` types aligned with the real `ServerStatus`/`runtime.health` payloads (Rust SDK `health()` previously always failed strict deserialization).
- Node SDK: `health()` = `GET /health`, `runtimeHealth()` = `runtime.health` (methods were inverted); `taskPlan`/`taskExecute` parameter names fixed.
- VS Code workflow/process-flow views now flatten their local step model into the `task` param the backend actually reads.
- `knowledge.distill` callers now pass the backend's `limit` window instead of phantom `query`/`source` fields.

#### Anti-Fake-Fix / Metrics (principles #13/18)

- `SecurityGovernor` counters centralized in `record_audit` (denials/reviews/escalations were double-counted by `evaluate`+`record_audit`; `active_escalations` no longer increments on every audit).
- `harness_bus` `current_active_policies` replaced the hardcoded 12 with a real count (security-governor policies + 5 fixed bundles).
- Drift auto-baseline now keys history by metric name — `validate_action` vs `verify_output` latency no longer pollute each other's baselines.
- `governance.status` loads the config once (new `governance_config_summary_with`) instead of parsing the same TOML twice.
- `hash_file` rejects unknown algorithms (no more silent sha256 fallback mislabelled with the requested name) and dropped the unreachable sha1 branch; `random_token` now uses cryptographically secure `rand` instead of `fastrand`.
- `cache_strategy` case-insensitive scanning no longer allocates a lowercased copy of the conversation history.
- `default_non_ai_config_toml` writes the canonical `mode = "adaptive"`; OTel default-endpoint warning text matches the actual `localhost:4317`.

#### Performance

- `http_request` (async + blocking) uses one pooled reqwest client with per-request timeouts (was: a fresh client per call).
- `record_outcome` lock count reduced 7 → 4 (no unconditional `register_service`; lazy `entry()` initialization).
- `synthesize_keyword_heuristic` is O(n) (was O(n²) — `lines().nth()` rescanned the whole file per match).
- `health`/`release.readiness` payloads run independent profile futures with `tokio::join!`.
- `load_recent_knowledge_context` caches the knowledge artifact by mtime (no per-request file read).
- `web_search` and LSP clients are cached (no fresh TCP+TLS handshake / LSP initialize per call).

#### Dead Code / Lifecycle (principles #11/13)

- Deleted: `builtin_tools!` macro module, `ToolProgress`/`run_with_progress`/`run_streaming` subsystem, `ShardedGovernanceCache` hit/miss counters, `with_default_trigger_source`, `Agent::send_message` no-op, token-cache TTL machinery (never-set `ttl_ms`), `report`/`stats_snapshot`/`reset`/`put_string*`/`get_string`/`summarize_sync`/`expected_dimension` dead APIs.
- **Fault-tolerance recovery plans now complete or fail** after their post-recovery consistency check (`recovery_plans_in_progress` can no longer grow unbounded).

#### Documentation

- Cookbook API docs rewritten to the real surface: JSON-RPC over `POST /rpc` (not `/v1/responses`), the 7 real GET routes, real CLI flags/subcommands; fictional REST endpoints removed across `core-runtime`, `learning-intelligence`, `workflow-task`, `safety-governance`, `optimization-ops`, `observability`.
- `scripts/verify-zed-integration.sh` fixed (port 8090, `agent_servers` schema) — now PASSes.
- README sub-bus counts unified to the 7 feature-gated sub-buses; SDK count includes Node.js; unverifiable test-count numbers removed.
- `contracts/cross-client-sync.md` path references corrected.

### Round 39 (continued) — Legacy-Item Closeout (2026-08-07)

- **ApprovalEngine deleted** (verified clean): its HITL approval queue is fully replaced by the ACP chain — `executor.rs` → `request_client_permission` → SSE `PermissionRequest` notification → `session/request_permission` / `tool.approve` → `user_approved_tools` → `require_review` gate.
- **Fault-tolerance fault records wired**: `check_heartbeats` now creates a `FaultEvent` (NetworkTimeout, de-duplicated) when a node goes Offline, and `report_heartbeat` auto-resolves all unresolved faults when the node recovers — `active_faults()`/cluster health now reflect real outages.
- **Alert rule producers added**: a 30s background task evaluates the four previously producer-less rules (`p95_latency_high` via avg latency, `error_rate_high`, `circuit_breaker_open`, `agent_timeout_rate`).
- **Distributed memory transport activated**: new `DistributedMemoryBus::configure_from_env` (reads `GOON_MEMORY_PEERS`, `GOON_MEMORY_SYNC_INTERVAL_MS`, `GOON_MEMORY_AUTH_TOKEN`) wired in `server_builder` for multi-users-server; with no peers the bus stays local and no transport thread spawns.
- **Dead counters/APIs removed**: `CommunicationMetrics.tool_*` + `record_metrics`/`record_message_received` (AgentCommunicationHook slimmed to an empty struct); `AgentExecutionBudget.record_spawn`/`record_completion`/`record_tokens`/`can_spawn`/`reset`/`with_max_wall_clock`/`max_wall_clock_ms` (+5 dead-method tests).
- **Shared blocking HTTP client**: new `shared::http_client::blocking_http_client()` singleton; `web_scrape`/`rss_read`/`game` per-call client construction now reuses it with per-request timeouts.
- **Verified already-wired (no change needed)**: IdempotencyCache (producer `record_tool_success` at tools_pack.rs:812, consumer at :493) — the scan report was a false positive; kept: failover groups and remaining pub accessors (public API surface per principle #11).

#### Verification

```
cargo check 4 profiles + --workspace   → zero warnings
cargo clippy --all-targets -D warnings (4 profiles) → zero warnings
cargo test --lib                     → 1639 passed / 0 failed
```

---

### Round 38 — Legacy 7-Item Closeout (2026-08-06)

#### Docs Consistency Pass (2026-08-07)

- Documentation aligned with the codebase (principle #18): skill counts (33), JSON-RPC handler count (148), provider count (37), prompt template counts (149 templates / 16 categories), backend i18n key counts (711, flattened), the MCP tool table in `docs/protocol-guide.md` (the real 27 baseline tools), CLI flags (`--protocol-mode`; no `--export`/`--import`/`--mock`/`--dry-run`), config presets (`config/config.simple-server.toml`, etc.), and removal of references to non-existent env vars (`GO_ON_CONFIG`, `GOON_LOG`) and config sections (`[observability]`, `[access]`, `[logging]`, `[metrics]`, `[concurrency]`, `[timeouts]`, `[security]`, `[database]`).
- `.zed/settings.json` now pre-registers go-on as a Zed agent server (`agent_servers.go-on` + auto-approve + `auto_approve_tools`) exactly as documented in `docs/zed-integration.md` and checked by `scripts/verify-zed-integration.sh`; `scripts/verify-zed-integration.ps1` was rewritten to match the current settings schema and doc locations.
- `prompts/zh-CN.json` and `prompts/zh-TW.json` repaired (misplaced `goon_agent` category moved into a proper category; two raw newlines inside a zh-TW string escaped); `scripts/validate-prompts.sh` (incl. `--strict-i18n`) passes with zero errors/warnings.
- Test counts in README/README.zh-CN now defer to the latest full `cargo test --all-targets` run recorded below (Round 38: **3478 passed / 0 failed**) instead of stale per-profile numbers.

The 7 items left open by Round 37 (`docs/log/log-20260806-6.md`) are all completed and verified in one pass: duplicate logic collapsed to a single source, hidden duplicate stacks unified, and the self-evolution subsystem finally wired to a real LLM.

#### Duplicate Unification (principle #8)

- **Requirement auto-recovery merged 3× → 1×**: `evaluate_requirement_gate_facade` is now pure evaluation; the synthesize→inject→re-evaluate sequence lives only in `try_auto_recover_requirement_gate` (the single metric-recording point). Removed the dead `RequirementContinuationKind::ClarificationInProgress` variant, and **fixed a double/mis-count**: two manual `record_requirement_auto_recovery()` calls in `workflow_pack.rs` were removed — the metric is recorded once on successful recovery, and plain `Confirmed` continuations are no longer miscounted as auto-recoveries.
- **Agent token classification extracted 3× → 1×**: new `AgentToken` enum + `classify_agent_token()` in `autonomy_runtime.rs`; the three collection loops (`collect_agent_responses`, `run_agent_collecting`, `run_followup_after_tool_observation`) now share it. The SSE-forwarding loop in `autonomy_loop.rs` intentionally keeps its inline handling (distinct semantics: finish-reason/usage separation, tool-call text feedback, SSE frame shaping).
- **ACP bridge ↔ native MCP resources/prompts deduped**: five shared functions (`mcp_resources_list_value`, `mcp_resources_read_value`, `mcp_prompts_list_value`, `mcp_prompts_get_template_value`, `mcp_prompts_get_agent_value`) are now the single source for both the ACP `mcp.*` bridge payloads and the native MCP handlers; the three duplicate native builders were deleted.
- **Document parsing unified**: `read_pdf`/`read_docx` now delegate to `DocumentParser::parse_bytes` (the inline lopdf/docx-rs extraction copies were removed); `page_count`/`paragraph_count` compatibility fields are preserved and `images`/`tables`/`metadata` added (additive keys only). Object-level PDF merge/split and DOCX generation remain separate (not text extraction).
- **Token cache main-path write + 3× embedding eliminated**: `lookup`/`peek_similar` return the precomputed confidence (1.0 for L1/L3, the cosine score for L2), so `decide_from_entry` no longer re-embeds on every L2 hit; the primary (non-fallback) path now fills the token cache via `store_async` (previously only fallback/secondary paths wrote it); `CachedAgentWrapper::chat` applies the same execution-like bypass gate on hits.
- **Three embedding/similarity stacks unified (big refactor)**: `local_hash_embed` + `shared::math::cosine_similarity_f32` are now the only embedding/similarity implementation. The token cache `simple_embedding` (256-d), the semantic response cache (bigram/Jaccard replaced with precomputed embedding + cosine, 128-d), and the skill semantic matcher (f64/DefaultHasher bucket hashing replaced, 96-d) all delegate to it; the now-unused f64 `cosine_similarity` and its 4 dead-code tests were removed.

#### Wiring Activation (principle #14)

- **Self-evolution LLM wiring**: `SelfEvolutionAgent::with_llm` is fed a real agent resolved from `agent_registry` (assistant → summarizer → first) by the background task, so `generate_patch` uses the LLM path; `MemorySummarizer` is now `Clone` with an `Option<Arc<dyn Agent>>` LLM agent (built in `server_builder` and reused — no more default no-LLM instance in `get_or_init_memory_persistence`); `summarize_hot_entries` is async (snapshot → await real `summarize` → re-lock, no Mutex held across await) and `auto_migrate` awaits it. `analyze_code` docs corrected: it is deterministic static analysis and never calls the LLM (principle #18).

#### Verification

- `cargo check` all 4 profiles: zero warnings.
- `cargo clippy --all-targets -- -D warnings` (local/simple-server/multi-users-server/full): zero warnings.
- `cargo test --all-targets`: **3478 passed / 0 failed**.

#### Follow-up re-review of the three "keep" decisions (user question)

- **`run_autonomy_loop` now uses the shared `classify_agent_token`**: the reasoning start/end markers are never emitted by any agent (only consumed by the CLI/classifier), so the conversion is behavior-neutral — and it removes a latent control-character leak into `response`/SSE if an agent ever emits them. Also deleted the write-only `round_response` accumulator (5 pushes, 0 reads).
- **`pdf_split` no longer parses the file twice** (page count and page deletion share a single load); merge/split share new `load_pdf_document`/`save_pdf_document` helpers. Object-level page-tree manipulation still correctly stays outside `DocumentParser` (text extraction only).
- **The three word-tokenizers remain separate** (`signature_similarity`, `semantic_matcher::tokenize`, `execution::tokenize_text`): different min-length/unicode/set/scoring (Jaccard vs Dice vs TF+tag) — domain tuning, not duplication.

### Rounds 32–37 Deep Scan & Cleanup (2026-08-06)

Seven more super-deep + super-broad scan rounds (see `docs/log/log-20260806-{1..6}.md`), converging on the same principles: no dead code, no placeholders, no fake fixes, unified three-end architecture.

#### Anti-Fake & Honesty Fixes (principle #13/#15)

- **Imported-skill execution implemented**: `mcp.tools.call` on an imported skill previously returned a fake `NOT_IMPLEMENTED_EXECUTOR` success; it now executes through the real `PromptBasedSkill` LLM executor (and fails loudly when no LLM agent is wired or the manifest has no executable content).
- **`health.check` propagates failures**: the RPC previously always returned `{"ok": true}` even when the health probe failed.
- **`workflow.execute` real reviews**: the fabricated `APPROVE` reviewer entries were replaced with real deterministic verification of the execution summary; `review_status` now reflects the actual outcome, and the autonomy contract reports the real repair-cycle corrective-action counts and effectiveness.
- **`game_monitor`/`game_screen_capture`/`game_replay_recorder` honesty**: `window_active` is derived from the real process state; screen capture reports failure when no capture binary exists; the replay recorder actually runs ffmpeg (x11grab) instead of returning a "ready" command hint.
- **Audit-chain rotation preserves signing**: a signed chain (`GOON_AUDIT_SIGNING_KEY`) no longer becomes unsigned after the 100 MB rotation.
- **Distributed-memory transport is real HTTP**: the multi-users-server `do_sync` no longer simulates peer transmission (it previously ingested entries locally and reported `Completed`); it now POSTs JSON-RPC `memory.ingest` to each peer's `/rpc` endpoint, and the hub server gained the matching `memory.ingest` handler. Failures are reported, never faked.
- **PostgreSQL init retry implemented**: `initialize_postgres_backend` documented a 3-attempt exponential-backoff retry but never retried; the retry loop (1s/2s/4s) now runs on the blocking pool as documented.

#### Dead Code Elimination (principle #11)

- Removed the producer-less `IDEMPOTENCY_HIT_TOTAL` counter, the never-called `GovernanceStatus::to_json`, `record_audit_threadsafe`, `McpServer.logging_level` field, the `SESSION_UPDATE` fast-path entry, the `mcp/tools.rs` forwarding shell (error codes moved into `mcp/mod.rs`), and the dead `_client` parameter of `dispatch_server`.
- Collapsed the harness_bus `AuditEntry` intermediate type (both call sites now build the canonical `AuditLogEntry` directly); the intelligence hub's `AUDIT_ENTRY_COUNT` static now reads the canonical sink length.
- Unified the three private SHA-256 wrappers into `shared::sha256_bytes`/`sha256_hex`; `time.rs` now uses `shared::timestamps`.

#### Wiring Activation (principle #14)

- **Tool fallback chains live in the executor**: the autonomy loop and ACP agent runtime now consult each tool's configured fallback chain (`read_file→search_files`, `grep→search_files`, etc.) via a new hook-free fallback helper, matching the CLI path.
- **Fault-tolerance recovery cycle scheduled**: `FaultToleranceEngine::run_recovery_cycle` now runs on a 30 s interval in `start_background_tasks` (previously only tests invoked it).
- **`state_sync` model/agent events published**: `config.reload` now diffs the agent set and configured models and emits `AgentsChanged`/`ModelsChanged`, un-dead-ing the GUI/VS Code `onModelsChanged`/`onAgentsChanged` handlers.
- **Governance audit unified into the canonical sink**: `governance.plan.update` events now flow through `global_audit_log()` (chained, rotated) instead of a second un-chained `.goon/governance/audit.ndjson` file; `governance.audit.recent` reads the in-memory sink (no per-request file I/O).
- **Drift monitor got real producers**: `validate_action`/`verify_output` feed latency metrics into `DriftProtectionEngine` with a registered performance drift policy, so the 60 s monitor evaluates real data.

#### Duplicate Unification & Correctness

- `web_scrape` and `rss_read` now enforce the same `http_request` URL sandbox (`validate_url`), closing the SSRF/private-IP bypass.
- `is_low_risk_tool` stale names fixed (`time_util`/`diff`/`rss_feed` → `date_time`/`file_diff`/`rss_read`); CLI `/grep` now hits the registered content `grep` tool instead of being shadowed by the `search_files` alias.
- MCP `filter_tools_by_exposure` stale deferred names fixed (`container_*`→`docker_*`, removed `compile_and_run`/`qrcode_`); gzip decompression shared between `decompress` and archive extraction.

#### Performance

- Startup: config validation no longer loads the TOML twice; Copilot proxy probing + client construction cached per env snapshot (up to ~700 ms saved per `provider.list_models`/device-code call); `/proc` memory/CPU reads cached with a 5 s TTL across status/health/metrics endpoints.
- Request path: data-URI attachments processed with `join_all`; `observe_phase` shares the process HTTP client; document parsing offloaded via `spawn_blocking`; MCP HTTP JSON-RPC batches dispatched with `join_all`; capability-bus selection and vector-context load run in parallel; the multi-agent safety net no longer marks fresh executions as cache hits.

#### Three-End Alignment

- **VS Code addon phantom RPCs mapped to real methods**: `approval.approve/reject` → `session/request_permission`, `checkpoint.create` → `conversation.checkpoint.create`, `skill.import_local` → `skill.import`, `runtime.reload_config` → `config.reload`, `checkpoint.load` → `checkpoint.list` (with warn), destructive commands (`chat.delete`/`session.clear`/`memory.clear`) → `session/delete`/`vector.clear`; `config.reset`/`agent.remove` now fail loudly with a warn instead of sending a doomed request.
- **TypeScript SDK phantom methods renamed** to the backend names used by the other three SDKs: `workflow.plan`→`task.plan`, `summary.get`→`learning.summary`, `knowledge.search`→`knowledge.distill`, `rl.optimize`→`rl.alignment.offline_eval`.

#### Validation

- `cargo check` 4 profiles + `--workspace`: zero warnings.
- `cargo clippy --all-targets -- -D warnings` on local / simple-server / multi-users-server / full: zero warnings.
- `cargo test --all-targets`: **3486 passed / 0 failed**.
- `scripts/gen-provider-catalog.py --check`: dual output OK (37 providers).

---

### 24 Rounds of Deep Scan & Optimization (2026-07-24 → 2026-08-05)

Version 1.5.0 consolidates 24 rounds of super-deep + super-broad multi-agent scanning (see `docs/log/`), converging on the principles in `docs/blueprints/principle.md`: no dead code, no placeholders, no fake fixes, unified architecture across backend / GUI / VS Code.

#### Rounds 25–29 Refinement (all under 1.5.0)

- **Circuit breaker unification**: the per-agent failure-prevention state machine (~600 lines) was retired; health monitoring, degradation strategy, recovery, and breaker snapshots moved into `HyperResilienceEngine`, now the single resilience authority (`breaker.*` RPCs / `governance.status` / health probes read one source).
- **Provider model suggestions unified**: the GUI's hand-maintained ~180-line model table moved into the backend authority (`ProviderSpec::model_suggestions`) and is generated with `gen-provider-catalog.py --check` parity.
- **Audit pipeline unification**: the canonical audit sink now hash-chains **every** persisted entry into `~/.goon/audit_chain.ndjson` (single writer thread, exact ordering, size-based chain rotation). The standalone per-server `HashChainAuditor` plumbing and the per-request `spawn_blocking` append were removed; the request ledger now records through the sink (redacted, non-blocking); optional Ed25519 signing (`GOON_AUDIT_SIGNING_KEY`) and the new `governance.audit.verify` RPC (chain summary, integrity violations, time-window report) close the verification loop.
- **SSE + SDK alignment**: VS Code/Node.js SSE chunk parsing aligned to the SSE contract; the Node.js chat stream is a true incremental AsyncGenerator; `tests/e2e/` renamed to `tests/structural/`.
- **Bench regression fixed**: `benches/acp_bench.rs` raw-string delimiter break produced invalid JSON at runtime (criterion `--test` mode panic).
- **Round 30 — SSE field extraction unified**: `extract_chunk_text` / `extract_agent_model` / `extract_result_meta` live in `gui/src/backend/state.rs` (single source of truth); both the rich stream path and the non-streaming fallback consume them, eliminating the `token`/`text` fallback drift.
- **Round 30 — backoff skeleton unified**: new `gui/src/backoff.rs::exp_backoff_ms` shared by health polling, crash-restart limiting, channel-full retry, and the RPC retry base.
- **Round 30 — cross-SDK backoff contract drift fixed**: Rust/Node/Python/TypeScript SDKs implemented AWS full-jitter instead of the contract's ±30% jitter (`min(base×2^n, 30s) × (0.7+random×0.3)`); all four now match the GUI/VS Code implementations; VS Code binary-download retry gained the missing 30 s cap.
- **Round 31 — `governance.audit.verify` wired end-to-end**: typed wrappers added to all 4 SDKs, a new VS Code `go-on.governanceAuditVerify` command, and an e2e test that round-trips the RPC against a real spawned binary.
- **Round 31 — TS SDK test suite unblocked**: `node_modules` installed and the suite ran for the first time; fixed a timeout regression from the round-30 backoff change (`maxRetries: 0` in the HTTP-error test) and a pre-existing hang in the abort-stream test (mock stream now closes on abort).

#### Redundancy Elimination & Unification

- **Provider catalog triplication → 1 authority + 2 generated artifacts**: `src/core/providers.rs` is the single source of truth; `gui/src/views/providers/generated_catalog.rs` and `vscode-addon/src/settings/providerCatalog.generated.ts` are generated by `scripts/gen-provider-catalog.py` (with `--check` parity validation). VS Code catalog caught up (added kimi/siliconflow, env-var mapping, grouping derived from backend).
- **MCP bridge ↔ native handler drift closed**: `mcp.resources.list` no longer returns an empty list over the ACP bridge; fake-success no-ops (`mcp.resources.subscribe`, `mcp.logging.setLevel`, `mcp.completion.complete`) replaced with honest implementations/errors matching the native `src/mcp/handlers.rs`.
- **PostgreSQL TLS connect stack merged** (~200 duplicated lines): `parse_sslmode` / `PermissiveVerifier` / `connect_postgres` unified into `src/memory/pg_pool.rs`.
- **Duplicate clock helpers merged**: `agents::unix_now_secs` now delegates to `shared::timestamps::now_ts`.
- **`keyring://` constant unified** across `agents`, `acp::helpers::planning::context`, `config_validation`, `env_override`.
- **Dead-committed artifacts removed**: 8.5 MB `scripts/go-on` binary, empty `debug_binding.py`, orphaned shell scripts, dead TypeScript exports, dead Rust API surface (`Agent::on_message`/`send_message`, `AgentMessenger::with_capacity`/`peek`, `new_safeguard`).

#### PostgreSQL Production Hardening

- Connection pooling (`deadpool`) with read/write replica split, versioned migrations, `sslmode` TLS support (require/verify-ca/verify-full).

#### Functional Completion (Gaps Closed)

- **F-GAP-66 multimodal attachments**: GUI attachments (file-picker + paste/drag) now flow through the backend multimodal pipeline (image extraction, document parsing, audio transcription, `repo:` analysis) instead of text-only summaries.
- **MCP `initialize` capability declaration unified**: only capabilities with real handlers on both native and bridge entries are advertised; `sampling` removed from the shared declaration.
- **Copilot URL authority converged** to `https://api.githubcopilot.com` in the provider catalog (was a drifted localhost copy).
- **`build_role_routing` now reads the populated global role registry** (was constructing an always-empty registry and reporting `available_custom_roles: 0`).

#### SDK Protocol Drift Fixed

- `checkpoint.create` → `conversation.checkpoint.create` (with required `conversation_id`) across rust / nodejs / python SDKs.
- nodejs `runtime.initialize`/`runtime.shutdown` → canonical `initialize`/`shutdown`.
- `breaker.reset` param contract aligned to backend (`agent`/`name`).

#### Docs & Versioning

- All product versions aligned to **1.5.0** (workspace, GUI, VS Code addon, rust/nodejs/python/typescript SDKs, crates).
- CHANGELOG restored the missing `[1.2.0]` entry (previously stranded as stale `[Unreleased]`).
- README stats corrected against measured values (2018 tests, 37 providers, 37 marketplace skills, ~238K LOC, 13-sub-bus architecture); CI badge URL fixed.

### Validation

- Backend: `cargo check --all-targets` clean; `cargo test` green; `cargo clippy --all-targets -- -D warnings` zero warnings.
- GUI: `cargo check` clean.
- VS Code addon: `tsc --noEmit` + mocha green.
- Provider generator: `scripts/gen-provider-catalog.py --check` dual-output OK (37 providers).

## [1.4.3] - 2026-07-24

### BLUE71 — Cross-System Deep Analysis & High-Impact Improvements

This release implements all 9 improvement plans from BLUE71, closing the architectural gaps identified through deep comparison with Codex and Harness Gitness. Total completion rate: 100%.

#### SessionActor — Tree-based Session Architecture (§2.1.1)

- **SessionLifecycle**: Finite state machine — Created → Ready → Active → Draining → Archived with watch channel propagation.
- **SessionInput**: Actor-model message queue (mpsc) with UserMessage, Cancel, and Steer variants.
- **SessionHandle**: External interaction handle with send_message(), cancel(), steer(), lifecycle subscription.
- **SessionState**: Owns CommunicationBus, ConversationHistory, CompactionManager, FragmentRegistry, AgentGraphStore.
- **session_main_loop**: Persistent tokio task that processes SessionInput, manages lifecycle transitions, and triggers auto-compaction.
- **Integration with AgentThread**: SessionActor spawns one AgentThread at startup, reuses it across all UserMessages via ChatRequest.
- **Graceful Drain**: Cancel → send Cancel to AgentThread → lifecycle: Draining → Archived.

#### AgentThread — Non-blocking Agent Spawn with Persistent Loop (§4)

- **AgentThread**: Non-blocking agent execution handle with input queue, status watch channel, and JoinHandle.
- **spawn_agent_non_blocking()**: Returns immediately with AgentThread handle. Agent runs as independent tokio task.
- **agent_main_loop**: True persistent Actor loop — no `break` after single message. Processes UserMessage, ChatRequest, Cancel continuously.
- **ChatRequest variant**: Accepts full message list with system prompt + options + oneshot reply channel, enabling SpawnAgentTool integration.
- **SpawnConfig**: Configurable max_depth, max_concurrency, token_ceiling, timeout_secs.

#### SpawnGuard — RAII Concurrency Slot Protection (§5)

- **SpawnGuard**: Atomic counter with try_reserve/commit/release_slot/Drop. Auto-releases on panic (no leaks).
- **Commit pattern**: Ownership transfers from caller to spawned task. Spawned task releases on completion.
- **Integration**: SpawnGuard replaces static Semaphore in SpawnAgentTool. Also used by SessionActor for AgentThread budget.
- **Current usage tracking**: `SpawnGuard::current_usage()` for observability.

#### Event-driven State Propagation — Zero Polling (§6)

- **AgentMessenger.notify**: Watch channel incremented on each message delivery.
- **wait_for()**: Uses `notify_rx.changed().await` instead of `tokio::time::sleep` polling.
- **AgentNode.lifecycle_tx**: Watch channel sender for lifecycle state — subscribers notified on each transition.

#### AgentLifecycle — Finite State Machine (§7)

- **AgentLifecycle enum**: 6 states — Registered, Idle, Active (with phase: Planning/Executing/Reflecting/Waiting), Completed, Errored, Cancelled.
- **AgentLifecycleBuilder**: Convenient construction with automatic timing.
- **Integration**: Every AgentNode in the tree carries `lifecycle_tx: watch::Sender<AgentLifecycle>`.
- **Summary method**: Human-readable state description for logging and debugging.

#### AgentGraphStore — Persistence Abstraction (§8)

- **AgentGraphStore trait**: upsert_edge / set_edge_status / list_descendants / remove_subtree.
- **InMemoryAgentGraphStore**: HashMap-based default — thread-safe via Arc<RwLock>.
- **SqliteAgentGraphStore**: SQLite-backed (feature: backend-sqlite) — rusqlite + spawn_blocking pattern.
- **Checkpoint serialization**: ConversationHistory.to_checkpoint_json() / from_checkpoint_json() — full JSON roundtrip.
- **Integration**: SessionState holds `graph_store: Arc<dyn AgentGraphStore>`. Checkpoint stores serialized history as an edge.

#### ContextFragment — Structured Context Injection (§9)

- **ContextFragment trait**: role() / priority() / body() / weight() for injectable context pieces.
- **FragmentRole**: System, Developer, User — controls where fragment appears in prompt.
- **FragmentPriority**: Low, Normal, High, Critical — Critical always included regardless of token budget.
- **FragmentRegistry**: register() + build_context(budget) + build_context_pairs(budget) with priority sorting and budget-aware truncation.
- **SimpleFragment**: Built-in implementation for static string-based fragments.
- **Integration**: SessionState.fragments populates system prompt in UserMessage handler.

#### AdaptiveCompactor — Self-learning Conversation Compaction (§10)

- **ConversationTurn / ConversationHistory**: Token-aware conversation tracking with drain, prepend, to_text operations.
- **CompactionStrategy**: SlidingWindow (keep N turns), Summarize (LLM summary), Hybrid (summary + keep recent).
- **CompactionManager**: Synchronous compaction engine — works in any context without tokio runtime.
- **AdaptiveCompactor**: Self-learning — auto-selects strategy based on conversation length and historical quality scores.
- **AdaptiveThreshold**: Dynamic threshold — raises on high quality (compact less), lowers on low quality (compact more aggressively).
- **User feedback integration**: quality * 0.6 + feedback * 0.4 blended score.
- **30 tests**: ConversationTurn, ConversationHistory, CompactionManager, AdaptiveThreshold, AdaptiveCompactor.

#### GuardianReviewer — Independent Model Review (§11)

- **GuardianReviewer**: Uses a separate agent instance to review tool actions before execution.
- **GuardianDecision**: Allow / Deny / EscalateToUser — fail-closed (error/timeout/parse-failure → Deny).
- **GuardianCircuitBreaker**: Dual-threshold — max consecutive denials (3) + max recent denials (10/50).
- **from_registry()**: Look up review agent from AgentRegistry — returns None for graceful fallback.
- **16 tests**: Circuit breaker, decision parsing, allow/deny/invalid/trips.

#### Cross-module Refactoring & Cleanup

- **agent_main_loop break removed**: Both UserMessage and ChatRequest handlers continue the loop — persistent agent.
- **InterAgentComms stub removed**: Variant had empty handler (only logging) — removed per principle §9.
- **SessionActor async**: spawn_session changed from sync (with block_on) to async fn.
- **panic! elimination**: spawn_session returns Result<SessionHandle, String> instead of panicking on path parse.
- **Root path caching**: AgentPath::parse("root") parsed once, cached in SessionState.
- **Code cleanup**: Zero #[allow(dead_code)] or #[expect(dead_code)] in production code. Zero unused imports.

#### New Files

| File | Lines | Description |
|------|-------|-------------|
| `src/agents/session.rs` | ~700 | SessionActor tree architecture |
| `src/agents/graph_store.rs` | ~280 | AgentGraphStore trait + InMemory + SQLite |
| `src/agents/fragment.rs` | ~300 | ContextFragment trait + FragmentRegistry |
| `src/governance/guardian.rs` | ~600 | GuardianReviewer + circuit breaker |
| `src/optimization/compaction.rs` | ~1000 | AdaptiveCompactor + conversation types |

### Validation

- **New tests**: ~70 new tests across all new modules.
- **All profiles**: local, simple-server, multi-users-server, full compile cleanly.
- **Blueprint compliance**: 100% of BLUE71 P0/P1/P2 improvements implemented.

## [1.4.2] - 2026-07-23

### BLUE70 — Multi-Agent Communication System + Architecture Consolidation

This release introduces a complete multi-agent tree-based communication system and consolidates the 14-bus architecture down to 11 core buses + 1 communication bus.

#### New: CommunicationBus (Agent Tree Communication)

- **AgentPath**: Hierarchical agent addressing (`root/research/coder`) with simplified wildcard matching (`root/*/coder`).
- **AgentMessage**: Structured inter-agent message types — Delegate, Result, Progress, Cancel, StatusQuery, Custom.
- **AgentTree**: Lightweight hierarchical agent index with flat HashMap + parent pointers + BFS traversal (no recursion).
- **AgentMessenger**: Two-level message delivery (AtMostOnce / AtLeastOnce) with inbox routing and cancellation propagation.
- **CommunicationBus**: Top-level bus aggregating AgentTree + AgentMessenger, with profile and health endpoints.
- **SpawnAgentTool integration**: Each sub-agent spawn is registered in the CommunicationBus AgentTree for observability.
- **AgentCommunicationHook**: ToolHook that records execution metrics on every spawn_agent call.
- **Agent trait extension**: New `on_message()` and `send_message()` methods on the Agent trait for future agent-to-agent communication.

#### Architecture Consolidation (14 → 11 Buses)

- **UnifiedKnowledgeBus**: Merged KnowledgeBus + ReputationStore + ExperienceKnowledgeBase into one cohesive bus with unified query/record APIs and EMA reputation smoothing.
- **ReinforcementBus**: Merged QLearningAgent + FederatedRL with epsilon-greedy action selection and optional federated coordinator.
- **LearningOptimizationBus**: Merged WorkflowLearningBus + OptimizationBus with atomic `record_and_optimize()` — learns from events and generates optimization suggestions and prevention rules.
- **Legacy code removed**: All 6 legacy bus fields, 2 legacy struct definitions, and 3 legacy imports deleted (~250 lines). 22 call sites migrated.

#### ForkRegistry Enhancement

- New fields: `agent_path`, `parent_agent_path`, `budget`, `started_at_ms`, `completed_at_ms` on `ForkEntry`.
- New methods: `with_agent_path()`, `with_parent_agent_path()`, `with_budget()`, `mark_completed()`.

#### Quality

- 97 new tests across all new modules, 276 BLUE70-related tests total.
- Zero clippy warnings, zero compilation warnings.
- All 3 server profiles (simple-server, multi-users-server, full) compile cleanly.
- Full backward compatibility preserved — Agent trait extended with default implementations.

### Architecture — Transport Trait Phase 4 Complete + i18n Unification

This release completes the Transport Trait migration (Phase 4) and unifies error messages across all three interfaces (CLI, GUI, ACP).

#### Transport Trait — Phase 4 (RPC_BUFFER Elimination)

- **RPC_BUFFER task-local removed**: All JSON-RPC output now routes through `CURRENT_TRANSPORT` (RwLock-based global transport), eliminating the dual-path legacy mechanism (io.rs).
- **RpcBufferTransport wired**: HTTP RPC handler (`/rpc`) and TLS handler now use `set_current_transport(RpcBufferTransport)` instead of `RPC_BUFFER.scope()`. Response capture remains identical.
- **SseTransport wired**: `/chat/stream`, `/v1/chat/completions`, and `/v1/responses` handlers set `SseTransport` per-connection.
- **CURRENT_TRANSPORT upgraded**: OnceLock → RwLock, enabling runtime transport switching between stdio/HTTP-SSE/HTTP-RPC modes.
- **Test serialization**: Dispatch tests use `DISPATCH_TEST_LOCK` to prevent parallel races on the global transport.
- **dead_code annotation cleanup**: All Phase-1/2 stale `#[allow(dead_code)]` removed from transport.rs.

#### i18n Error Message Unification (Cross-Module)

- **12 new CLI i18n keys**: `cli.chat.git_diff_failed`, `cli.chat.summarization_failed`, `cli.chat.ai_review_failed`, `cli.chat.find_path_usage`, `cli.chat.tool_call_limit_mode`, `cli.chat.conversation_long_warning`, `cli.chat.tool_blocked_by_mode`, `cli.chat.tool_call_blocked_by_mode`, `cli.chat.tip_compact` — added to en-US.json, zh-CN.json, zh-TW.json.
- **6 new GUI i18n keys**: `chat.error.*` hint keys matching the backend error template format.
- **10 hardcoded CLI eprintln! messages migrated** to i18n `t()`/`tf()` calls.
- **GUI Backend empty response**: Replaced misleading canned message with transparent empty-content propagation (the GUI's `finalize_stream_result` already provides helpful diagnostics).

#### Chat Loop Reliability

- **GUI generation timeout protection**: Added `generation_deadline` — after 330s, stuck generations are force-reset with error, preventing permanent UI lock.
- **GUI event overflow fix**: `process_pending` changed from fixed-cap event processing to unbounded `while let Ok(...)` drain, eliminating silent event drops under high token throughput.
- **GUI empty-generation-id cleanup**: Added fallback cleanup of orphan empty-assistant messages when `generation_id=None`.
- **GUI phase sync**: Stream request body now includes `phase` field; `ChatCompleted` responses carry `actual_mode` for backend-side mode sync.
- **GUI SSE flush optimization**: `/v1/chat/completions` streaming path changed from per-event flush to batch flush every 4 events (matching `/chat/stream` behavior).
- **GUI StreamProcessor field removed**: Eliminated dead field that was set but never read.
- **GUI split_thinking dead code fixed**: `extra_thinking` from content is now properly merged with authoritative thinking before display.
- **CLI stdin async**: Replaced `spawn_blocking` with `tokio::io::stdin().lines()` for responsive Ctrl+C handling.
- **CLI Ctrl+C re-arm**: `signal::ctrl_c()` now re-armed every iteration, enabling multiple interrupts.
- **CLI mode persistence**: Mode saved to `goon-cli-mode.json` on `/mode` switch, restored on startup. `GOON_DEFAULT_MODE` env var supported.
- **CLI failed-message cleanup**: Failed assistant messages automatically removed from history on error.
- **CLI input backpressure**: `unbounded_channel` → bounded channel(32) for paste-storm protection.
- **CLI multi-line input**: Support for backslash continuation, whitespace continuation, and unbalanced-brace detection.

#### ACP/ZED Agent Server Integration

- **Platform profile injection**: `initialize`, `session/new`, `tools/list` responses now include `platform_metadata` with available modes, capabilities list, and default mode.
- **session/prompt thinking regex**: Now supports BOTH `<thinking>...</thinking>` AND `__thinking__` prefix formats.
- **session/close cleanup**: Session close and delete now clean up permission state to prevent stale grants.
- **session/config per-session**: Verified and documented — `session_set_config_option` already stores per-session via `acp_session_state().entry()`.
- **MCP notifications/initialized**: Now returns `id: Some(Value::Null)` sentinel (skipped by dispatch layer), preventing Zed's client from logging spurious errors.

#### Concurrency & Configuration

- **AgentFactory lock unification**: Consolidated `instances` and `expirations` into a single `AgentFactoryInner` behind one Mutex, eliminating the TOCTOU race between capacity check and insert, and removing the double-lock crash-safety gap in `destroy_agent`.
- **Config hot-reload**: Reduced full-config clones from 2 to 1; eliminated stale snapshot read race by capturing before dropping write guard.
- **Config parser fix**: Auto-rules now applied AFTER schema migration to avoid referencing stale phase names; parse result validated before writing to disk.
- **Config serde safety**: `flow` field in `AppConfig` now has `#[serde(default)]` — missing `[flow]` section deserializes as default rather than failing.

#### Code Quality

- **`is_clean()` cfg(test) fix**: Changed from `#[cfg(test)] pub fn` to `#[cfg(test)] pub(crate) fn` — the previous form would fail to compile if called from another module in non-test builds.
- **18-line commented criterion benchmark removed** from `adaptive_selector.rs`.
- **`connect_direct_for_test` renamed** to `connect_direct` — the method is used in both production and tests.
- **All dead_code allows cleaned**: Zero `#[allow(dead_code)]` or `#[expect(dead_code)]` in production code.
- **All profiles zero warnings**: `local`, `simple-server` 0 warnings; `multi-users-server` only 2 pre-existing `config_path` warnings.

### Validation

- **Tests**: 2069 passed, 0 failed, 0 ignored (full suite).
- **GUI tests**: 25 passed, 0 failed.
- **MCP tests**: 20 passed, 0 failed.
- **Agent Factory tests**: 12 passed, 0 failed.
- **Config core tests**: 49 passed, 0 failed.
- **ACP tests**: 385 passed, 0 failed.
- **Clippy**: `-D warnings` zero violations (backend + GUI).
- **Profiles**: `local`, `simple-server` zero warnings.

## [1.3.0] - 2026-06-23

### Architecture — Lock Contention Elimination (Phase 4)

This release completes the systematic lock architecture upgrade across the entire runtime, eliminating 12 hot-path mutex contention points through precision lock-type selection and channel-based offloading.

#### Mutex → RwLock (Read-Heavy Paths)

- **agent_router** (1 file): Global route statistics table upgraded from `Mutex` to `RwLock`. Concurrent agent routing queries no longer serialize against each other.
- **agent_preference** (1 file): Agent-to-phase binding state upgraded from `StdMutex` to `RwLock`. Every-request phase resolution reads proceed in parallel.
- **semantic_cache** (4 files): Semantic response cache upgraded from `StdMutex` to `RwLock`. Near-duplicate request detection reads are now concurrent.
- **skill_registry** (17 files): Global skill registry upgraded from `Arc<StdMutex>` to `Arc<RwLock>` across the entire call chain including orchestration, MCP handlers, capability bus, and autonomy adapter. Every-query skill scoring and retrieval reads are now lock-contention-free.
- **maintenance_tracker** (3 files): 100% read-only diagnostic snapshots — RwLock eliminates unnecessary serialization.
- **inflight_limiter** (2 files): 100% read-only diagnostic snapshots — RwLock eliminates unnecessary serialization.
- **lifecycle_state** (3 files): 80/20 read/write ratio. Server health checks (read) no longer block each other; the single shutdown write is unaffected.
- **review_timeout_policy** (1 file): Dead field converted as part of structural consistency.

#### Mutex → mpsc Channel (Write-Heavy Hot Path)

- **online_controller** (6 files, 13 call sites): The most significant architectural change. Nine write-only outcome recording calls (record_agent_outcome, record_phase_outcome) on the request hot path are now dispatched via `mpsc::UnboundedSender` — zero lock contention. Four read calls that return values (rank_agent_names_for_phase, recommend_phase, phase_policy_snapshot) retain synchronous lock access. A background event processor drains the channel and applies mutations asynchronously.

#### Clone Dead Code Removal

- **HyperResilienceEngine** (1 file): Removed the `Clone` implementation which sequentially acquired 5 internal locks. The impl was never called in production (all instances behind `Arc`), making it both dead code and a latent deadlock risk.

#### Semantic Precision — Intentionally Retained StdMutex Fields

Three fields in `ResilienceContext` remain as `StdMutex` after analysis showed RwLock would provide no meaningful benefit:
- **circuit_breakers** (62% read, 38% write): Internal double-locking makes outer RwLock irrelevant.
- **failure_prevention** (50/50 balanced): RwLock write-path identical to Mutex; no gain.
- **phase_rate_limiter** (60% read, 40% write per-request): Every-request token bucket mutation is a write; RwLock serializes the same.

### Dead Code Elimination

- **run_health_check**: Replaced no-op stub with real subsystem verification (governance, runtime config, agent registry).
- **BrainLoopReport** and `with_diagnostic_feedback`: Removed deprecated structures and methods from reflection module.
- **Pipeline variants**: Removed 5 dead `PipelineStep` and `PipelineErrorStrategy` variants (Parallel, Sequence, Conditional, Stop, Rollback) and all associated branch functions/tests.
- **execute_with_two_phase_coordination**: Removed entire 2PC coordinator function (reserved F-GAP-49, unused).
- **PluginRegistry::unregister**, **SkillDiscovery::invalidate_cache**, **session_context** dead methods: Removed individually tagged dead code.
- **DiagnosticFeedbackEngine** dead method chain: Removed `has_errors`, `recommend_repair`, `latest_batch` and 3 associated tests.
- **sign_request**, **make_signature_for_test**, **subscriber_count**: Removed test-only helper functions with zero callers.
- **ApprovalPolicySuggester::new()**: Removed redundant constructor (Default trait provides the same).
- **HyperResilienceEngine::clone()**: Removed dead Clone impl (5-lock sequential acquisition).
- **e2e test dead imports**: Removed `ImageAttachment`, `MtlsConfig`, `sign_request` imports and associated test code.

### Build & Lint Cleanup

- **temp_env dependency**: Moved from optional feature-gated dependency to `[dev-dependencies]` — resolves 3 test compilation failures in `federated_transport.rs`.
- **BrainLoopReport visibility**: Added `pub use reflection::BrainLoopReport` — resolves test compilation error.
- **Empty coordinator module**: Removed `pub mod coordinator` and deleted empty file — eliminates 6 dead-code warnings.
- **Clippy lint fixes**: 7 lints resolved (manual_pattern_char_comparison, len_zero, manual_is_multiple_of ×4, needless_borrow, for_kv_map, manual_range_contains, unused import).

### Test Reliability

- **video_processor test**: Repaired inconsistent ffmpeg detection — test now handles both available and unavailable ffmpeg uniformly via match, eliminating a spurious panic.
- **shell_exec test**: Made environment-robust — accepts timeout as valid outcome on systems without `sh` access (macOS CI), no longer panics.

### Performance

- **I18nManager::clone()**: Redesigned from deep-copying all translations (O(n) per clone) to `Arc<I18nInner>` sharing (O(1)). The previous implementation cloned the entire `HashMap<Language, HashMap<String, String>>` on every clone.

## [1.2.0] - 2026-06-10

### Changed
- **Version updated**: go-on v1.1.0 → v1.2.0
- **Performance**: Startup time reduced from 180s+ to seconds by eliminating redundant
  MemoryPersistence initialization in `new_acp_server()`.
- **Memory bridge**: Auto-migrate task moved to `start_background_tasks()` for all
  4 protocol modes; initial `bridge_promote` now runs on HTTP/WebSocket modes too.
- **Config format migration**: GUI config format changed from JSON to TOML.
  `load_app_config()` and `save_app_config()` now delegate to the TOML-based
  load/save paths. Existing `gui_config.json` files are automatically migrated
  to `gui_config.toml` on load (the JSON file is preserved as a backup).
- **SSE parser unification**: The inline SSE frame parser in
  `gui/src/views/chat/chat_impl/runtime.rs` was replaced with the shared
  `StreamProcessor` from `gui/src/backend.rs`. All GUI consumers now use a
  single SSE parsing implementation.
- **SSE protocol contract**: Created `contracts/sse-protocol.md` as the single
  source of truth for the SSE wire format, event types, and parsing contract.
  The VSCode `runtime/sseStream.ts` now references this contract.

### Security
- **VSCode stderr sanitization**: Raw stderr output in `runtimeManager.ts` is
  now sanitized to redact potential API keys and long base64-like secrets before
  display in the output channel.
- **VSCode OAuth client ID**: `settingsView.ts` now falls back to the
  `GO_ON_COPILOT_CLIENT_ID` environment variable for the Copilot OAuth client
  ID, allowing it to be configured without hardcoding or manual entry.
- **VSCode activation retry**: `extension.ts` now retries activation once after
  a 2-second delay if the initial activation fails, improving resilience.

## [1.1.0] - 2026-05-26

### Changed
- Project version updated to 1.1.0 across all modules
- Zero dead-code suppression — all `#![allow(dead_code)]` removed or replaced with feature-gated cfg_attr
- Zero compiler warnings — cargo check (bin + tests) + clippy -D warnings all clean across 3 profiles
- Documentation reorganized and completed for all 8 advanced orchestration modules
- ACP helpers/ reorganized into 7 domain subdirectories with #[path] backward compatibility

### Fixed
- unreachable!() production panic risk in prelude.rs replaced with graceful warn!() + fallback
- SessionCompressor now wired into SessionContextManager for semantic compression
- SseBufferPool now used for zero-allocation SSE event serialization
- CacheWarmingEngine initialized from main.rs, warmed after server completion
- planner_embedding classifier integrated into Planner::plan() main path
- RBAC tenant isolation tests fixed for i18n key compatibility

### Added
- 43 smoke tests for 6 ACP helper modules + 3 orchestration modules
- Full F-GAP label coverage for all 90+ #[allow(dead_code)] annotations
- local now includes sub-bus-memory and sub-bus-protocol for 14-bus completeness
## [1.0.0] - 2026-05-25

### Added
- ACP/MCP dual-protocol support with 5 transport modes
- 35+ AI provider integrations with native function calling
- 14-Bus architecture with 21 F-GAP cognitive modules
- Full-auto skill discovery and task execution
- Native function calling for OpenAI, Anthropic, DeepSeek
- Multi-model concurrent voting (Majority/Weighted/Unanimous/BestOfN)
- Transaction system with WAL persistence and 2PC
- Session context management with key concept extraction
- Cache warming with adaptive TTL and multi-tier management
- Chaos testing framework with 10 fault types
- Hot-reload configuration system
- Config schema versioning and migration
- Plugin system with Plugin trait and PluginRegistry
- Skill marketplace with install/uninstall/search
- SSE streaming optimizer with adaptive batching

### Changed
- Refactored mode runtimes to eliminate 5x code duplication
- Migrated from global OnceLock singletons to OrchestrationContext
- Upgraded recovery strategy matching from Levenshtein to explicit enums
- Optimized scheduler dequeue from O(n log n) to O(log n)
- Integrated BrainLoop into full-auto execution flow
- Fixed Gemini function call streaming parser
- Cleaned up deprecated model entries across all providers
- Enhanced Groq provider with tool_choice defaults and tests

### Fixed
- BrainLoop off-by-one iteration limit bug
- DAG executor was parallel fan-out (now real topological ordering)
- Dead code modules hot_reload/schema_version (now integrated)
