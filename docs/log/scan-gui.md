# GUI Source Scan Report

**Date:** 2026-07-02  
**Scope:** `go-on/gui/src` (all `.rs` files)  
**Focus:** Dead code, unnecessary allocations, redundant patterns, misleading abstractions

---

## 1. Dead Code — Explicit `#[allow(dead_code)]`

### `backend/mod.rs:233–237` — `get_chat_endpoint()`

```rust
#[allow(dead_code)]
pub async fn get_chat_endpoint(&self) -> String {
    self.chat_endpoint.read().await.clone()
}
```

This is the only `#[allow(dead_code)]` in the entire GUI crate. The doc comment says _"Public API for external SDK consumers — not called within this binary crate."_ It is dead code within this binary — never referenced from any other module. It should either be removed or gated behind a cargo feature flag if external consumers exist.

---

## 2. Write-Only / No-Op Abstractions

### `views/providers/mod.rs:24–26` + `render.rs:244–248,345–349` — `COPILOT_AUTH_CACHE`

```rust
thread_local! {
    static COPILOT_AUTH_CACHE: RefCell<CachedView> = RefCell::new(CachedView::new());
}
```

At `render.rs:244–248` the value is **read and immediately discarded**:

```rust
let _ = COPILOT_AUTH_CACHE.with(|c| {
    c.borrow().check_size("copilot_auth", copilot_hash)
});
```

At `render.rs:345–349` the value is stored but never read back meaningfully. The entire cache is a **write-only sink** — always allocates to store, never read. Can be removed entirely.

### `widgets/cache.rs:51–63` — `CachedView::check_or_render()`

```rust
pub fn check_or_render(
    &mut self,
    ui: &mut egui::Ui,
    key: &str,
    hash: u64,
    render_fn: impl FnOnce(&mut egui::Ui),
) -> egui::Vec2 {
    let cache_key = (key.to_owned(), hash);
    let resp = egui::Frame::NONE.show(ui, render_fn);
    let size = resp.response.rect.size();
    self.cache.insert(cache_key, size);
    size
}
```

**Always renders** — there is no early return on cache hit. The "check" part is a no-op. The method always calls `render_fn`, then stores the resulting size. Despite the name, this functions as an unconditional `render_and_store_size`. Every caller in the codebase passes `hash = 0_u64`, making the hash dimension entirely useless.

Called in:
- `views/about.rs:28` — `hash = 0`
- `views/autotune.rs:79` — `hash = 0`
- `views/config_editor.rs:137` — `hash = 0`
- `views/security.rs:57` — `hash = 0`

Since the hash is always `0` and keys are static strings, the cache accumulates one entry per unique key string and never invalidates. This isn't harmful (entries are bounded by the number of views) but the `check_` prefix is misleading and the hash parameter is dead weight.

---

## 3. Unnecessary Clones / Allocations

### `config.rs:470–476` — Full `AppConfig` clone before TOML serialization

```rust
let mut config_for_save = config.clone();
for provider in &mut config_for_save.providers {
    provider.api_key.clear();
    provider.secret_key.clear();
}
```

`AppConfig` is a non-trivial struct containing provider lists, feature toggles, and enterprise config. Cloning it just to clear two fields per provider is wasteful. A better approach would be to serialize directly while skipping those fields, or use a serialization wrapper.

### `config_store.rs:13–14` — Double buffering with `config_shared`

```rust
inner: Arc<RwLock<AppConfig>>,
config_shared: Arc<AppConfig>,
```

`sync_shared_if_needed()` clones the entire `AppConfig` on each detected change to update the shared snapshot. The fingerprint comparison mitigates this on unchanged frames, but when the config does change (e.g. provider toggle), the entire provider list and nested structs are deep-copied. Consider `Arc::make_mut` or a delta-based approach if this shows up in profiles.

---

## 4. Misleading / Redundant Code

### `views/mod.rs:8–11` — `send_with_retry()` — no retry

```rust
pub(crate) fn send_with_retry(tx: &mpsc::SyncSender<String>, msg: String) {
    if tx.try_send(msg).is_err() {
        eprintln!("WARN: channel full — message dropped");
    }
}
```

Named `_with_retry` but performs a single `try_send` with no retry loop. The comment says this is intentional (no `thread::sleep` on UI thread), but the name is misleading. Rename to `send_nonblocking` or `try_send_msg`.

### `backend/rpc.rs:10–11` — Retry attempts set to 1000

```rust
pub(super) const QUICK_RPC_ATTEMPTS: usize = 1000;
pub(super) const FULL_RPC_ATTEMPTS: usize = 1000;
```

Exponential backoff capped at 30s. With 1000 max attempts, the theoretical maximum retry duration is ~8 hours (1000 × 30s). This is almost certainly not the intended behavior — if the backend is down for 8 hours, no amount of retrying will help. A value of 10–20 attempts (capped at ~5 minutes) would be more reasonable.

### `config_store.rs:49–103` — Manual field-by-field hashing

`config_fingerprint()` manually calls `.hash()` on every individual field of `AppConfig`. This could be replaced with `#[derive(Hash)]` on the relevant structs, or at minimum with a macro to avoid the repetition. The current approach is fragile — adding or removing a field requires updating this method.

### `app/mod.rs:28–39` — `log_msg()` is a release-build no-op

```rust
pub fn log_msg(msg: &str) {
    #[cfg(debug_assertions)]
    { /* write to temp dir */ }
    #[cfg(not(debug_assertions))]
    let _ = msg;
}
```

In release builds this compiles to a function that takes `msg` and discards it. Called from `app/mod.rs:674` inside a frame timing diagnostic block. This is not harmful — the optimizer likely removes the call entirely — but it's worth noting.

---

## 5. Compile-Time Feature Flags That Should Be Runtime Config

### `views/chat/chat_impl.rs:14–17`

```rust
const CHAT_DISABLE_MARKDOWN_RENDER: bool = false;
const CHAT_STAGE6_ENABLE_MODE_ROW: bool = true;
const CHAT_STAGE6_ENABLE_EXTRA_BUTTONS: bool = true;
const MAX_CONCURRENT_GENERATIONS: usize = 4;
```

These feature flags require a rebuild to change. `CHAT_DISABLE_MARKDOWN_RENDER` gates markdown rendering entirely; `CHAT_STAGE6_ENABLE_MODE_ROW` and `CHAT_STAGE6_ENABLE_EXTRA_BUTTONS` gate UI elements. These are the kind of things users might want to toggle at runtime. The `MAX_CONCURRENT_GENERATIONS` constant (4) controls how many concurrent AI generations can run — also a candidate for runtime configuration.

---

## 6. Minor / Cosmetic Issues

| Path | Line(s) | Issue |
|------|---------|-------|
| `views/config_editor.rs:150` | `150` | `.matches(&self.search_query).count()` — O(n) scan on every keystroke over the full config JSON. Fine for typical <10KB configs, but worth noting. |
| `config.rs:421` | `421` | `CONFIG_SAVE_DEBOUNCE: LazyLock<Mutex<Option<Instant>>>` — A `Mutex` on every config save. The lock duration is very short (~microseconds), but since this is called on the UI thread, an `AtomicU64` timestamp would be lock-free. |
| `views/security.rs:112–125` | `112–125` | Restart timeout is 10s with error message "timeout" if exceeded. This error bubbles to the UI as `"security.restartFailed: timeout"`. The restart timeout error string should be more descriptive. |
| `keyring_util.rs:370–387` | `370–387` | `provider_to_env_name()` handles "copilot" and "github" as special cases. If both "copilot" and "github" providers are configured, they map to the same env var `GITHUB_COPILOT_TOKEN`, which could cause conflicts. |

---

## Summary

| Category | Count | Severity |
|----------|-------|----------|
| Dead code (explicit `#[allow]`) | 1 | Low |
| Write-only / no-op abstractions | 2 | Medium |
| Unnecessary clones / allocations | 2 | Low–Medium |
| Misleading names / redundant code | 4 | Low |
| Compile-time flags → runtime | 1 | Low |
| Minor / cosmetic | 5 | Very Low |

**Total meaningful issues found: 10**

### Top action items

1. **Remove `COPILOT_AUTH_CACHE`** — write-only cache, ~2 lines of deletion + the static
2. **Remove or feature-gate `get_chat_endpoint()`** — the only `#[allow(dead_code)]` in the crate
3. **Rename `send_with_retry()`** to reflect its non-retrying nature, or add retry logic
4. **Reduce `QUICK_RPC_ATTEMPTS` / `FULL_RPC_ATTEMPTS`** from 1000 to 10–20
5. **Avoid cloning `AppConfig` for serialization** — serialize with `#[serde(skip)]` instead
