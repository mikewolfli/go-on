# Changelog

## [1.4.1] - 2026-07-21

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

## [Unreleased]

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
