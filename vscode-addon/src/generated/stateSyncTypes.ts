// AUTO-GENERATED from contracts/state-sync-events.json — do not edit.
// Regenerate with: python3 scripts/gen-state-sync-types.py

/**
 * Mirror of the backend's `StateSyncEvent` (single source of truth:
 * contracts/state-sync-events.json).
 */
export type StateSyncEvent =
  | { type: "models_changed"; models: string[] }
  | { type: "config_reloaded"; changed_keys: string[] }
  | { type: "agents_changed"; added: string[]; removed: string[] }
  | { type: "backend_restarting"; reason: string; restart_in_ms: number }
  | { type: "heartbeat"; timestamp: number };
