import * as vscode from "vscode";

import { protocolContract } from "./protocolContract";
import { RuntimeManagerLike } from "./managerTypes";

export class StatusMonitor {
  private statusBarItem: vscode.StatusBarItem;
  private healthCheckTimer: NodeJS.Timeout | undefined;
  private manager: RuntimeManagerLike;
  private consecutiveFailures = 0;
  private readonly maxFailures = 3;
  private healthCheckInFlight = false;
  private failureWarningShown = false;
  private _configListener: vscode.Disposable | undefined;

  constructor(manager: RuntimeManagerLike) {
    this.manager = manager;
    this.statusBarItem = vscode.window.createStatusBarItem(
      vscode.StatusBarAlignment.Left,
      100,
    );
    this.statusBarItem.command = "go-on.openChat";
    this.updateStatus();
    this.statusBarItem.show();

    // Start health monitoring
    this.startHealthMonitoring();

    // Listen for config changes to re-read the health interval
    this._configListener = vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration("go-on.health.interval")) {
        this.restartHealthMonitoring();
      }
    });
  }

  private async updateStatus() {
    const isRunning = this.manager.isRunning();
    this.statusBarItem.text = "$(comment-discussion) Go-On Chat";
    this.statusBarItem.tooltip = isRunning
      ? "Go-On backend is running. Click to open chat."
      : "Click to open chat. Backend can be configured from Chat/Settings.";
    this.statusBarItem.backgroundColor = undefined;
  }

  private restartHealthMonitoring() {
    this.stopHealthMonitoring();
    this.startHealthMonitoring();
  }

  private startHealthMonitoring() {
    if (this.healthCheckTimer) {
      return;
    }
    const config = vscode.workspace.getConfiguration("go-on");
    const interval = config.get<number>("health.interval", 300) * 1000; // Convert to milliseconds

    if (interval <= 0) {
      return; // Disabled
    }

    this.healthCheckTimer = setInterval(async () => {
      if (!this.manager.isRunning() || this.healthCheckInFlight) {
        return;
      }

      this.healthCheckInFlight = true;
      try {
        const health = await this.manager.sendRequest("runtime.health");
        this.updateHealthStatus(health);
        this.consecutiveFailures = 0;
        this.failureWarningShown = false;
      } catch {
        this.consecutiveFailures++;
        this.statusBarItem.tooltip = `Go-On Status: ${protocolContract.statusTerms.healthCheckFailed} (${this.consecutiveFailures}/${this.maxFailures})\nMonitoring will continue automatically.\nClick to open chat`;

        if (
          this.consecutiveFailures >= this.maxFailures &&
          !this.failureWarningShown
        ) {
          this.failureWarningShown = true;
          void vscode.window.showWarningMessage(
            "Go-On: Health checks are failing, but monitoring is still running and will recover automatically once the backend responds again.",
          );
        }
      } finally {
        this.healthCheckInFlight = false;
      }
    }, interval);
  }

  private stopHealthMonitoring() {
    if (this.healthCheckTimer) {
      clearInterval(this.healthCheckTimer);
      this.healthCheckTimer = undefined;
    }
  }

  private updateHealthStatus(health: unknown) {
    // Update tooltip with health information
    const healthInfo =
      typeof health === "object"
        ? JSON.stringify(health, null, 2)
        : String(health);
    this.statusBarItem.tooltip = `Go-On Status: ${protocolContract.statusTerms.healthy}\nLast health check: ${new Date().toLocaleTimeString()}\n${healthInfo}\nClick to open chat`;
  }

  public refresh() {
    this.updateStatus();
    if (!this.healthCheckTimer) {
      this.startHealthMonitoring();
    }
  }

  public dispose() {
    this.stopHealthMonitoring();
    this.statusBarItem.dispose();
    this._configListener?.dispose();
  }
}
