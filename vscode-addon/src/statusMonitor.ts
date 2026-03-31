import * as vscode from 'vscode';

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
        this.statusBarItem.text = `$(robot) Go-On: ${isRunning ? 'Running' : 'Stopped'}`;
        this.statusBarItem.tooltip = `Go-On Status: ${isRunning ? 'Connected' : 'Disconnected'}\nClick to open chat`;

        if (isRunning) {
            this.statusBarItem.backgroundColor = undefined;
        } else {
            this.statusBarItem.backgroundColor = new vscode.ThemeColor('statusBarItem.errorBackground');
        }
    }

    private startHealthMonitoring() {
        const config = vscode.workspace.getConfiguration('go-on');
        const interval = config.get<number>('health.interval', 300) * 1000; // Convert to milliseconds

        this.healthCheckTimer = setInterval(async () => {
            if (this.manager.isRunning()) {
                try {
                    const health = await this.manager.sendRequest('runtime.health');
                    this.updateHealthStatus(health);
                } catch (error) {
                    console.warn('Health check failed:', error);
                    this.statusBarItem.tooltip = 'Go-On Status: Health check failed\nClick to open chat';
                }
            }
        }, interval);
    }

    private updateHealthStatus(health: any) {
        // Update tooltip with health information
        const healthInfo = typeof health === 'object' ? JSON.stringify(health, null, 2) : String(health);
        this.statusBarItem.tooltip = `Go-On Status: Healthy\nLast health check: ${new Date().toLocaleTimeString()}\n${healthInfo}\nClick to open chat`;
    }

    public refresh() {
        this.updateStatus();
    }

    public dispose() {
        if (this.healthCheckTimer) {
            clearInterval(this.healthCheckTimer);
        }
        this.statusBarItem.dispose();
    }
}