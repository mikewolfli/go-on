# Tool Pipeline Scan Report

Date: 2026-06-26  
Scope: `go-on/src/orchestration/tool/` + `go-on/src/orchestration/skill/` + call sites  
Severity legend: **CRITICAL** / HIGH / MEDIUM / LOW

---

## 1. CRITICAL — Synchronous `execute_loop` called from async contexts (blocks tokio worker threads)

### 1a. `acp/impl/chat.rs` — line 1094

`execute_loop` (which is entirely synchronous) is called directly inside an `async fn`:

```rust
pub(crate) async fn run_full_auto_execution(/* ... */) {
    // ...
    let (tao_decision, tao_trace) = execute_loop(
        &task_description,
        &tool_registry,
        &tool_input,
        &preferred_tools,
        &tao_config,
    );
```

This blocks the **tokio worker thread** for the entire duration of the loop (default max_iterations=10, each tool potentially running for seconds). If the tool calls hit blocking I/O (e.g. `shell_exec`, `http_request`), the worker thread is stalled.

- Fix: call `execute_loop_async` instead, or wrap in `tokio::task::spawn_blocking`.

### 1b. `orchestration/dag_driver.rs` — line 316-349

`create_tool_jobs` spawns `tokio::spawn(async move { ... })` tasks that call `execute_loop`:

```rust
tokio::spawn(async move {
    let (decision, _trace) = execute_loop(&tool_name, &registry, &input, &pref_tools, &cfg);
    // ...
})
```

Every DAG-parallel tool execution blocks a worker thread.

- Fix: use `tokio::task::spawn_blocking` or call `execute_loop_async`.

### 1c. `acp/helpers/autonomy/autonomy_loop.rs` — line 860

Same pattern inside `tokio::spawn`:

```rust
tokio::spawn(async move {
    let (decision, _trace) = execute_loop(&tool_name, &registry, &tool_input, &[], &loop_cfg);
})
```

These are collected into a `join_all`, so all concurrent tool calls block distinct worker threads simultaneously.

---

## 2. CRITICAL — `pipeline.rs::run_single_tool` is async but calls sync `run_with_fallback`

`pipeline.rs` line 320-390:

```rust
async fn run_single_tool(/* ... */) -> PipelineStepResult {
    // ...
    let output = match registry.run_with_fallback(tool_name, &tool_input) {  // ← sync call
```

Even though the method signature is `async fn`, the actual tool dispatch goes through the **synchronous** `run_with_fallback` (which calls `tool.run()` directly). This means:

- The method signature is misleading — it's not truly async.
- Any long-running tool blocks the tokio worker.
- The `run_with_fallback_async` method exists but is never used here.

Fix: call `registry.run_with_fallback_async(tool_name, &tool_input).await` instead.

---

## 3. HIGH — `tool_bus.rs::dispatch_tool` is async but calls sync `run_with_fallback`

`intelligence/capability_bus/tool_bus.rs` line 340-350:

```rust
async fn dispatch_tool(&self, tool_name: &str, input: &ToolInput) -> Result<ToolOutput> {
    let reg = self.tool_registry.lock().unwrap_or_else(|poisoned| { /* ... */ });
    if reg.get(tool_name).is_some() {
        return reg.run_with_fallback(tool_name, input);  // ← sync, blocking
    }
```

The comment says "Lock scope: dropped before any await point" but the lock is dropped *because the function returns immediately*. However `run_with_fallback` is blocking — if the tool takes 30 seconds, this async function blocks for 30 seconds.

Fix: use `run_with_fallback_async` with `get_arc` outside the lock, and drop the lock before `.await`.

---

## 4. MEDIUM — Pipeline uses flat `run_with_fallback` (no alias-aware dispatch)

Although `ToolRegistry::get()` resolves aliases (e.g. `"terminal"` → `"shell_exec"`), `pipeline.rs::run_single_tool` calls `registry.run_with_fallback(tool_name, &tool_input)` which internally calls `self.get(name)`. This *does* resolve aliases, so this is not a bug, but it's worth noting that the `run_with_fallback` method:

- Does **not** call `get_arc` (for potential async dispatch).
- Uses `get` (returns `&dyn Tool`), then calls `tool.run()` — always synchronous.

This means the pipeline path never exercises the `run_async` vtable method.

---

## 5. MEDIUM — `lock.rs` — `acquire()` and `acquire_async()` are `#[cfg(test)]` only

`go-on/src/orchestration/tool/lock.rs`:

- `pub fn acquire()` — test-only (line 157: `#[cfg(test)]`)
- `pub async fn acquire_async()` — test-only (line 200: `#[cfg(test)]`)

In production, only `try_acquire()` is available, which is **non-blocking and returns `None` if the lock would block**. This means:

- Production code never waits for a lock; if contention exists, the tool proceeds unlocked.
- This is intentional per the comment (`"Production code should use try_acquire() which is non-blocking"`) but it means the lock manager is effectively a **best-effort advisory lock** in production.

**No production code currently calls `acquire()` or `acquire_async()`** — the lock is used only in tests. Production tools (`WriteFileTool`, `ApplyPatchTool`) do not appear to acquire locks at all based on the code examined.

---

## 6. MEDIUM — `ImageGenerateTool` has unreasonably low timeout budget

Registered with `timeout_budget_ms: 5_000` (line 748 in `mod.rs`):

```rust
timeout_budget_ms: 5_000,
```

But `ImageGenerateTool::run()` (in `image.rs` line 324-492):
- Generates pixel buffers in memory
- Validates output path
- Creates parent directories
- Saves to disk in various formats
- Has no internal timeout or cancellation

For large images (e.g. 4096×4096 in WebP), the tool can easily take >5 seconds. The `run()` implementation is synchronous and runs on the tokio blocking pool via the default `run_async`, but the 5_000ms budget is tight.

---

## 7. LOW — `think()` ignores the `_task` parameter

`mod.rs` line 2900-2928:

```rust
fn think(
    _task: &str,    // ← unused
    candidates: &[String],
    retry_counts: &HashMap<String, u32>,
    config: &LoopConfig,
) -> Option<ThinkResult> {
    let best = candidates
        .iter()
        .filter(|t| retry_counts.get(*t).copied().unwrap_or(0) < config.max_retries_per_tool)
        .min_by_key(|t| retry_counts.get(*t).copied().unwrap_or(0))?;
```

The think phase is **not AI-driven** — it's a simple round-robin over candidates sorted by retry count. The `_task` parameter is a vestigial API. This is not a bug (the system works), but it means the "Think" phase doesn't actually reason about the task.

---

## 8. LOW — `SkillExecuteTool::run()` creates a second tokio runtime

`mod.rs` line 2196-2205:

```rust
static SKILL_RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();

fn skill_runtime() -> &'static tokio::runtime::Runtime {
    SKILL_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build shared skill runtime")
    })
}
```

`run()` then does:

```rust
fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
    let input = input.clone();
    let rt = skill_runtime();
    rt.block_on(skill_execute_arc().run_async(input))
}
```

This works (bridges sync→async without `block_in_place`), but it's worth auditing whether the dedicated per-thread runtime could lead to thread starvation under heavy concurrent `SkillExecuteTool` usage. Currently no production caller uses `run()` on `SkillExecuteTool` — the async path is preferred.

---

## 9. LOW — Some pipeline action mappings reference unregistered tool names

The `pipeline_tool_to_action` function (line 132-237 in `pipeline.rs`) has a comprehensive mapping, but many listed tool names are **unconditionally** compiled (not feature-gated), while the actual tools may not exist if their features are disabled. For example:

- `"stl_generate"` — requires `feature = "cad-stl"`
- `"svg_read"`, `"svg_export"`, `"svg_generate"` — require `feature = "drawing-svg"`
- `"dxf_read"` — requires `feature = "cad-dxf"`
- `"stl_read"` — requires `feature = "cad-stl"`
- `"obj_read"` — requires `feature = "cad-obj"`
- `"step_read"` — requires `feature = "cad-step"`
- `"ply_read"` — requires `feature = "cad-ply"`
- `"iges_read"` — requires `feature = "cad-iges"`
- `"gltf_read"` — requires `feature = "cad-gltf"`
- `"geo_util"` — requires `feature = "cad-geo"`
- `"gcode_read"` — requires `feature = "cam-gcode"`
- `"gpx_read"` — requires `feature = "gis-gpx"`
- `"obj_model_read"` — requires `feature = "model-3d-extra"`
- `"cad_convert"` — requires `feature = "cad-utils"`
- `"image_analyze"`, `"image_generate"`, `"image_resize"`, `"image_convert"` — require `feature = "image-processing"`
- `"sqlite_query"` — requires `feature = "backend-sqlite"`
- `"qrcode_generate"` — requires `feature = "barcode-tools"`

When the feature is disabled, the tool won't be registered, the registry will return `None`, and the pipeline step will fail with "tool not found". The sandbox check passes (because the mapping is unconditional), but execution fails. This is mostly harmless (clean error message) but confusing.

---

## 10. NO ISSUE — No stub/no-op tool implementations found

The grep for `unimplemented!`, `todo!`, `panic!` (non-test), and `stub` returned **zero results** in the tool implementation code. Every extended tool in `go-on/src/orchestration/tool/extended/` has a concrete implementation:

| Tool | Status |
|------|--------|
| `ShellExecTool` | Full implementation with timeout, env, stdin |
| `HttpRequestTool` | Full implementation |
| `GrepTool` | Full implementation |
| `FindFilesTool` | Full implementation |
| `GitTool` | Full implementation |
| `ListDirectoryTool` | Full implementation |
| `CargoCheckTool` / `CargoTestTool` | Full implementation |
| `FileMoveTool` / `FileDeleteTool` | Full implementation |
| `ImageGenerateTool` | Full implementation (solid, checkerboard, gradient) |
| `ImageResizeTool` / `ImageConvertTool` / `ImageAnalyzeTool` | Full implementation |
| `BarcodeTool` (QrCodeTool) | Full implementation |
| All CAD tools | Full implementations |
| Game tools | Full implementations (40+ tools) |

---

## 11. Summary — All Issues

| # | Severity | File(s) | Issue |
|---|----------|---------|-------|
| 1a | **CRITICAL** | `acp/impl/chat.rs:1094` | Sync `execute_loop` called from async fn, blocking tokio worker |
| 1b | **CRITICAL** | `dag_driver.rs:349` | Sync `execute_loop` inside `tokio::spawn`, blocking worker threads |
| 1c | **CRITICAL** | `autonomy_loop.rs:860` | Sync `execute_loop` inside `tokio::spawn`, blocking worker threads |
| 2 | **CRITICAL** | `pipeline.rs:346` | `async fn run_single_tool` calls sync `run_with_fallback`, never uses async |
| 3 | **HIGH** | `tool_bus.rs:349` | `async fn dispatch_tool` calls sync `run_with_fallback`, blocking |
| 4 | MEDIUM | `pipeline.rs` | Pipeline never uses `run_async` vtable — always sync path |
| 5 | MEDIUM | `lock.rs` | `acquire()` / `acquire_async()` are test-only; production uses best-effort `try_acquire()`. No production code acquires locks at all. |
| 6 | MEDIUM | `mod.rs:748` | `ImageGenerateTool` timeout budget 5s is unreasonably tight |
| 7 | LOW | `mod.rs:2900` | `think()` ignores `_task`, no AI-driven tool selection |
| 8 | LOW | `mod.rs:2196` | `SkillExecuteTool::run()` creates dedicated tokio runtime per sync call |
| 9 | LOW | `pipeline.rs:132` | Pipeline action mapping lists feature-gated tool names unconditionally |

### Root cause pattern

The single largest issue is **synchronous tool dispatch in async contexts**. Of the 5 CRITICAL/HIGH findings, all 5 are cases where the synchronous `run_with_fallback()` or `execute_loop()` is called from an async function without `spawn_blocking`. The codebase has a proper `run_with_fallback_async` and `execute_loop_async` but they are almost never used by callers.
