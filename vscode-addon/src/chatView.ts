import * as vscode from "vscode";
import { spawn } from "child_process";

import { RuntimeManagerLike } from "./managerTypes";
import { t, MessageKeys } from "./i18n";

type ChatRole = "user" | "assistant" | "error" | "system";

interface ChatMessage {
  role: ChatRole;
  content: string;
  timestamp: string;
}

export class GoOnChatViewProvider implements vscode.WebviewViewProvider {
  public static readonly viewType = "go-on-chat";
  private _view?: vscode.WebviewView;
  private _messageSubscription?: vscode.Disposable;
  private readonly _executionOutput = vscode.window.createOutputChannel(
    "Go-On Code Execution",
  );
  private _currentSession: string = "default";
  private _sessions: Map<string, ChatMessage[]> = new Map();

  private readonly manager: RuntimeManagerLike;
  private readonly context: vscode.ExtensionContext;
  private readonly onViewResolved?: () => void | Promise<void>;

  constructor(
    private readonly _extensionUri: vscode.Uri,
    _manager: RuntimeManagerLike,
    _context: vscode.ExtensionContext,
    _onViewResolved?: () => void | Promise<void>,
  ) {
    this.manager = _manager;
    this.context = _context;
    this.onViewResolved = _onViewResolved;
    this._loadSessions();
    this.context.subscriptions.push(
      new vscode.Disposable(() => this._messageSubscription?.dispose()),
      this._executionOutput,
    );
  }

  private _loadSessions() {
    const storedSessions = this.context.globalState.get<
      Record<string, unknown>
    >("go-on-chat-sessions", {});
    for (const [sessionName, messages] of Object.entries(storedSessions)) {
      this._sessions.set(
        sessionName,
        Array.isArray(messages) ? (messages as ChatMessage[]) : [],
      );
    }
    // Ensure default session exists
    if (!this._sessions.has("default")) {
      this._sessions.set("default", []);
    }
  }

  private async _saveSessions() {
    const sessionsObject: Record<string, ChatMessage[]> = {};
    for (const [sessionName, messages] of this._sessions) {
      sessionsObject[sessionName] = messages;
    }
    await this.context.globalState.update(
      "go-on-chat-sessions",
      sessionsObject,
    );
  }

  private _getCurrentSessionMessages(): ChatMessage[] {
    return this._sessions.get(this._currentSession) || [];
  }

  private async _addMessageToCurrentSession(message: ChatMessage) {
    const messages = this._getCurrentSessionMessages();
    messages.push(message);
    this._sessions.set(this._currentSession, messages);
    await this._saveSessions();
  }

  private _extractResponseText(result: unknown): string | undefined {
    if (!result || typeof result !== "object") {
      return undefined;
    }
    const candidate = (result as { response?: unknown }).response;
    return typeof candidate === "string" ? candidate : undefined;
  }

  private _getErrorMessage(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
  }

  public resolveWebviewView(
    webviewView: vscode.WebviewView,
    _context: vscode.WebviewViewResolveContext,
    _token: vscode.CancellationToken,
  ) {
    this._view = webviewView;

    webviewView.webview.options = {
      enableScripts: true,
      localResourceRoots: [this._extensionUri],
    };

    webviewView.webview.html = this._getHtmlForWebview(webviewView.webview);

    if (this.onViewResolved) {
      Promise.resolve(this.onViewResolved()).catch((error) => {
        void vscode.window.showWarningMessage(
          t(
            MessageKeys.chatInitFailed,
            error instanceof Error ? error.message : String(error),
          ),
        );
      });
    }

    this._messageSubscription?.dispose();
    this._messageSubscription = webviewView.webview.onDidReceiveMessage(
      async (message) => {
        try {
          switch (message.type) {
            case "sendMessage":
              await this._handleSendMessage(message.text);
              break;
            case "sendMessageWithAttachments":
              await this._handleSendMessage(message.text, message.attachments);
              break;
            case "clearChat":
              await this._clearCurrentSession();
              break;
            case "exportChat":
              this._exportCurrentSession();
              break;
            case "runCode":
              await this._handleRunCode(message.code, message.language);
              break;
            case "newSession":
              await this._createNewSession(message.sessionName);
              break;
            case "switchSession":
              await this._switchSession(message.sessionName);
              break;
            case "showInputBox":
              {
                const value = await vscode.window.showInputBox({
                  prompt: message.prompt,
                  placeHolder: message.placeHolder,
                  value: message.value || "",
                });
                this._view?.webview.postMessage({
                  type: "showInputBoxResult",
                  id: message.id,
                  value,
                });
              }
              break;
            case "showConfirm":
              {
                const selection = await vscode.window.showWarningMessage(
                  message.message,
                  { modal: true },
                  t(MessageKeys.ok),
                  t(MessageKeys.cancel),
                );
                this._view?.webview.postMessage({
                  type: "showConfirmResult",
                  id: message.id,
                  confirmed: selection === t(MessageKeys.ok),
                  workflowId: message.workflowId,
                });
              }
              break;
            case "copyCode":
              await vscode.env.clipboard.writeText(message.code);
              void vscode.window.setStatusBarMessage(
                t(MessageKeys.codeCopied),
                2000,
              );
              break;
            case "getSessions":
              this._sendSessionsList();
              break;
          }
        } catch (error: unknown) {
          const message_text =
            error instanceof Error ? error.message : String(error);
          void vscode.window.showErrorMessage(
            t(MessageKeys.chatError, message_text),
          );
        }
      },
      undefined,
    );
  }

  private async _handleSendMessage(
    text: string,
    attachments?: { name: string; type: string; dataUrl: string }[],
  ) {
    if (!this._view) return;

    try {
      // Add user message to current session
      const userMessage = {
        role: "user",
        content: text,
        timestamp: new Date().toISOString(),
      } as ChatMessage;
      await this._addMessageToCurrentSession(userMessage);

      // Send message to UI
      this._view.webview.postMessage({
        type: "addMessage",
        ...userMessage,
      });

      // Build content array per OpenAI Vision API format
      let messagesPayload: Array<{
        role: string;
        content:
        | string
        | Array<{
          type: string;
          text?: string;
          image_url?: { url: string; detail: string };
          file_data?: { data: string; filename: string; mime_type: string };
        }>;
      }>;
      if (!attachments || attachments.length === 0) {
        // Backward compatible: plain text
        messagesPayload = [{ role: "user", content: text }];
      } else {
        // Multi-modal content with text + images/files
        const content: (
          | { type: "text"; text: string }
          | { type: "image_url"; image_url: { url: string; detail: string } }
          | {
            type: "file";
            file_data: { data: string; filename: string; mime_type: string };
          }
        )[] = [{ type: "text", text }];
        for (const a of attachments) {
          if (a.type && a.type.startsWith("image/")) {
            content.push({
              type: "image_url",
              image_url: { url: a.dataUrl, detail: "auto" },
            });
          } else {
            content.push({
              type: "file",
              file_data: {
                data: a.dataUrl,
                filename: a.name || "attachment",
                mime_type: a.type || "application/octet-stream",
              },
            });
          }
        }
        messagesPayload = [{ role: "user", content }];
      }

      // Send to Go-On
      const result = await this.manager.sendRequest("chat", {
        messages: messagesPayload,
      });
      const responseText = this._extractResponseText(result);

      // Add response to current session
      const assistantMessage = {
        role: "assistant",
        content: responseText || JSON.stringify(result),
        timestamp: new Date().toISOString(),
      } as ChatMessage;
      await this._addMessageToCurrentSession(assistantMessage);

      // Send response to UI
      this._view.webview.postMessage({
        type: "addMessage",
        ...assistantMessage,
      });
    } catch (error: unknown) {
      const errorMsg = this._getErrorMessage(error);

      // Check for provider-not-ready errors and show a more helpful message
      if (
        errorMsg.includes("No runtime-ready AI provider") ||
        errorMsg.includes("providerNotReady")
      ) {
        const systemMessage = {
          role: "system",
          content:
            "⚠️ No API key configured. Please set up an AI provider API key in Go-On Settings to start chatting.",
          timestamp: new Date().toISOString(),
        } as ChatMessage;
        await this._addMessageToCurrentSession(systemMessage);
        this._view.webview.postMessage({
          type: "addMessage",
          ...systemMessage,
        });

        // Open settings automatically with a user prompt
        const action = await vscode.window.showWarningMessage(
          "Go-On needs an AI provider API key to process your request. Open Settings to configure one?",
          "Open Settings",
          "Later",
        );
        if (action === "Open Settings") {
          vscode.commands.executeCommand("go-on.openSettings");
        }
        return;
      }

      const errorMessage = {
        role: "error",
        content: `Error: ${errorMsg}`,
        timestamp: new Date().toISOString(),
      } as ChatMessage;
      await this._addMessageToCurrentSession(errorMessage);

      this._view.webview.postMessage({
        type: "addMessage",
        ...errorMessage,
      });
    }
  }

  public postMessage(message: unknown) {
    this._view?.webview.postMessage(message);
  }

  public createNewSession(sessionName: string) {
    this.postMessage({
      type: "newSession",
      sessionName,
    });
  }

  public switchSession(sessionName: string) {
    this.postMessage({
      type: "switchSession",
      sessionName,
    });
  }

  public dispose() {
    this._messageSubscription?.dispose();
    this._executionOutput?.dispose();
  }

  public clearChat() {
    this._clearCurrentSession();
  }

  public exportChat() {
    this._exportCurrentSession();
  }

  private async _handleRunCode(code: string, language: string) {
    if (!this._view) return;

    try {
      let result = "";

      const approved = await this._confirmCodeExecution(code, language);
      if (!approved) {
        this._executionOutput.appendLine(
          `[blocked] ${new Date().toISOString()} language=${language}`,
        );
        this._view.webview.postMessage({
          type: "codeResult",
          result: t(MessageKeys.executionCanceled),
        });
        return;
      }

      switch (language) {
        case "javascript":
          try {
            const blockedReason = this._validateJavaScriptSnippet(code);
            if (blockedReason) {
              result = `Blocked by safety policy: ${blockedReason}`;
              break;
            }

            // Use safe JSON parsing instead of arbitrary code execution
            try {
              result = JSON.stringify(JSON.parse(code), null, 2);
            } catch {
              result = "⚠️ Only JSON expressions are supported for preview";
            }
          } catch (e: unknown) {
            result = `Error: ${this._getErrorMessage(e)}`;
          }
          break;
        case "python":
          result = await this._executePythonCode(code);
          break;
        case "bash":
        case "shell":
          result = await this._executeShellCode(code);
          break;
        default:
          result = t(MessageKeys.codeExecutionNotSupported, language);
      }

      this._executionOutput.appendLine(
        `[exec] ${new Date().toISOString()} language=${language}`,
      );
      this._executionOutput.appendLine(`[code] ${code.substring(0, 240)}`);
      this._executionOutput.appendLine(`[result] ${result.substring(0, 240)}`);

      this._view.webview.postMessage({
        type: "codeResult",
        result: result,
      });
    } catch (error: unknown) {
      this._view.webview.postMessage({
        type: "codeResult",
        result: t(MessageKeys.executionFailed, this._getErrorMessage(error)),
      });
      this._executionOutput.appendLine(
        `[error] ${new Date().toISOString()} ${this._getErrorMessage(error)}`,
      );
    }
  }

  private async _confirmCodeExecution(
    code: string,
    language: string,
  ): Promise<boolean> {
    const preview = code.trim().replace(/\s+/g, " ").slice(0, 120);
    const executeOption = t(MessageKeys.execute);
    const cancelOption = t(MessageKeys.cancel);
    const choice = await vscode.window.showWarningMessage(
      t(MessageKeys.codeExecutionConfirm, language, preview || "<empty>"),
      { modal: true },
      executeOption,
      cancelOption,
    );

    return choice === executeOption;
  }

  private _validateJavaScriptSnippet(code: string): string | null {
    const dangerousPatterns: Array<{ pattern: RegExp; reason: string }> = [
      { pattern: /\brequire\s*\(/i, reason: "require() is not allowed." },
      { pattern: /\bimport\s+/i, reason: "import is not allowed." },
      { pattern: /\bprocess\b/i, reason: "process access is not allowed." },
      { pattern: /\bglobal\b/i, reason: "global access is not allowed." },
      { pattern: /\beval\s*\(/i, reason: "eval() is not allowed." },
      {
        pattern: /\bFunction\s*\(/i,
        reason: "nested Function constructor is not allowed.",
      },
      {
        pattern: /\bchild_process\b/i,
        reason: "child process modules are not allowed.",
      },
      { pattern: /\bfs\b/i, reason: "filesystem access is not allowed." },
      { pattern: /\bexec\s*\(/i, reason: "exec() is not allowed." },
      { pattern: /\bspawn\s*\(/i, reason: "spawn() is not allowed." },
      { pattern: /\bfork\s*\(/i, reason: "fork() is not allowed." },
      { pattern: /\b__dirname\b/i, reason: "__dirname access is not allowed." },
      {
        pattern: /\b__filename\b/i,
        reason: "__filename access is not allowed.",
      },
      { pattern: /\bmodule\b/i, reason: "module access is not allowed." },
      { pattern: /\bexports\b/i, reason: "exports access is not allowed." },
      { pattern: /\bReflect\b/i, reason: "Reflect API is not allowed." },
      { pattern: /\bProxy\b/i, reason: "Proxy is not allowed." },
    ];

    for (const { pattern, reason } of dangerousPatterns) {
      if (pattern.test(code)) {
        return reason;
      }
    }

    return null;
  }

  private _getExecutionConfig() {
    const config = vscode.workspace.getConfiguration("go-on");
    return {
      pythonPath: config.get<string>("pythonPath", "python"),
      executionTimeout: config.get<number>("execution.timeout", 30000),
      allowedShellPaths: config.get<string[]>(
        "execution.allowedShellPaths",
        [],
      ),
    };
  }

  private _isPathAllowed(shellPath: string): boolean {
    const { allowedShellPaths } = this._getExecutionConfig();
    if (allowedShellPaths.length === 0) {
      return true;
    }
    return allowedShellPaths.some((allowed) => shellPath.startsWith(allowed));
  }

  private async _executePythonCode(code: string): Promise<string> {
    const { pythonPath, executionTimeout } = this._getExecutionConfig();
    return new Promise((resolve) => {
      const pythonProcess = spawn(pythonPath, ["-c", code], {
        cwd: this.context.extensionUri.fsPath,
      });

      let stdout = "";
      let stderr = "";
      let timedOut = false;
      const timeoutHandle = setTimeout(() => {
        timedOut = true;
        pythonProcess.kill("SIGTERM");
      }, executionTimeout);

      pythonProcess.stdout.on("data", (data: Buffer) => {
        stdout += data.toString();
      });

      pythonProcess.stderr.on("data", (data: Buffer) => {
        stderr += data.toString();
      });

      pythonProcess.on("close", (code: number) => {
        clearTimeout(timeoutHandle);
        if (timedOut) {
          resolve(
            `Python execution timed out after ${executionTimeout / 1000} seconds`,
          );
          return;
        }
        if (code === 0) {
          resolve(stdout || "Code executed successfully (no output)");
        } else {
          resolve(`Error (exit code ${code}):\n${stderr || stdout}`);
        }
      });

      pythonProcess.on("error", (error: Error) => {
        clearTimeout(timeoutHandle);
        resolve(
          `Failed to execute Python: ${error.message}\nMake sure Python is installed and in your PATH.`,
        );
      });
    });
  }

  private async _executeShellCode(code: string): Promise<string> {
    const { executionTimeout } = this._getExecutionConfig();
    return new Promise((resolve) => {
      const shell = process.platform === "win32" ? "cmd" : "bash";
      const shellPath =
        process.platform === "win32"
          ? process.env.COMSPEC || "cmd.exe"
          : process.env.SHELL || "/bin/sh";
      const shellArg = process.platform === "win32" ? "/c" : "-c";

      if (!this._isPathAllowed(shellPath)) {
        resolve(
          `Blocked: shell path '${shellPath}' is not in the allowed paths list.`,
        );
        return;
      }

      const shellProcess = spawn(shell, [shellArg, code], {
        cwd: this.context.extensionUri.fsPath,
      });

      let stdout = "";
      let stderr = "";
      let timedOut = false;
      const timeoutHandle = setTimeout(() => {
        timedOut = true;
        shellProcess.kill("SIGTERM");
      }, executionTimeout);

      shellProcess.stdout.on("data", (data: Buffer) => {
        stdout += data.toString();
      });

      shellProcess.stderr.on("data", (data: Buffer) => {
        stderr += data.toString();
      });

      shellProcess.on("close", (code: number) => {
        clearTimeout(timeoutHandle);
        if (timedOut) {
          resolve(
            `Shell execution timed out after ${executionTimeout / 1000} seconds`,
          );
          return;
        }
        if (code === 0) {
          resolve(stdout || "Command executed successfully (no output)");
        } else {
          resolve(`Error (exit code ${code}):\n${stderr || stdout}`);
        }
      });

      shellProcess.on("error", (error: Error) => {
        clearTimeout(timeoutHandle);
        resolve(`Failed to execute shell command: ${error.message}`);
      });
    });
  }

  private async _createNewSession(sessionName: string) {
    if (this._sessions.has(sessionName)) {
      this._view?.webview.postMessage({
        type: "error",
        message: `Session "${sessionName}" already exists`,
      });
      return;
    }

    this._sessions.set(sessionName, []);
    await this._saveSessions();
    await this._switchSession(sessionName);
  }

  private async _switchSession(sessionName: string) {
    if (!this._sessions.has(sessionName)) {
      this._view?.webview.postMessage({
        type: "error",
        message: `Session "${sessionName}" does not exist`,
      });
      return;
    }

    this._currentSession = sessionName;
    const messages = this._getCurrentSessionMessages();

    this._view?.webview.postMessage({
      type: "switchSession",
      sessionName,
      messages,
    });
  }

  private async _clearCurrentSession() {
    this._sessions.set(this._currentSession, []);
    await this._saveSessions();

    this._view?.webview.postMessage({
      type: "clearChat",
    });
  }

  private async _exportCurrentSession() {
    const messages = this._getCurrentSessionMessages();
    const exportData = {
      session: this._currentSession,
      timestamp: new Date().toISOString(),
      messages,
    };

    const doc = await vscode.workspace.openTextDocument({
      content: JSON.stringify(exportData, null, 2),
      language: "json",
    });
    void vscode.window.showTextDocument(doc);
  }

  private _sendSessionsList() {
    const sessions = Array.from(this._sessions.keys());
    this._view?.webview.postMessage({
      type: "sessionsList",
      sessions,
      currentSession: this._currentSession,
    });
  }

  private _getHtmlForWebview(webview: vscode.Webview) {
    const styleResetUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this._extensionUri, "media", "reset.css"),
    );
    const styleVSCodeUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this._extensionUri, "media", "vscode.css"),
    );
    const scriptUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this._extensionUri, "media", "chat.js"),
    );

    const nonce = getNonce();

    return `<!DOCTYPE html>
            <html lang="en">
            <head>
                <meta charset="UTF-8">
                <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource} 'unsafe-inline'; img-src ${webview.cspSource} data:; script-src 'nonce-${nonce}';">
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
                        background: var(--vscode-textCodeBlock-background);
                        border: 1px solid var(--vscode-textCodeBlock-border);
                        border-radius: 3px;
                        margin: 4px 0;
                        overflow: hidden;
                    }
                    .code-block summary {
                        cursor: pointer;
                        padding: 8px 12px;
                        background: var(--vscode-editor-background);
                        user-select: none;
                        list-style: none;
                    }
                    .code-block summary::-webkit-details-marker {
                        display: none;
                    }
                    .code-block summary::before {
                        content: "▸ ";
                        margin-right: 4px;
                    }
                    .code-block[open] summary::before {
                        content: "▾ ";
                    }
                    .code-header {
                        display: flex;
                        justify-content: space-between;
                        align-items: center;
                        width: 100%;
                    }
                    .code-lang {
                        color: var(--vscode-textCodeBlock-foreground);
                        font-family: var(--vscode-editor-font-family);
                        font-size: 0.85em;
                    }
                    .code-actions {
                        display: flex;
                        gap: 6px;
                    }
                    .code-content {
                        margin: 0;
                        padding: 12px;
                        background: var(--vscode-textCodeBlock-background);
                        color: var(--vscode-editor-foreground);
                        overflow-x: auto;
                        font-family: var(--vscode-editor-font-family);
                        font-size: 0.9em;
                        white-space: pre-wrap;
                        word-break: break-word;
                        line-height: 1.5;
                    }
                    .code-block code {
                        font-family: inherit;
                        color: inherit;
                    }
                    .copy-btn, .run-btn {
                        background: var(--vscode-button-background);
                        color: var(--vscode-button-foreground);
                        border: none;
                        border-radius: 3px;
                        padding: 2px 6px;
                        cursor: pointer;
                        font-size: 0.8em;
                    }
                    .copy-btn:hover, .run-btn:hover {
                        background: var(--vscode-button-hoverBackground);
                    }
                    .thinking-block {
                        background: var(--vscode-editor-background);
                        border: 1px solid var(--vscode-input-border);
                        border-radius: 3px;
                        margin: 4px 0;
                        overflow: hidden;
                    }
                    .thinking-block summary {
                        cursor: pointer;
                        padding: 8px 12px;
                        user-select: none;
                        list-style: none;
                        color: var(--vscode-textPreformat-foreground);
                    }
                    .thinking-block summary::-webkit-details-marker {
                        display: none;
                    }
                    .thinking-block summary::before {
                        content: "▸ ";
                        margin-right: 4px;
                    }
                    .thinking-block[open] summary::before {
                        content: "▾ ";
                    }
                    .thinking-toggle {
                        font-weight: 500;
                        font-style: italic;
                    }
                    .thinking-content {
                        margin: 0;
                        padding: 12px;
                        background: var(--vscode-input-background);
                        border-top: 1px solid var(--vscode-input-border);
                        color: var(--vscode-editor-foreground);
                        font-family: var(--vscode-editor-font-family);
                        font-size: 0.85em;
                        white-space: pre-wrap;
                        word-break: break-word;
                        line-height: 1.5;
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
                    .chat-attach {
                        padding: 8px 10px;
                        background: var(--vscode-button-secondaryBackground);
                        color: var(--vscode-button-secondaryForeground);
                        border: 1px solid var(--vscode-button-border);
                        border-radius: 3px;
                        cursor: pointer;
                        font-size: 1em;
                        line-height: 1;
                    }
                    .chat-attach:hover {
                        background: var(--vscode-button-secondaryHoverBackground);
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
                    .attachment-preview {
                        display: flex;
                        align-items: flex-start;
                        gap: 6px;
                        padding: 6px 8px;
                        margin-bottom: 6px;
                        background: var(--vscode-editor-background);
                        border: 1px solid var(--vscode-panel-border);
                        border-radius: 3px;
                    }
                    .attachment-list {
                        display: flex;
                        flex-wrap: wrap;
                        gap: 6px;
                        flex: 1;
                    }
                    .attachment-item {
                        display: flex;
                        align-items: center;
                        gap: 4px;
                        padding: 3px 6px;
                        background: var(--vscode-badge-background);
                        color: var(--vscode-badge-foreground);
                        border-radius: 3px;
                        font-size: 0.8em;
                        max-width: 180px;
                        overflow: hidden;
                        text-overflow: ellipsis;
                        white-space: nowrap;
                    }
                    .attachment-item img.attachment-thumb {
                        width: 24px;
                        height: 24px;
                        object-fit: cover;
                        border-radius: 2px;
                    }
                    .attachment-item .attachment-icon {
                        font-size: 1.1em;
                    }
                    .attachment-item .attachment-name {
                        overflow: hidden;
                        text-overflow: ellipsis;
                        white-space: nowrap;
                    }
                    .attachment-clear {
                        padding: 2px 6px;
                        background: transparent;
                        color: var(--vscode-input-foreground);
                        border: 1px solid var(--vscode-panel-border);
                        border-radius: 3px;
                        cursor: pointer;
                        font-size: 0.9em;
                        line-height: 1;
                        opacity: 0.7;
                    }
                    .attachment-clear:hover {
                        opacity: 1;
                        background: var(--vscode-list-hoverBackground);
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
                        ${this.manager.isRunning() ? "🟢 " + t(MessageKeys.goOnStarted) : "🔴 " + t(MessageKeys.goOnStopped)}
                    </div>
                    <div class="session-controls">
                        <select class="session-select" id="sessionSelect">
                            <option value="default">default</option>
                        </select>
                        <button class="session-btn" id="newSessionBtn" title="${t(MessageKeys.newSession)}">➕</button>
                        <button class="session-btn" id="clearSessionBtn" title="${t(MessageKeys.clearChat)}">🗑️</button>
                        <button class="session-btn" id="exportSessionBtn" title="${t(MessageKeys.export)}">📤</button>
                    </div>
                    <div class="chat-messages" id="messages"></div>
                        <div class="attachment-preview" id="attachmentPreview" style="display:none">
                            <div class="attachment-list" id="attachmentList"></div>
                            <button class="attachment-clear" id="clearAttachmentsBtn">✕</button>
                        </div>
                        <div class="chat-input-container">
                            <input type="file" id="fileInput" accept="image/*,.pdf,.txt,.md" multiple style="display:none" />
                            <button class="chat-attach" id="attachBtn" title="${t(MessageKeys.attachFiles)}">📎</button>
                            <input type="text" class="chat-input" id="messageInput" placeholder="${t(MessageKeys.inputPlaceholder)}" />
                            <button class="chat-send" id="sendButton">${t(MessageKeys.sendMessage)}</button>
                        </div>
                </div>
                <script nonce="${nonce}" src="${scriptUri}"></script>
            </body>
            </html>`;
  }
}

function getNonce() {
  let text = "";
  const possible =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  for (let i = 0; i < 32; i++) {
    text += possible.charAt(Math.floor(Math.random() * possible.length));
  }
  return text;
}
