"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.GoOnProcessFlowViewProvider = void 0;
const vscode = require("vscode");
class GoOnProcessFlowViewProvider {
    constructor(_extensionUri, _manager, _context) {
        this._extensionUri = _extensionUri;
        this.manager = _manager;
        this.context = _context;
    }
    _extractResponseText(result) {
        if (!result || typeof result !== 'object') {
            return undefined;
        }
        const candidate = result.response;
        return typeof candidate === 'string' ? candidate : undefined;
    }
    resolveWebviewView(webviewView, _context, _token) {
        this._view = webviewView;
        webviewView.webview.options = {
            enableScripts: true,
            localResourceRoots: [
                this._extensionUri
            ]
        };
        webviewView.webview.html = this._getHtmlForWebview(webviewView.webview);
        webviewView.webview.onDidReceiveMessage(async (message) => {
            switch (message.type) {
                case 'createProcess':
                    await this._createProcess(message.processData);
                    break;
                case 'runProcess':
                    await this._runProcess(message.processId);
                    break;
                case 'updateProcess':
                    await this._updateProcess(message.processId, message.updates);
                    break;
                case 'importProcesses':
                    await this._importProcesses(message.processes);
                    break;
            }
        }, undefined, this.context.subscriptions);
        // Load existing processes
        this._loadProcesses();
    }
    getErrorMessage(error) {
        return error instanceof Error ? error.message : String(error);
    }
    async _loadProcesses() {
        const processes = this.context.globalState.get('go-on-processes', {});
        this._view?.webview.postMessage({
            type: 'processesLoaded',
            processes
        });
    }
    async _importProcesses(imported) {
        if (!imported || typeof imported !== 'object') {
            vscode.window.showErrorMessage('Invalid import data');
            return;
        }
        const processes = this.context.globalState.get('go-on-processes', {});
        Object.keys(imported).forEach((id) => {
            if (imported[id] && imported[id].id) {
                processes[id] = imported[id];
            }
        });
        await this.context.globalState.update('go-on-processes', processes);
        this._view?.webview.postMessage({
            type: 'processesLoaded',
            processes
        });
        vscode.window.showInformationMessage('Processes imported successfully');
    }
    async _createProcess(processData) {
        const processes = this.context.globalState.get('go-on-processes', {});
        const processId = `process_${Date.now()}`;
        processes[processId] = {
            ...processData,
            id: processId,
            created: new Date().toISOString(),
            status: 'created',
            stages: processData.stages || []
        };
        await this.context.globalState.update('go-on-processes', processes);
        this._view?.webview.postMessage({
            type: 'processCreated',
            process: processes[processId]
        });
        vscode.window.showInformationMessage(`Process "${processData.name}" created successfully!`);
    }
    async _runProcess(processId) {
        const processes = this.context.globalState.get('go-on-processes', {});
        const process = processes[processId];
        if (!process) {
            vscode.window.showErrorMessage('Process not found');
            return;
        }
        process.status = 'running';
        await this.context.globalState.update('go-on-processes', processes);
        this._view?.webview.postMessage({
            type: 'processStatusUpdate',
            processId,
            status: 'running'
        });
        try {
            for (let i = 0; i < process.stages.length; i++) {
                const stage = process.stages[i];
                this._view?.webview.postMessage({
                    type: 'stageStatusUpdate',
                    processId,
                    stageIndex: i,
                    status: 'running'
                });
                // Execute stage based on type
                switch (stage.type) {
                    case 'chat': {
                        const result = await this.manager.sendRequest('chat', {
                            messages: [{ role: 'user', content: stage.prompt }]
                        });
                        stage.result = this._extractResponseText(result) || '';
                        break;
                    }
                    case 'code':
                        // Code execution result would be handled
                        stage.result = 'Code execution completed';
                        break;
                    case 'delay':
                        await new Promise(resolve => setTimeout(resolve, Number(stage.delay || 0) * 1000));
                        break;
                    case 'manual':
                        // Wait for manual confirmation
                        await new Promise((resolve) => {
                            vscode.window.showInformationMessage(`Process "${process.name}" - Stage ${i + 1}: ${stage.name}`, 'Continue').then(() => resolve(void 0));
                        });
                        break;
                }
                stage.status = 'completed';
                stage.completedAt = new Date().toISOString();
                this._view?.webview.postMessage({
                    type: 'stageStatusUpdate',
                    processId,
                    stageIndex: i,
                    status: 'completed',
                    result: stage.result
                });
            }
            process.status = 'completed';
            process.completedAt = new Date().toISOString();
            await this.context.globalState.update('go-on-processes', processes);
            this._view?.webview.postMessage({
                type: 'processStatusUpdate',
                processId,
                status: 'completed'
            });
            vscode.window.showInformationMessage(`Process "${process.name}" completed successfully!`);
        }
        catch (error) {
            process.status = 'failed';
            const message = this.getErrorMessage(error);
            process.error = message;
            await this.context.globalState.update('go-on-processes', processes);
            this._view?.webview.postMessage({
                type: 'processStatusUpdate',
                processId,
                status: 'failed',
                error: message
            });
            vscode.window.showErrorMessage(`Process failed: ${message}`);
        }
    }
    async _updateProcess(processId, updates) {
        const processes = this.context.globalState.get('go-on-processes', {});
        // Validate input
        if (!processId) {
            vscode.window.showErrorMessage('Invalid process: ID is required');
            return;
        }
        if (!processes[processId]) {
            vscode.window.showErrorMessage('Process not found');
            return;
        }
        if (updates && typeof updates === 'object' && updates.stages && !Array.isArray(updates.stages)) {
            vscode.window.showErrorMessage('Invalid stages format: must be array');
            return;
        }
        try {
            Object.assign(processes[processId], updates);
            await this.context.globalState.update('go-on-processes', processes);
            this._view?.webview.postMessage({
                type: 'processUpdated',
                process: processes[processId]
            });
        }
        catch (error) {
            vscode.window.showErrorMessage(`Failed to update process: ${this.getErrorMessage(error)}`);
        }
    }
    _getHtmlForWebview(webview) {
        const styleResetUri = webview.asWebviewUri(vscode.Uri.joinPath(this._extensionUri, 'media', 'reset.css'));
        const styleVSCodeUri = webview.asWebviewUri(vscode.Uri.joinPath(this._extensionUri, 'media', 'vscode.css'));
        const scriptUri = webview.asWebviewUri(vscode.Uri.joinPath(this._extensionUri, 'media', 'processFlow.js'));
        const nonce = getNonce();
        return `<!DOCTYPE html>
            <html lang="en">
            <head>
                <meta charset="UTF-8">
                <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource} 'unsafe-inline'; script-src 'nonce-${nonce}';">
                <meta name="viewport" content="width=device-width, initial-scale=1.0">
                <link href="${styleResetUri}" rel="stylesheet">
                <link href="${styleVSCodeUri}" rel="stylesheet">
                <title>Go-On Process Flow</title>
                <style>
                    .process-container {
                        height: 100%;
                        display: flex;
                        flex-direction: column;
                        padding: 10px;
                    }
                    .process-canvas {
                        flex: 1;
                        border: 1px solid var(--vscode-panel-border);
                        border-radius: 3px;
                        background: var(--vscode-input-background);
                        position: relative;
                        overflow: hidden;
                    }
                    .process-header {
                        display: flex;
                        justify-content: space-between;
                        align-items: center;
                        margin-bottom: 10px;
                    }
                    .process-title {
                        font-weight: bold;
                        font-size: 1.1em;
                    }
                    .process-controls {
                        display: flex;
                        gap: 5px;
                    }
                    .process-btn {
                        padding: 4px 8px;
                        background: var(--vscode-button-background);
                        color: var(--vscode-button-foreground);
                        border: none;
                        border-radius: 3px;
                        cursor: pointer;
                        font-size: 0.8em;
                    }
                    .process-btn:hover {
                        background: var(--vscode-button-hoverBackground);
                    }
                    .process-btn.primary {
                        background: var(--vscode-button-background);
                    }
                    .stage-node {
                        position: absolute;
                        width: 120px;
                        height: 60px;
                        border: 2px solid var(--vscode-panel-border);
                        border-radius: 8px;
                        background: var(--vscode-editor-background);
                        display: flex;
                        flex-direction: column;
                        align-items: center;
                        justify-content: center;
                        cursor: move;
                        font-size: 0.8em;
                        text-align: center;
                        padding: 4px;
                    }
                    .stage-node.running {
                        border-color: var(--vscode-progressBar-background);
                        box-shadow: 0 0 8px var(--vscode-progressBar-background);
                    }
                    .stage-node.completed {
                        border-color: var(--vscode-notificationsInfoIcon-foreground);
                        background: rgba(0, 255, 0, 0.1);
                    }
                    .stage-node.failed {
                        border-color: var(--vscode-notificationsErrorIcon-foreground);
                        background: rgba(255, 0, 0, 0.1);
                    }
                    .stage-name {
                        font-weight: bold;
                        margin-bottom: 2px;
                    }
                    .stage-type {
                        font-size: 0.7em;
                        color: var(--vscode-descriptionForeground);
                    }
                    .connection-line {
                        position: absolute;
                        pointer-events: none;
                        stroke: var(--vscode-panel-border);
                        stroke-width: 2;
                        fill: none;
                    }
                    .create-process {
                        margin-bottom: 10px;
                    }
                    .create-process-btn {
                        width: 100%;
                        padding: 8px;
                        background: var(--vscode-button-background);
                        color: var(--vscode-button-foreground);
                        border: none;
                        border-radius: 3px;
                        cursor: pointer;
                    }
                    .create-process-btn:hover {
                        background: var(--vscode-button-hoverBackground);
                    }
                    .process-list {
                        max-height: 200px;
                        overflow-y: auto;
                        margin-bottom: 10px;
                    }
                    .process-item {
                        padding: 8px;
                        border: 1px solid var(--vscode-panel-border);
                        border-radius: 3px;
                        margin-bottom: 4px;
                        background: var(--vscode-input-background);
                        cursor: pointer;
                    }
                    .process-item:hover {
                        background: var(--vscode-list-hoverBackground);
                    }
                    .process-item.active {
                        border-color: var(--vscode-focusBorder);
                    }
                </style>
            </head>
            <body>
                <div class="process-container">
                    <div class="process-header">
                        <div class="process-title" id="currentProcessTitle">No Process Selected</div>
                        <div class="process-controls">
                            <button class="process-btn" id="createProcessBtn">Create</button>
                            <button class="process-btn primary" id="runProcessBtn">Run</button>
                            <button class="process-btn" id="exportProcessBtn">Export JSON</button>
                            <button class="process-btn" id="importProcessBtn">Import JSON</button>
                            <input id="importFile" type="file" accept="application/json" style="display:none" />
                        </div>
                    </div>
                    <div class="process-list" id="processList"></div>
                    <div class="process-canvas" id="processCanvas"></div>
                </div>
                <script nonce="${nonce}" src="${scriptUri}"></script>
            </body>
            </html>`;
    }
}
exports.GoOnProcessFlowViewProvider = GoOnProcessFlowViewProvider;
GoOnProcessFlowViewProvider.viewType = 'go-on-process-flow';
function getNonce() {
    let text = '';
    const possible = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';
    for (let i = 0; i < 32; i++) {
        text += possible.charAt(Math.floor(Math.random() * possible.length));
    }
    return text;
}
//# sourceMappingURL=processFlowView.js.map