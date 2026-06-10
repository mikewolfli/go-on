import * as vscode from "vscode";

/**
 * Manages heartbeat ping/pong for both framed and legacy (JSON-RPC) modes.
 *
 * Framed heartbeat: sends `{"type":"heartbeat.ping"}` via FramedWriter at intervals.
 * Legacy heartbeat: sends `{"jsonrpc":"2.0","id":0,"method":"runtime.health"}` via stdin.
 */
export class HeartbeatManager {
  private readonly HEARTBEAT_INTERVAL_MS = 30000;
  private readonly HEARTBEAT_TIMEOUT_MS = 90000;

  private _legacyTimer: ReturnType<typeof setInterval> | null = null;
  private _legacyTimeoutTimer: ReturnType<typeof setTimeout> | null = null;
  private _framedIntervalTimer: ReturnType<typeof setInterval> | null = null;
  private _framedTimeoutTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(
    private readonly sendFramedPing: () => void,
    private readonly sendLegacyPing: () => void,
    private readonly onTimeout: () => void,
    private readonly outputChannel?: vscode.OutputChannel,
  ) {}

  // ── Framed heartbeat ──

  /** Start the framed heartbeat interval. */
  startFramed(): void {
    this.stopFramed();
    this.sendFramedPing();
    this._framedIntervalTimer = setInterval(() => {
      this.sendFramedPing();
    }, this.HEARTBEAT_INTERVAL_MS);
    this.resetFramedTimeout();
  }

  /** Reset the framed heartbeat timeout (called when pong is received). */
  resetFramedTimeout(): void {
    if (this._framedTimeoutTimer) {
      clearTimeout(this._framedTimeoutTimer);
      this._framedTimeoutTimer = null;
    }
    this._framedTimeoutTimer = setTimeout(() => {
      this.handleTimeout("framed");
    }, this.HEARTBEAT_TIMEOUT_MS);
  }

  /** Stop all framed heartbeat timers. */
  stopFramed(): void {
    if (this._framedIntervalTimer) {
      clearInterval(this._framedIntervalTimer);
      this._framedIntervalTimer = null;
    }
    if (this._framedTimeoutTimer) {
      clearTimeout(this._framedTimeoutTimer);
      this._framedTimeoutTimer = null;
    }
  }

  // ── Legacy heartbeat ──

  /** Start the legacy (JSON-RPC) heartbeat interval. */
  startLegacy(): void {
    this.stopLegacy();
    this.sendLegacyPing();
    this._legacyTimer = setInterval(() => {
      this.sendLegacyPing();
    }, this.HEARTBEAT_INTERVAL_MS);
    this.resetLegacyTimeout();
  }

  /** Reset the legacy heartbeat timeout (called on any valid response). */
  resetLegacyTimeout(): void {
    if (this._legacyTimeoutTimer) {
      clearTimeout(this._legacyTimeoutTimer);
      this._legacyTimeoutTimer = null;
    }
    this._legacyTimeoutTimer = setTimeout(() => {
      this.handleTimeout("legacy");
    }, this.HEARTBEAT_TIMEOUT_MS);
  }

  /** Stop all legacy heartbeat timers. */
  stopLegacy(): void {
    if (this._legacyTimer) {
      clearInterval(this._legacyTimer);
      this._legacyTimer = null;
    }
    if (this._legacyTimeoutTimer) {
      clearTimeout(this._legacyTimeoutTimer);
      this._legacyTimeoutTimer = null;
    }
  }

  // ── Common ──

  /** Stop all heartbeat timers (both framed and legacy). */
  stopAll(): void {
    this.stopFramed();
    this.stopLegacy();
  }

  private handleTimeout(mode: string): void {
    this.outputChannel?.appendLine(
      `[heartbeat] Timeout (${mode}) — no pong received, triggering reconnection`,
    );
    void vscode.window.showWarningMessage(
      "Go-On connection lost (heartbeat timeout). Reconnecting...",
    );
    this.onTimeout();
  }
}
