/**
 * State Sync Listener for Go-On VSCode Extension.
 *
 * Connects to the backend's `/v1/state/events` SSE endpoint and
 * dispatches events to registered callbacks.
 *
 * Architecture (see contracts/cross-client-sync.md):
 *   Backend → SSE stream → stateSync.ts → callbacks → UI updates
 *
 * BLUE68 P5-4: SSE connections use AbortController with configurable
 * timeout and exponential backoff with full jitter for reconnection.
 */

import * as vscode from "vscode";

/** Default SSE connection timeout (in ms). */
const DEFAULT_SSE_TIMEOUT_MS = 15_000;

/** Maximum delay cap for exponential backoff (60 seconds). */
const MAX_BACKOFF_MS = 60_000;

/** Base delay for exponential backoff (1 second). */
const BASE_DELAY_MS = 1_000;

/** Mirror of the backend's `StateSyncEvent` (see src/protocol/state_sync.rs). */
export type StateSyncEvent =
  | { type: "models_changed"; models: string[] }
  | { type: "config_reloaded"; changed_keys: string[] }
  | { type: "agents_changed"; added: string[]; removed: string[] }
  | { type: "backend_restarting"; reason: string; restart_in_ms: number }
  | { type: "heartbeat"; timestamp: number };

/** Callbacks for each event type. */
export interface StateSyncCallbacks {
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  onModelsChanged?: (models: string[]) => void;
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  onConfigReloaded?: (changedKeys: string[]) => void;
  onAgentsChanged?: (added: string[], removed: string[]) => void;
  onBackendRestarting?: (reason: string, restartInMs: number) => void;
  onHeartbeat?: (timestamp: number) => void;
}

/** Human-readable summary of a state sync event. */
function stateSyncEventSummary(event: StateSyncEvent): string {
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
      if (event.removed.length > 0)
        parts.push(`-${event.removed.length} agents`);
      return `Agents changed (${parts.join(", ")})`;
    }
    case "backend_restarting":
      return `Backend restarting: ${event.reason}`;
    case "heartbeat":
      return "heartbeat";
  }
}

/**
 * Compute exponential backoff delay with full jitter.
 *
 * Implements AWS full-jitter strategy: delay = random(0, min(cap, base * 2^attempt))
 * This prevents thundering herd when multiple clients reconnect simultaneously.
 *
 * @param attempt - zero-based retry attempt number
 * @returns delay in milliseconds
 */
function backoffDelay(attempt: number): number {
  const exponential = BASE_DELAY_MS * Math.pow(2, Math.min(attempt, 6)); // cap exponent at 6 (64s)
  const capped = Math.min(exponential, MAX_BACKOFF_MS);
  // Full jitter: random between 0 and capped
  return Math.floor(Math.random() * capped);
}

/**
 * Start listening for state sync events from the backend.
 * Returns an abort function to stop listening.
 */
export function startStateSyncListener(
  baseUrl: string,
  callbacks: StateSyncCallbacks,
  outputChannel?: vscode.OutputChannel,
  sseTimeoutMs: number = DEFAULT_SSE_TIMEOUT_MS,
): () => void {
  let aborted = false;
  let retryAttempt = 0;
  const url = `${baseUrl.replace(/\/+$/, "")}/v1/state/events`;

  const log = (msg: string) => {
    if (outputChannel) {
      outputChannel.appendLine(`[state-sync] ${msg}`);
    }
  };

  async function connect() {
    while (!aborted) {
      let controller: AbortController | null = null;
      try {
        log(`connecting to ${url}...`);

        // AbortController with configurable timeout for SSE connection
        controller = new AbortController();
        const timeoutId = setTimeout(() => {
          controller!.abort();
          log(`SSE connection timed out after ${sseTimeoutMs}ms`);
        }, sseTimeoutMs);

        const response = await fetch(url, { signal: controller.signal });
        clearTimeout(timeoutId);

        if (!response.ok || !response.body) {
          log(`connection failed: ${response.status}`);
          const delay = backoffDelay(retryAttempt);
          retryAttempt++;
          await sleep(delay);
          continue;
        }

        // Successful connection — reset retry counter
        retryAttempt = 0;

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
        if (!aborted) {
          const delay = backoffDelay(retryAttempt);
          retryAttempt++;
          await sleep(delay);
        }
      } catch (err: unknown) {
        if (controller && (err as Error)?.name === "AbortError") {
          log(`SSE connection timed out (${sseTimeoutMs}ms)`);
        } else {
          log(`error: ${err}`);
        }
        const delay = backoffDelay(retryAttempt);
        retryAttempt++;
        if (!aborted) await sleep(delay);
      } finally {
        controller = null;
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
  let dataStr = "";

  for (const line of frame.split("\n")) {
    if (line.startsWith("data: ")) {
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
