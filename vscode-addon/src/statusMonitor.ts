import * as vscode from "vscode";
import { i18n, MessageKeys } from "./i18n";

import { RuntimeManagerLike } from "./managerTypes";

interface ProbeReport {
  probes?: {
    provider_dependencies?: {
      ready: boolean;
      [key: string]: unknown;
    };
    [key: string]: unknown;
  };
  [key: string]: unknown;
}

export class StatusMonitor {
  private statusBarItem: vscode.StatusBarItem;
  private healthCheckTimer: NodeJS.Timeout | undefined;
  private manager: RuntimeManagerLike;
  private consecutiveFailures = 0;
  private readonly maxFailures = 3;
  private healthCheckInFlight = false;
  private failureWarningShown = false;
  private _configListener: vscode.Disposable | undefined;
  private _disposed = false;
  private _noProviderWarningShown: boolean = false;

  constructor(manager: RuntimeManagerLike) {
    this.manager = manager;
    this.statusBarItem = vscode.window.createStatusBarItem(
      vscode.StatusBarAlignment.Left,
      100,
    );
    this.statusBarItem.backgroundColor = undefined;
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
    if (this._disposed) return;
    this.stopHealthMonitoring();
    this.startHealthMonitoring();
  }

  private startHealthMonitoring() {
    this.stopHealthMonitoring();
    const config = vscode.workspace.getConfiguration("go-on");
    const interval = config.get<number>("health.interval", 300) * 1000;
    if (interval <= 0) return;

    this.healthCheckTimer = setInterval(async () => {
      if (
        this._disposed ||
        !this.manager.isRunning() ||
        this.healthCheckInFlight
      ) {
        return;
      }
      this.healthCheckInFlight = true;
      try {
        const health = await this.manager.sendRequest("runtime.health");
        this.updateHealthStatus(health);
        this.consecutiveFailures = 0;
        this.failureWarningShown = false;

        // Check provider readiness when runtime is healthy
        if (health) {
          await this._checkProviderReadiness();
        }
      } catch {
        this.consecutiveFailures++;
        this.statusBarItem.tooltip = i18n.getMessage(
          MessageKeys.statusBarHealthCheckFailedTooltip,
          [String(this.consecutiveFailures), String(this.maxFailures)],
        );
        if (
          this.consecutiveFailures >= this.maxFailures &&
          !this.failureWarningShown
        ) {
          this.failureWarningShown = true;
          void vscode.window.showWarningMessage(
            i18n.getMessage(MessageKeys.healthCheckWarning),
          );
        }
      } finally {
        this.healthCheckInFlight = false;
      }
    }, interval);
  }

  private async _checkProviderReadiness(): Promise<void> {
    try {
      const probeResult = (await this.manager.sendRequest(
        "health.probes",
      )) as ProbeReport;
      const probes = probeResult?.probes;
      const providerDep = probes?.provider_dependencies;
      const hasProviderConfig = providerDep !== undefined;
      const providerReady = hasProviderConfig && providerDep!.ready;

      if (hasProviderConfig && !providerReady) {
        this.statusBarItem.text = "$(warning) Go-On Chat";
        this.statusBarItem.tooltip =
          "Go-On is running but no AI provider is ready. Configure API key in Settings.";
        this.statusBarItem.backgroundColor = new vscode.ThemeColor(
          "statusBarItem.warningBackground",
        );
        if (!this._noProviderWarningShown) {
          this._noProviderWarningShown = true;
          void vscode.window
            .showWarningMessage(
              "Go-On needs an AI provider API key to function. Open Settings to configure one.",
              "Open Settings",
            )
            .then((action) => {
              if (action === "Open Settings") {
                return vscode.commands.executeCommand("go-on.openSettings");
              }
            });
        }
        return;
      }

      // Provider ready — reset to normal status
      this._noProviderWarningShown = false;
      this.statusBarItem.backgroundColor = undefined;
      this.updateStatus();
    } catch {
      // probe check failed — ignore, will retry on next interval
    }
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
    this.statusBarItem.tooltip = i18n.getMessage(
      MessageKeys.statusBarHealthTooltip,
      [new Date().toLocaleTimeString(), healthInfo],
    );
  }

  public refresh() {
    if (this._disposed) return;
    this.updateStatus();
    if (!this.healthCheckTimer) {
      this.startHealthMonitoring();
    }
  }

  public dispose() {
    this._disposed = true;
    this.stopHealthMonitoring(); // Stop timer first
    this.statusBarItem.dispose();
    this._configListener?.dispose();
  }
}
