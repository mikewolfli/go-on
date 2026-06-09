/**
 * State Sync Listener for Go-On VSCode Extension.
 *
 * Connects to the backend's `/v1/state/events` SSE endpoint and
 * dispatches events to registered callbacks.
 *
 * Architecture (see contracts/cross-client-sync.md):
 *   Backend → SSE stream → stateSync.ts → callbacks → UI updates
 */

import * as vscode from "vscode";

/** Mirror of the backend's `StateSyncEvent` (see src/protocol/state_sync.rs). */
export type StateSyncEvent =
  | { type: "models_changed"; models: string[] }
  | { type: "config_reloaded"; changed_keys: string[] }
  | { type: "agents_changed"; added: string[]; removed: string[] }
  | { type: "backend_restarting"; reason: string; restart_in_ms: number }
  | { type: "heartbeat"; timestamp: number };

/** Callbacks for each event type. */
export interface StateSyncCallbacks {
  onModelsChanged?: (models: string[]) => void;
  onConfigReloaded?: (changedKeys: string[]) => void;
  onAgentsChanged?: (added: string[], removed: string[]) => void;
  onBackendRestarting?: (reason: string, restartInMs: number) => void;
  onHeartbeat?: (timestamp: number) => void;
}

/** Human-readable summary of a state sync event. */
export function stateSyncEventSummary(event: StateSyncEvent): string {
  switch (event.type) {
    case "models_changed":
      return `Models changed (${event.models.length} models)`;
    case "config_reloaded":
      return event.changed_keys.length > 0
        ? `Config reloaded: ${event.changed_keys.join(", ")}`
        : "Config reloaded";
    case "agents_changed": {
      const parts: string[] = [];
      if (event.added.length > 0) parts.push(`+${event.added.length} agents`);
      if (event.removed.length > 0) parts.push(`-${event.removed.length} agents`);
      return `Agents changed (${parts.join(", ")})`;
    }
    case "backend_restarting":
      return `Backend restarting: ${event.reason}`;
    case "heartbeat":
      return "heartbeat";
  }
}

/**
 * Start listening for state sync events from the backend.
 * Returns an abort function to stop listening.
 */
export function startStateSyncListener(
  baseUrl: string,
  callbacks: StateSyncCallbacks,
  outputChannel?: vscode.OutputChannel,
): () => void {
  let aborted = false;
  const url = `${baseUrl.replace(/\/+$/, "")}/v1/state/events`;

  const log = (msg: string) => {
    if (outputChannel) {
      outputChannel.appendLine(`[state-sync] ${msg}`);
    }
  };

  async function connect() {
    while (!aborted) {
      try {
        log(`connecting to ${url}...`);
        const response = await fetch(url);
        if (!response.ok || !response.body) {
          log(`connection failed: ${response.status}`);
          await sleep(5000);
          continue;
        }

        const reader = response.body.getReader();
        const decoder = new TextDecoder();
        let buffer = "";

        while (!aborted) {
          const { done, value } = await reader.read();
          if (done) break;

          buffer += decoder.decode(value, { stream: true });

          // Process complete SSE frames
          let idx: number;
          while ((idx = buffer.indexOf("\n\n")) !== -1) {
            const frame = buffer.slice(0, idx);
            buffer = buffer.slice(idx + 2);
            processFrame(frame, callbacks, log);
          }
        }

        log("stream ended, reconnecting...");
        if (!aborted) await sleep(5000);
      } catch (err) {
        log(`error: ${err}, reconnecting in 10s...`);
        if (!aborted) await sleep(10000);
      }
    }
  }

  connect();

  return () => {
    aborted = true;
  };
}

function processFrame(
  frame: string,
  callbacks: StateSyncCallbacks,
  log: (msg: string) => void,
): void {
  let eventType = "";
  let dataStr = "";

  for (const line of frame.split("\n")) {
    if (line.startsWith("event: ")) {
      eventType = line.slice(7).trim();
    } else if (line.startsWith("data: ")) {
      dataStr = line.slice(6).trim();
    }
  }

  if (!dataStr || dataStr === "[DONE]") return;

  try {
    const event = JSON.parse(dataStr) as StateSyncEvent;
    log(`received: ${stateSyncEventSummary(event)}`);

    switch (event.type) {
      case "models_changed":
        callbacks.onModelsChanged?.(event.models);
        break;
      case "config_reloaded":
        callbacks.onConfigReloaded?.(event.changed_keys);
        break;
      case "agents_changed":
        callbacks.onAgentsChanged?.(event.added, event.removed);
        break;
      case "backend_restarting":
        callbacks.onBackendRestarting?.(event.reason, event.restart_in_ms);
        break;
      case "heartbeat":
        callbacks.onHeartbeat?.(event.timestamp);
        break;
    }
  } catch (err) {
    log(`parse error: ${err}`);
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
