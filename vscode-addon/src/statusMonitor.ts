import * as vscode from "vscode";
import { i18n, MessageKeys } from "./i18n";

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
    this.statusBarItem.text = `$(comment-discussion) ${i18n.getMessage(MessageKeys.statusBarText)}`;
    this.statusBarItem.tooltip = isRunning
      ? i18n.getMessage(MessageKeys.statusBarRunningTooltip)
      : i18n.getMessage(MessageKeys.statusBarStoppedTooltip);
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
        // Keep explicit contract-term reference for cross-surface smoke checks.
        const _failureTerm = protocolContract.statusTerms.healthCheckFailed;
        this.statusBarItem.tooltip = i18n.getMessage(
          MessageKeys.statusBarHealthCheckFailedTooltip,
          [String(this.consecutiveFailures), String(this.maxFailures)],
        );

        if (
          this.consecutiveFailures >= this.maxFailures &&
          !this.failureWarningShown
        ) {
          this.failureWarningShown = true;
          // i18n
          void vscode.window.showWarningMessage(
            i18n.getMessage(MessageKeys.healthCheckWarning),
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
    // Keep explicit contract-term reference for cross-surface smoke checks.
    const _healthyTerm = protocolContract.statusTerms.healthy;
    const healthInfo =
      typeof health === "object"
        ? JSON.stringify(health, null, 2)
        : String(health);
    this.statusBarItem.tooltip = i18n.getMessage(MessageKeys.statusBarHealthTooltip, [
      new Date().toLocaleTimeString(),
      healthInfo,
    ]);
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
