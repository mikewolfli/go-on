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
import { parseSseChunk } from "./runtime/sseStream";
import { StateSyncEvent } from "./generated/stateSyncTypes";

export { StateSyncEvent };

/** Default SSE connection timeout (in ms). */
const DEFAULT_SSE_TIMEOUT_MS = 15_000;

/** Maximum delay cap for exponential backoff (30 seconds, matches reconnect.ts / cross-client-sync.md). */
const MAX_BACKOFF_MS = 30_000;

/** Base delay for exponential backoff (1 second). */
const BASE_DELAY_MS = 1_000;

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
 * Compute exponential backoff delay with 30% jitter.
 *
 * Unified formula from contracts/cross-client-sync.md:
 * delay = min(1000 * 2^attempt, 30000) * (0.7 + random * 0.3)
 * Matches runtime/reconnect.ts. This prevents thundering herd when
 * multiple clients reconnect simultaneously.
 *
 * @param attempt - zero-based retry attempt number
 * @returns delay in milliseconds
 */
function backoffDelay(attempt: number): number {
  const exponential = BASE_DELAY_MS * Math.pow(2, attempt);
  const capped = Math.min(exponential, MAX_BACKOFF_MS);
  // 30% jitter: keep at least 70% of the base delay
  const jitter = 0.7 + Math.random() * 0.3;
  return Math.round(capped * jitter);
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
  // Reuse the canonical SSE frame parser (runtime/sseStream.ts) instead of a
  // second hand-rolled data:-line extractor. The backend always emits
  // `event: state_sync\ndata: {json}`; parseSseChunk handles both lines and
  // injects `_event_type`.
  const frames = parseSseChunk(frame + "\n\n");
  if (frames.length === 0) return;
  const event = frames[0].data as unknown as StateSyncEvent;
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
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
