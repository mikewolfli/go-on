import * as vscode from 'vscode';

import { protocolContract } from './protocolContract';

export class StatusMonitor {
    private statusBarItem: vscode.StatusBarItem;
    private healthCheckTimer: NodeJS.Timeout | undefined;
    private manager: any;

    constructor(manager: any) {
        this.manager = manager;
        this.statusBarItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
        this.statusBarItem.command = 'go-on.openChat';
        this.updateStatus();
        this.statusBarItem.show();

        // Start health monitoring
        this.startHealthMonitoring();
    }

    private async updateStatus() {
        const isRunning = this.manager.isRunning();
        this.statusBarItem.text = '$(comment-discussion) Go-On Chat';
        this.statusBarItem.tooltip = isRunning
            ? 'Go-On backend is running. Click to open chat.'
            : 'Click to open chat. Backend can be configured from Chat/Settings.';
        this.statusBarItem.backgroundColor = undefined;
    }

    private startHealthMonitoring() {
        const config = vscode.workspace.getConfiguration('go-on');
        const interval = config.get<number>('health.interval', 300) * 1000; // Convert to milliseconds

        let consecutiveFailures = 0;
        const maxFailures = 3;

        this.healthCheckTimer = setInterval(async () => {
            if (this.manager.isRunning()) {
                try {
                    const health = await this.manager.sendRequest('runtime.health');
                    this.updateHealthStatus(health);
                    consecutiveFailures = 0; // Reset counter on success
                } catch (error) {
                    consecutiveFailures++;
                    console.warn(`${protocolContract.statusTerms.healthCheckFailed} (${consecutiveFailures}/${maxFailures}):`, error);

                    this.statusBarItem.tooltip = `Go-On Status: ${protocolContract.statusTerms.healthCheckFailed} (${consecutiveFailures}/${maxFailures})\nClick to open chat`;

                    if (consecutiveFailures >= maxFailures) {
                        console.error('Max health check failures reached, stopping monitoring');
                        this.stopHealthMonitoring();
                        vscode.window.showWarningMessage('Go-On: Health checks failed. Please restart the extension.');
                    }
                }
            }
        }, interval);
    }

    private stopHealthMonitoring() {
        if (this.healthCheckTimer) {
            clearInterval(this.healthCheckTimer);
            this.healthCheckTimer = undefined;
        }
    }

    private updateHealthStatus(health: any) {
        // Update tooltip with health information
        const healthInfo = typeof health === 'object' ? JSON.stringify(health, null, 2) : String(health);
        this.statusBarItem.tooltip = `Go-On Status: ${protocolContract.statusTerms.healthy}\nLast health check: ${new Date().toLocaleTimeString()}\n${healthInfo}\nClick to open chat`;
    }

    public refresh() {
        this.updateStatus();
    }

    public dispose() {
        this.stopHealthMonitoring();
        this.statusBarItem.dispose();
    }
}