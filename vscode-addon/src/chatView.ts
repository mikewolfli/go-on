import * as vscode from 'vscode';
import * as path from 'path';
import { spawn } from 'child_process';

export class GoOnChatViewProvider implements vscode.WebviewViewProvider {
    public static readonly viewType = 'go-on-chat';
    private _view?: vscode.WebviewView;
    private _currentSession: string = 'default';
    private _sessions: Map<string, any[]> = new Map();

    constructor(
        private readonly _extensionUri: vscode.Uri,
        private readonly manager: any,
        private readonly context: vscode.ExtensionContext,
        private readonly onViewResolved?: () => void | Promise<void>
    ) {
        this._loadSessions();
    }

    private _loadSessions() {
        const storedSessions = this.context.globalState.get('go-on-chat-sessions', {});
        for (const [sessionName, messages] of Object.entries(storedSessions)) {
            this._sessions.set(sessionName, messages as any[]);
        }
        // Ensure default session exists
        if (!this._sessions.has('default')) {
            this._sessions.set('default', []);
        }
    }

    private _saveSessions() {
        const sessionsObject: { [key: string]: any[] } = {};
        for (const [sessionName, messages] of this._sessions) {
            sessionsObject[sessionName] = messages;
        }
        this.context.globalState.update('go-on-chat-sessions', sessionsObject);
    }

    private _getCurrentSessionMessages(): any[] {
        return this._sessions.get(this._currentSession) || [];
    }

    private _addMessageToCurrentSession(message: any) {
        const messages = this._getCurrentSessionMessages();
        messages.push(message);
        this._sessions.set(this._currentSession, messages);
        this._saveSessions();
    }

    public resolveWebviewView(
        webviewView: vscode.WebviewView,
        context: vscode.WebviewViewResolveContext,
        _token: vscode.CancellationToken,
    ) {
        this._view = webviewView;

        webviewView.webview.options = {
            enableScripts: true,
            localResourceRoots: [
                this._extensionUri
            ]
        };

        webviewView.webview.html = this._getHtmlForWebview(webviewView.webview);

        if (this.onViewResolved) {
            Promise.resolve(this.onViewResolved()).catch((error) => {
                console.warn('Failed to initialize Go-On runtime after chat view opened:', error);
            });
        }

        webviewView.webview.onDidReceiveMessage(
            async (message) => {
                switch (message.type) {
                    case 'sendMessage':
                        await this._handleSendMessage(message.text);
                        break;
                    case 'clearChat':
                        this._clearCurrentSession();
                        break;
                    case 'exportChat':
                        this._exportCurrentSession();
                        break;
                    case 'runCode':
                        await this._handleRunCode(message.code, message.language);
                        break;
                    case 'newSession':
                        this._createNewSession(message.sessionName);
                        break;
                    case 'switchSession':
                        this._switchSession(message.sessionName);
                        break;
                    case 'getSessions':
                        this._sendSessionsList();
                        break;
                    case 'getSessions':
                        this._sendSessionsList();
                        break;
                }
            },
            undefined,
            this.context.subscriptions
        );
    }

    private async _handleSendMessage(text: string) {
        if (!this._view) return;

        try {
            // Add user message to current session
            const userMessage = {
                role: 'user',
                content: text,
                timestamp: new Date().toISOString()
            };
            this._addMessageToCurrentSession(userMessage);

            // Send message to UI
            this._view.webview.postMessage({
                type: 'addMessage',
                ...userMessage
            });

            // Send to Go-On
            const result = await this.manager.sendRequest('chat', {
                messages: [{ role: 'user', content: text }]
            });

            // Add response to current session
            const assistantMessage = {
                role: 'assistant',
                content: result.response || JSON.stringify(result),
                timestamp: new Date().toISOString()
            };
            this._addMessageToCurrentSession(assistantMessage);

            // Send response to UI
            this._view.webview.postMessage({
                type: 'addMessage',
                ...assistantMessage
            });
        } catch (error: any) {
            const errorMessage = {
                role: 'error',
                content: `Error: ${error.message}`,
                timestamp: new Date().toISOString()
            };
            this._addMessageToCurrentSession(errorMessage);

            this._view.webview.postMessage({
                type: 'addMessage',
                ...errorMessage
            });
        }
    }

    public postMessage(message: any) {
        this._view?.webview.postMessage(message);
    }

    public createNewSession(sessionName: string) {
        this.postMessage({
            type: 'newSession',
            sessionName
        });
    }

    public switchSession(sessionName: string) {
        this.postMessage({
            type: 'switchSession',
            sessionName
        });
    }

    private async _handleRunCode(code: string, language: string) {
        if (!this._view) return;

        try {
            let result = '';

            switch (language) {
                case 'javascript':
                    try {
                        // Use Function constructor instead of eval for better security
                        result = String(new Function('return (' + code + ')()')()); 
                    } catch (e: any) {
                        result = `Error: ${e.message}`;
                    }
                    break;
                case 'python':
                    result = await this._executePythonCode(code);
                    break;
                case 'bash':
                case 'shell':
                    result = await this._executeShellCode(code);
                    break;
                default:
                    result = `Code execution not supported for ${language}`;
            }

            this._view.webview.postMessage({
                type: 'codeResult',
                result: result
            });
        } catch (error: any) {
            this._view.webview.postMessage({
                type: 'codeResult',
                result: `Execution failed: ${error.message}`
            });
        }
    }

    private async _executePythonCode(code: string): Promise<string> {
        return new Promise((resolve) => {
            const pythonPath = 'python'; // Could be configurable

            const pythonProcess = spawn(pythonPath, ['-c', code], {
                cwd: this.context.extensionUri.fsPath
            });

            let stdout = '';
            let stderr = '';

            pythonProcess.stdout.on('data', (data: Buffer) => {
                stdout += data.toString();
            });

            pythonProcess.stderr.on('data', (data: Buffer) => {
                stderr += data.toString();
            });

            pythonProcess.on('close', (code: number) => {
                if (code === 0) {
                    resolve(stdout || 'Code executed successfully (no output)');
                } else {
                    resolve(`Error (exit code ${code}):\n${stderr || stdout}`);
                }
            });

            pythonProcess.on('error', (error: Error) => {
                resolve(`Failed to execute Python: ${error.message}\nMake sure Python is installed and in your PATH.`);
            });

            // Timeout after 10 seconds
            setTimeout(() => {
                pythonProcess.kill();
                resolve('Python execution timed out after 10 seconds');
            }, 10000);
        });
    }

    private async _executeShellCode(code: string): Promise<string> {
        return new Promise((resolve) => {
            const shell = process.platform === 'win32' ? 'cmd' : 'bash';
            const shellArg = process.platform === 'win32' ? '/c' : '-c';

            const shellProcess = spawn(shell, [shellArg, code], {
                cwd: this.context.extensionUri.fsPath
            });

            let stdout = '';
            let stderr = '';

            shellProcess.stdout.on('data', (data: Buffer) => {
                stdout += data.toString();
            });

            shellProcess.stderr.on('data', (data: Buffer) => {
                stderr += data.toString();
            });

            shellProcess.on('close', (code: number) => {
                if (code === 0) {
                    resolve(stdout || 'Command executed successfully (no output)');
                } else {
                    resolve(`Error (exit code ${code}):\n${stderr || stdout}`);
                }
            });

            shellProcess.on('error', (error: Error) => {
                resolve(`Failed to execute shell command: ${error.message}`);
            });

            // Timeout after 15 seconds for shell commands
            setTimeout(() => {
                shellProcess.kill();
                resolve('Shell execution timed out after 15 seconds');
            }, 15000);
        });
    }

    private _createNewSession(sessionName: string) {
        if (this._sessions.has(sessionName)) {
            this._view?.webview.postMessage({
                type: 'error',
                message: `Session "${sessionName}" already exists`
            });
            return;
        }

        this._sessions.set(sessionName, []);
        this._saveSessions();
        this._switchSession(sessionName);
    }

    private _switchSession(sessionName: string) {
        if (!this._sessions.has(sessionName)) {
            this._view?.webview.postMessage({
                type: 'error',
                message: `Session "${sessionName}" does not exist`
            });
            return;
        }

        this._currentSession = sessionName;
        const messages = this._getCurrentSessionMessages();

        this._view?.webview.postMessage({
            type: 'switchSession',
            sessionName,
            messages
        });
    }

    private _clearCurrentSession() {
        this._sessions.set(this._currentSession, []);
        this._saveSessions();

        this._view?.webview.postMessage({
            type: 'clearChat'
        });
    }

    private _exportCurrentSession() {
        const messages = this._getCurrentSessionMessages();
        const exportData = {
            session: this._currentSession,
            timestamp: new Date().toISOString(),
            messages
        };

        vscode.workspace.openTextDocument({
            content: JSON.stringify(exportData, null, 2),
            language: 'json'
        }).then(doc => {
            vscode.window.showTextDocument(doc);
        });
    }

    private _sendSessionsList() {
        const sessions = Array.from(this._sessions.keys());
        this._view?.webview.postMessage({
            type: 'sessionsList',
            sessions,
            currentSession: this._currentSession
        });
    }

    private _getHtmlForWebview(webview: vscode.Webview) {
        const styleResetUri = webview.asWebviewUri(vscode.Uri.joinPath(this._extensionUri, 'media', 'reset.css'));
        const styleVSCodeUri = webview.asWebviewUri(vscode.Uri.joinPath(this._extensionUri, 'media', 'vscode.css'));
        const scriptUri = webview.asWebviewUri(vscode.Uri.joinPath(this._extensionUri, 'media', 'chat.js'));

        const nonce = getNonce();

        return `<!DOCTYPE html>
            <html lang="en">
            <head>
                <meta charset="UTF-8">
                <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource} 'unsafe-inline'; script-src 'nonce-${nonce}';">
                <meta name="viewport" content="width=device-width, initial-scale=1.0">
                <link href="${styleResetUri}" rel="stylesheet">
                <link href="${styleVSCodeUri}" rel="stylesheet">
                <title>Go-On Chat</title>
                <style>
                    .chat-container {
                        height: 100%;
                        display: flex;
                        flex-direction: column;
                        padding: 10px;
                    }
                    .chat-messages {
                        flex: 1;
                        overflow-y: auto;
                        margin-bottom: 10px;
                        border: 1px solid var(--vscode-panel-border);
                        border-radius: 3px;
                        padding: 10px;
                        background: var(--vscode-input-background);
                    }
                    .message {
                        margin-bottom: 10px;
                        padding: 8px;
                        border-radius: 4px;
                    }
                    .message.user {
                        background: var(--vscode-textLink-foreground);
                        color: white;
                        margin-left: 20px;
                    }
                    .message.assistant {
                        background: var(--vscode-editor-background);
                        border: 1px solid var(--vscode-panel-border);
                        margin-right: 20px;
                    }
                    .message.error {
                        background: var(--vscode-notificationsErrorIcon-foreground);
                        color: white;
                    }
                    .message.system {
                        background: var(--vscode-notificationsInfoIcon-foreground);
                        color: white;
                        font-style: italic;
                    }
                    .message.typing {
                        background: var(--vscode-editor-background);
                        border: 1px solid var(--vscode-panel-border);
                        margin-right: 20px;
                        opacity: 0.7;
                    }
                    .message-header {
                        font-weight: bold;
                        margin-bottom: 4px;
                        font-size: 0.9em;
                    }
                    .code-block {
                        position: relative;
                        background: var(--vscode-textCodeBlock-background);
                        border: 1px solid var(--vscode-textCodeBlock-border);
                        border-radius: 3px;
                        padding: 8px;
                        margin: 4px 0;
                        font-family: var(--vscode-editor-font-family);
                        font-size: 0.9em;
                        overflow-x: auto;
                    }
                    .code-block code {
                        font-family: inherit;
                    }
                    .copy-btn, .run-btn {
                        position: absolute;
                        top: 4px;
                        right: 4px;
                        background: var(--vscode-button-background);
                        color: var(--vscode-button-foreground);
                        border: none;
                        border-radius: 3px;
                        padding: 2px 6px;
                        cursor: pointer;
                        font-size: 0.8em;
                    }
                    .run-btn {
                        right: 32px;
                    }
                    .inline-code {
                        background: var(--vscode-textCodeBlock-background);
                        border: 1px solid var(--vscode-textCodeBlock-border);
                        border-radius: 2px;
                        padding: 1px 3px;
                        font-family: var(--vscode-editor-font-family);
                        font-size: 0.9em;
                    }
                    .typing-dots {
                        display: flex;
                        gap: 2px;
                    }
                    .typing-dots span {
                        width: 4px;
                        height: 4px;
                        background: var(--vscode-progressBar-background);
                        border-radius: 50%;
                        animation: typing 1.4s infinite;
                    }
                    .typing-dots span:nth-child(2) { animation-delay: 0.2s; }
                    .typing-dots span:nth-child(3) { animation-delay: 0.4s; }
                    @keyframes typing {
                        0%, 60%, 100% { opacity: 0.3; }
                        30% { opacity: 1; }
                    }
                    .chat-input-container {
                        display: flex;
                        gap: 5px;
                    }
                    .chat-input {
                        flex: 1;
                        padding: 8px;
                        border: 1px solid var(--vscode-input-border);
                        border-radius: 3px;
                        background: var(--vscode-input-background);
                        color: var(--vscode-input-foreground);
                        resize: vertical;
                        min-height: 20px;
                        max-height: 100px;
                    }
                    .chat-send {
                        padding: 8px 12px;
                        background: var(--vscode-button-background);
                        color: var(--vscode-button-foreground);
                        border: none;
                        border-radius: 3px;
                        cursor: pointer;
                    }
                    .chat-send:hover {
                        background: var(--vscode-button-hoverBackground);
                    }
                    .status-bar {
                        padding: 5px;
                        background: var(--vscode-statusBar-background);
                        color: var(--vscode-statusBar-foreground);
                        font-size: 0.8em;
                        text-align: center;
                        border-radius: 3px;
                        margin-bottom: 10px;
                    }
                    .session-controls {
                        display: flex;
                        gap: 5px;
                        margin-bottom: 10px;
                        align-items: center;
                    }
                    .session-select {
                        flex: 1;
                        padding: 4px 8px;
                        border: 1px solid var(--vscode-input-border);
                        border-radius: 3px;
                        background: var(--vscode-dropdown-background);
                        color: var(--vscode-dropdown-foreground);
                    }
                    .session-btn {
                        padding: 4px 8px;
                        background: var(--vscode-button-secondaryBackground);
                        color: var(--vscode-button-secondaryForeground);
                        border: 1px solid var(--vscode-button-border);
                        border-radius: 3px;
                        cursor: pointer;
                        font-size: 0.8em;
                    }
                    .session-btn:hover {
                        background: var(--vscode-button-secondaryHoverBackground);
                    }
                </style>
            </head>
            <body>
                <div class="chat-container">
                    <div class="status-bar" id="status">
                        ${this.manager.isRunning() ? '🟢 Go-On Connected' : '🔴 Go-On Disconnected'}
                    </div>
                    <div class="session-controls">
                        <select class="session-select" id="sessionSelect">
                            <option value="default">default</option>
                        </select>
                        <button class="session-btn" id="newSessionBtn" title="New Session">➕</button>
                        <button class="session-btn" id="clearSessionBtn" title="Clear Session">🗑️</button>
                        <button class="session-btn" id="exportSessionBtn" title="Export Session">📤</button>
                    </div>
                    <div class="chat-messages" id="messages"></div>
                    <div class="chat-input-container">
                        <input type="text" class="chat-input" id="messageInput" placeholder="Type your message..." />
                        <button class="chat-send" id="sendButton">Send</button>
                    </div>
                </div>
                <script nonce="${nonce}" src="${scriptUri}"></script>
            </body>
            </html>`;
    }
}

function getNonce() {
    let text = '';
    const possible = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';
    for (let i = 0; i < 32; i++) {
        text += possible.charAt(Math.floor(Math.random() * possible.length));
    }
    return text;
}