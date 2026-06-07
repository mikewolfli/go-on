# Cross-Client State Sync

> **TODO: Implementation design doc**
>
> **Status:** Draft / Planned
> **Priority:** Low (T6)
> **Tracking:** Three-end cross-client state sync (GUI ↔ VSCode ↔ Backend)

## Problem

Go-On has three clients that each maintain overlapping state:

1. **GUI** (`gui/`) — egui-based desktop app with `ConfigStore`, provider list, chat session
2. **VSCode Addon** (`vscode-addon/`) — VS Code extension with `GoOnManager`, `ConfigManager`
3. **Backend** (`src/`) — Rust ACP/HTTP server with its own config and runtime state

Currently there is no cross-client state synchronisation. Changes made in one client
(e.g. adding a provider in the GUI) do not propagate to the VSCode addon or vice versa.
Each client is independently configured and must be restarted to pick up external changes.

## Proposed Mechanism

### 1. Backend as Source of Truth

The backend process should be the central coordination point:

- Expose a WebSocket or SSE endpoint for state change notifications
- Clients subscribe on startup and receive incremental updates
- The backend broadcasts state changes (provider CRUD, config changes, phase transitions)

### 2. Sync REST Endpoints

```
POST   /api/v1/sync/state     → Publish local state changes to backend
GET    /api/v1/sync/state     → Pull full state snapshot from backend
GET    /api/v1/sync/events    → SSE stream of state change events
```

### 3. Conflict Resolution

- **Last-writer-wins** for scalar config fields (theme, language, protocol mode)
- **Merge by key** for provider lists (add/update by provider name, remove by flag)
- **Reject stale writes** using a monotonic `state_version` counter per section

### 4. Client Integration

| Client     | Subscribe | Publish      | Reconnect            |
|------------|-----------|--------------|----------------------|
| GUI        | On startup | After config change | Exponential backoff |
| VSCode     | On startup | After config change | Exponential backoff |
| CLI        | N/A       | N/A          | N/A                  |

### 5. MVP Scope (First Cut)

- `/api/v1/sync/state` GET/POST with JSON body
- `state_version` field on the backend config
- GUI: post config changes, poll for external changes every 30s
- VSCode: post config changes, poll every 30s
- No SSE in MVP — long-polling is sufficient for low-frequency state changes

## Files to Create/Modify

- `src/core/sync/` — new module for sync endpoints
- `gui/src/sync/` — sync client for GUI
- `vscode-addon/src/sync/` — sync client for VSCode
- `contracts/state-sync-schema.json` — JSON Schema for the sync payload

## Out of Scope (for now)

- Real-time collaborative editing of config
- Session sharing between GUI and VSCode
- Bi-directional agent state synchronisation
