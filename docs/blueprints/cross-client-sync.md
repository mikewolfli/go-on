# Cross-Client State Synchronization

> Status: ✅ FULLY IMPLEMENTED
> Last Updated: 2026-06-09
> See also: `docs/blueprints/blue67.md` §2.5 (E1, E2)

## Architecture

The cross-client state sync system provides real-time push notification of
backend state changes to all connected clients (GUI, VSCode Addon, CLI, etc.)
using Server-Sent Events (SSE).

```
┌─────────────────┐     SSE /v1/state/events     ┌────────────────┐
│                 │ ◄─────────────────────────── │   GUI Client   │
│                 │                               │  (start_state_ │
│    Backend      │                               │   sync_listener│
│  (state_sync.rs)│                               │   → UI refresh)│
│                 │     SSE /v1/state/events     └────────────────┘
│  publish_event()│ ◄─────────────────────────── ┌────────────────┐
│  subscribe()    │                               │ VSCode Addon   │
│  broadcaster    │                               │ (startStateSync│
│                 │                               │  Listener →    │
│                 │                               │  status bar)   │
└─────────────────┘                               └────────────────┘
```

## Event Types

| Event | Payload | Trigger | Client Action |
|-------|---------|---------|---------------|
| `config_reloaded` | `{changed_keys: string[]}` | Backend hot-reload detects config change | GUI: refresh backend data; VSCode: status bar notification |
| `models_changed` | `{models: string[]}` | Provider list changes | GUI: refresh providers; VSCode: status bar notification |
| `agents_changed` | `{added: string[], removed: string[]}` | Agent lifecycle change | GUI: refresh agents; VSCode: status bar notification |
| `backend_restarting` | `{reason: string, restart_in_ms: number}` | Backend graceful shutdown | GUI: force health re-check; VSCode: warning notification |
| `heartbeat` | `{timestamp: number}` | Periodic (every 30s) | No UI action (connection keepalive) |

## Implementation

### Backend (`src/protocol/state_sync.rs`)

- `StateSyncEvent` enum — all event types with serde round-trip
- `BROADCASTER` — global `tokio::sync::broadcast` channel
- `publish_event(event)` — publish event to all subscribers
- `subscribe()` — get a receiver for the broadcast channel
- SSE endpoint: `GET /v1/state/events` in `src/acp/impl/runtime/http.rs`
  (`handle_state_events_sse`)

### GUI (`gui/src/backend.rs`)

- `StateSyncEvent` — mirror of backend enum
- `start_state_sync_listener(base_url, event_tx)` — spawns tokio task
  that connects to `/v1/state/events`, parses SSE frames, forwards events
  to `mpsc::Sender<StateSyncEvent>`
- Wired in `GoOnApp::new()` — receiver polled each frame in `update()`
  via `poll_state_sync_events()` method
- Actions: `ConfigReloaded`/`ModelsChanged` → force backend refresh;
  `BackendRestarting` → force health re-check

### VSCode Addon (`vscode-addon/src/stateSync.ts`)

- `StateSyncEvent` — TypeScript mirror type
- `startStateSyncListener(baseUrl, callbacks, outputChannel?)` — connects
  using `fetch()` with `ReadableStream`, dispatches events to callbacks
- Wired in `extension.ts` `activate()` — callbacks update status bar
  messages, log to output channel
- Disposable returned for cleanup via `context.subscriptions`

## End-to-End Verification

### Test: Config Hot-Reload Propagation

1. Start backend with `config.toml`
2. Open GUI — verify `[state-sync] Config reloaded` in console
3. Open VSCode — verify status bar shows config reloaded message
4. Modify `config.toml` while backend is running
5. Verify:
   - GUI console shows `[state-sync] Config reloaded`
   - VSCode output channel shows `[state-sync] Config reloaded`
   - VSCode status bar shows notification

### Test: Models Changed Propagation

1. Add a new provider via GUI → backend restarts
2. Verify GUI console shows `[state-sync] Models changed`
3. Verify VSCode status bar shows models updated notification

## Maintenance

- Add new event types by extending `StateSyncEvent` enum in both
  `src/protocol/state_sync.rs` and client mirrors
- Ensure SSE parsing in both clients handles the new variant
- Update this document with new event type, trigger, and client action

## Related Documents

- `docs/blueprints/blue67.md` — BLUE67 tracking: Item E1 (SSE→GUI), E2 (SSE→VSCode)
- `src/protocol/state_sync.rs` — Backend implementation
- `gui/src/backend.rs` (lines 1616-1751) — GUI client implementation
- `vscode-addon/src/stateSync.ts` — VSCode client implementation
