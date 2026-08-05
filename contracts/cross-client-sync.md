# Cross-Client State Sync

**Status:** Implemented (v2 — real backend broadcaster + SSE endpoint)
**Last updated:** 2026-06-09
**Tracking:** Three-end cross-client state sync (GUI ↔ VSCode ↔ Backend)

## Overview

Go-On has three clients that each maintain overlapping state:

1. **GUI** (`gui/`) — egui-based desktop app with `ConfigStore`, provider list, chat session
2. **VSCode Addon** (`vscode-addon/`) — VS Code extension with `GoOnManager`, `ConfigManager`
3. **Backend** (`src/`) — Rust ACP/HTTP server with its own config and runtime state

This document defines the unified reconnect strategy shared by all clients and the state sync mechanism between them.

---

## 1. Unified Reconnect Strategy

Both the GUI and VSCode addon use an **identical** reconnect strategy to ensure symmetric recovery behavior.

### Parameters

| Parameter | Value |
|-----------|-------|
| Max retries | Unlimited |
| Base interval | 1,000 ms |
| Growth factor | 2× (exponential) |
| Max interval | 30,000 ms (30 s) |
| Jitter | ±30% (random multiplier in [0.7, 1.0]) |
| Reset condition | On successful reconnect |

### Backoff Sequence (before jitter)

| Attempt | Base delay | With jitter (typical range) |
|---------|-----------|----------------------------|
| 0       | 1,000 ms  | 700–1,000 ms |
| 1       | 2,000 ms  | 1,400–2,000 ms |
| 2       | 4,000 ms  | 2,800–4,000 ms |
| 3       | 8,000 ms  | 5,600–8,000 ms |
| 4       | 16,000 ms | 11,200–16,000 ms |
| 5       | 30,000 ms | 21,000–30,000 ms |
| 6+      | 30,000 ms | 21,000–30,000 ms |

### Formula

```
delay_ms = min(1000 × 2^attempt, 30000) × (0.7 + random() × 0.3)
```

### Reset Behavior

When a reconnect attempt succeeds, the attempt counter resets to 0 so that the next disconnection starts from the 1 s base interval.

### Platform Implementations

| Platform | File | Key component | Notes |
|----------|------|--------------|-------|
| GUI (Rust) | `gui/src/backend/rpc.rs` | `retry_backoff()`, `QUICK_RPC_ATTEMPTS`, `FULL_RPC_ATTEMPTS` | Used for RPC call retries to the backend. Bounded: 20 attempts ≈ 5 minutes before giving up (base formula shared via `gui/src/backoff.rs::exp_backoff_ms`). |
| VSCode (TypeScript) | `vscode-addon/src/runtime/reconnect.ts` | `ReconnectManager.backoffMs()`, `ReconnectManager.schedule()` | Used for restarting the backend process. Already had unlimited retries; updated cap from 300 s to 30 s and base from 2 s to 1 s. |
| Rust SDK | `sdk/rust/src/client.rs` | `GoOnClient::backoff_delay()` | RPC retries with the same formula (`min(base*2^n, 30s) * (0.7 + random*0.3)`). |
| Node SDK | `sdk/nodejs/src/client.ts` | `GoOnClient._retryDelayForAttempt()` | Same formula (ms). |
| Python SDK | `sdk/python/go_on_sdk/client.py` | `GoOnClient._retry_delay_for_attempt()` | Same formula (seconds). |
| TypeScript SDK | `sdk/typescript/src/client.ts` | `GoOnClient.delay()` | Same formula (ms). |

### Remaining Platform Differences

- **GUI** uses the unified backoff for **RPC-level retries** (`rpc_call_internal` loop), while **VSCode** uses it for **process-level reconnection** (restarting the backend binary). This is because the GUI is a separate process communicating via HTTP, while the VSCode addon spawns the backend as a child process.
- **GUI** logs backoff delays to stderr (`eprintln!`); **VSCode** logs to its `OutputChannel`.
- The underlying timers differ: GUI uses `tokio::time::sleep`, VSCode uses `setTimeout`.

These differences are inherent to the platform and do not affect the observable reconnect behavior.

---

## 2. State Sync Mechanism

### Backend as Source of Truth

The backend process is the central coordination point:

- Exposes a **real-time SSE endpoint** (`GET /v1/state/events`) for streaming state changes
- Clients subscribe on startup and receive push updates immediately
- A `StateSyncBroadcaster` (tokio broadcast channel) fans out events to all connected clients

### State Sync Events

All events use server-sent events (SSE) with `event: state_sync` and a JSON body containing:

| Event type | Payload | Trigger |
|------------|---------|--------|
| `models_changed` | `{ models: string[] }` | Model list updated |
| `config_reloaded` | `{ changed_keys: string[] }` | Config file hot-reloaded |
| `agents_changed` | `{ added: string[], removed: string[] }` | Agent registry modified |
| `backend_restarting` | `{ reason: string, restart_in_ms: number }` | Backend about to restart |
| `heartbeat` | `{ timestamp: number }` | Periodic keep-alive (30s) |

**Single source of truth**: `contracts/state-sync-events.json`. The VSCode
TypeScript union (`vscode-addon/src/generated/stateSyncTypes.ts`) is generated
from it; `scripts/gen-state-sync-types.py` also verifies the backend
(`src/protocol/state_sync.rs`) and GUI (`gui/src/state_sync.rs`) Rust enums stay
in sync. Regenerate after any event change:

```
python3 scripts/gen-state-sync-types.py
```

### Sync REST Endpoints

```
GET    /v1/state/events     → SSE stream of state change events (real-time)
```

### Client Integration

| Client | Subscribe | Events Handled |
|--------|-----------|----------------|
| GUI | `start_state_sync_listener()` in `gui/src/backend.rs` | `ConfigReloaded` → notification, `ModelsChanged` → refresh cache |
| VSCode | `startStateSyncListener()` in `vscode-addon/src/stateSync.ts` | `ConfigReloaded` → status bar message, `ModelsChanged` → refresh |

### Implementation Files

| File | Purpose |
|------|---------|
| `src/protocol/state_sync.rs` | `StateSyncEvent` enum + global `StateSyncBroadcaster` |
| `src/acp/impl/runtime/http.rs` | SSE endpoint `/v1/state/events` |
| `src/core/config/hot_reload.rs` | Publishes `ConfigReloaded` on successful hot-reload |
| `gui/src/backend.rs` | GUI state sync listener (`start_state_sync_listener`) |
| `vscode-addon/src/stateSync.ts` | VSCode state sync listener (`startStateSyncListener`) |
| `contracts/cross-client-sync.md` | This document |

### Out of Scope (Current)

- Real-time collaborative editing of config
- Session sharing between GUI and VSCode
- Bi-directional agent state synchronization
- Pull-based state snapshot endpoint (`GET /v1/sync/state`) — SSE push is sufficient

---

## 3. Files

| File | Purpose |
|------|---------|
| `src/protocol/state_sync.rs` | `StateSyncEvent` enum + global `StateSyncBroadcaster` |
| `src/acp/impl/runtime/http.rs` | SSE endpoint `/v1/state/events` |
| `src/core/config/hot_reload.rs` | Publishes `ConfigReloaded` on successful hot-reload |
| `gui/src/backend/rpc.rs` | GUI RPC client with unified backoff (`retry_backoff`) |
| `vscode-addon/src/stateSync.ts` | VSCode state sync listener |
| `vscode-addon/src/runtime/reconnect.ts` | VSCode reconnection manager with unified backoff |
| `vscode-addon/src/runtimeManager.ts` | VSCode runtime manager that drives reconnection |
| `contracts/cross-client-sync.md` | This document |
