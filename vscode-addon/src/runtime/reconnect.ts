import * as vscode from "vscode";
import { backoffDelayMs } from "../utils";

/**
 * Manages reconnection logic with exponential backoff.
 *
 * Unified strategy (see contracts/cross-client-sync.md):
 * Backoff formula: min(1000 * 2^attempt, 30000)ms with 30% jitter.
 * Attempt 0: ~1000ms, 1: ~2000ms, 2: ~4000ms, 3: ~8000ms, 4: ~16000ms, 5+: ~30000ms
 * Supports unlimited retries for long-running multi-agent workflows.
 */
export class ReconnectManager {
  private _attempts = 0;
  private _timer: ReturnType<typeof setTimeout> | undefined;

  constructor(
    private readonly doReconnect: () => Promise<void>,
    private readonly outputChannel?: vscode.OutputChannel,
  ) {}

  /** Reset the attempt counter (call on successful connection/reconnect). */
  reset(): void {
    this._attempts = 0;
  }

  /** Get the current number of reconnect attempts. */
  get attempts(): number {
    return this._attempts;
  }

  /** Calculate exponential backoff delay for the given attempt. */
  backoffMs(attempt: number): number {
    // Unified formula shared with the state-sync listener
    // (contracts/cross-client-sync.md): 1000ms base, 30s cap, 30% jitter.
    return backoffDelayMs(attempt);
  }

  /** Schedule a reconnection attempt. */
  schedule(): void {
    const delay = this.backoffMs(this._attempts);
    this.outputChannel?.appendLine(
      `[reconnect] Scheduling attempt ${this._attempts + 1} in ${delay}ms...`,
    );
    this._timer = setTimeout(() => {
      void this.doAttempt();
    }, delay);
  }

  /** Cancel any pending reconnection. */
  cancel(): void {
    if (this._timer) {
      clearTimeout(this._timer);
      this._timer = undefined;
    }
  }

  /** Perform a reconnection attempt (increments counter and calls doReconnect). */
  private async doAttempt(): Promise<void> {
    this._attempts++;
    this.outputChannel?.appendLine(
      `[reconnect] Attempt ${this._attempts} with unlimited retries...`,
    );
    await this.doReconnect();
  }
}
