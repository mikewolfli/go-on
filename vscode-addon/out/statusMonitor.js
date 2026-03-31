"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.StatusMonitor = void 0;
const vscode = require("vscode");
class StatusMonitor {
    constructor(manager) {
        this.manager = manager;
        this.statusBarItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
        this.statusBarItem.command = 'go-on.openChat';
        this.updateStatus();
        this.statusBarItem.show();
        // Start health monitoring
        this.startHealthMonitoring();
    }
    async updateStatus() {
        const isRunning = this.manager.isRunning();
        this.statusBarItem.text = `$(robot) Go-On: ${isRunning ? 'Running' : 'Stopped'}`;
        this.statusBarItem.tooltip = `Go-On Status: ${isRunning ? 'Connected' : 'Disconnected'}\nClick to open chat`;
        if (isRunning) {
            this.statusBarItem.backgroundColor = undefined;
        }
        else {
            this.statusBarItem.backgroundColor = new vscode.ThemeColor('statusBarItem.errorBackground');
        }
    }
    startHealthMonitoring() {
        const config = vscode.workspace.getConfiguration('go-on');
        const interval = config.get('health.interval', 300) * 1000; // Convert to milliseconds
        this.healthCheckTimer = setInterval(async () => {
            if (this.manager.isRunning()) {
                try {
                    const health = await this.manager.sendRequest('runtime.health');
                    this.updateHealthStatus(health);
                }
                catch (error) {
                    console.warn('Health check failed:', error);
                    this.statusBarItem.tooltip = 'Go-On Status: Health check failed\nClick to open chat';
                }
            }
        }, interval);
    }
    updateHealthStatus(health) {
        // Update tooltip with health information
        const healthInfo = typeof health === 'object' ? JSON.stringify(health, null, 2) : String(health);
        this.statusBarItem.tooltip = `Go-On Status: Healthy\nLast health check: ${new Date().toLocaleTimeString()}\n${healthInfo}\nClick to open chat`;
    }
    refresh() {
        this.updateStatus();
    }
    dispose() {
        if (this.healthCheckTimer) {
            clearInterval(this.healthCheckTimer);
        }
        this.statusBarItem.dispose();
    }
}
exports.StatusMonitor = StatusMonitor;
//# sourceMappingURL=statusMonitor.js.map