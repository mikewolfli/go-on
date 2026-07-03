import * as path from "path";
import * as vscode from "vscode";
import { spawn } from "child_process";
import { RuntimeManagerLike } from "./managerTypes";
import { t, MessageKeys } from "./i18n";
import { getErrorMessage } from "./utils";
import { Logger } from "./logger";

const log = Logger.forModule("chatView");
import { getChatHtml } from "./chatHtmlTemplate";

type ChatRole = "user" | "assistant" | "error" | "system";

interface ChatMessage {
  role: ChatRole;
  content: string;
  timestamp: string;
}

/**
 * Manages an in-flight streaming request with abort control and token tracking.
 */
class StreamProcessor {
  private abortController: AbortController | null = null;
  private _tokenCount = 0;
  private manager: RuntimeManagerLike;

  constructor(_manager: RuntimeManagerLike) {
    this.manager = _manager;
  }

  get isActive(): boolean {
    return this.abortController !== null;
  }

  get tokenCount(): number {
    return this._tokenCount;
  }

  /** Start a new streaming session and return the AbortSignal. */
  start(): AbortSignal {
    this.abortController = new AbortController();
    this._tokenCount = 0;
    return this.abortController.signal;
  }

  /** Abort an in-flight stream and send a cancel request to the backend. */
  stop(): void {
    if (this.abortController) {
      this.abortController.abort();
      this.abortController = null;
    }
    if (this.manager.sendCancelRequest) {
      void this.manager.sendCancelRequest();
    }
  }

  /** Increment token count and return the new value. */
  incrementTokens(): number {
    this._tokenCount++;
    return this._tokenCount;
  }

  /** Reset state. */
  reset(): void {
    this.abortController = null;
    this._tokenCount = 0;
  }
}

// V13: Maximum number of chat sessions to keep in storage.
// When exceeded, the least-recently-used sessions are evicted.
const MAX_SESSIONS = 50;

export class GoOnChatViewProvider implements vscode.WebviewViewProvider {
  public static readonly viewType = "go-on-chat";
  private _view?: vscode.WebviewView;
  private _messageSubscription?: vscode.Disposable;
  private readonly _executionOutput = vscode.window.createOutputChannel(
    "Go-On Code Execution",
  );
  private _currentSession: string = "default";
  private _sessions: Map<string, ChatMessage[]> = new Map();
  // V13: Track last-access timestamp (epoch ms) per session for LRU eviction.
  private _sessionLastAccessed: Map<string, number> = new Map();

  private readonly manager: RuntimeManagerLike;
  private readonly context: vscode.ExtensionContext;
  private readonly onViewResolved?: () => void | Promise<void>;
  private readonly streamProcessor: StreamProcessor;

  constructor(
    private readonly _extensionUri: vscode.Uri,
    _manager: RuntimeManagerLike,
    _context: vscode.ExtensionContext,
    _onViewResolved?: () => void | Promise<void>,
  ) {
    this.manager = _manager;
    this.context = _context;
    this.onViewResolved = _onViewResolved;
    this.streamProcessor = new StreamProcessor(_manager);
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
    // V13: Load last-accessed timestamps from storage.
    const storedAccessTimes = this.context.globalState.get<
      Record<string, number>
    >("go-on-chat-sessions-access", {});
    const now = Date.now();
    for (const [sessionName, ts] of Object.entries(storedAccessTimes)) {
      this._sessionLastAccessed.set(sessionName, ts);
    }
    // Ensure default session exists
    if (!this._sessions.has("default")) {
      this._sessions.set("default", []);
    }
    if (!this._sessionLastAccessed.has("default")) {
      this._sessionLastAccessed.set("default", now);
    }
  }

  private async _saveSessions() {
    // V13: Enforce max session count with LRU eviction.
    this._trimSessions();

    const sessionsObject: Record<string, ChatMessage[]> = {};
    for (const [sessionName, messages] of this._sessions) {
      sessionsObject[sessionName] = messages;
    }
    await this.context.globalState.update(
      "go-on-chat-sessions",
      sessionsObject,
    );

    // V13: Persist last-accessed timestamps.
    const accessTimesObject: Record<string, number> = {};
    for (const [sessionName, ts] of this._sessionLastAccessed) {
      accessTimesObject[sessionName] = ts;
    }
    await this.context.globalState.update(
      "go-on-chat-sessions-access",
      accessTimesObject,
    );
  }

  // V13: Remove the least-recently-used sessions when the total exceeds MAX_SESSIONS.
  private _trimSessions(): void {
    if (this._sessions.size <= MAX_SESSIONS) return;

    // Sort sessions by last-accessed time (ascending).
    const sorted = [...this._sessionLastAccessed.entries()].sort(
      ([, a], [, b]) => a - b,
    );

    // Evict oldest sessions until we're within the limit.
    // Always keep the current session.
    const toEvict = this._sessions.size - MAX_SESSIONS;
    let evicted = 0;
    for (const [sessionName] of sorted) {
      if (evicted >= toEvict) break;
      if (sessionName === this._currentSession) continue;
      this._sessions.delete(sessionName);
      this._sessionLastAccessed.delete(sessionName);
      evicted++;
    }
  }

  private _getCurrentSessionMessages(): ChatMessage[] {
    // V13: Update last-accessed time for the current session.
    this._sessionLastAccessed.set(this._currentSession, Date.now());
    return this._sessions.get(this._currentSession) || [];
  }

  private async _addMessageToCurrentSession(message: ChatMessage) {
    const messages = this._getCurrentSessionMessages();
    messages.push(message);
    this._sessions.set(this._currentSession, messages);
    await this._saveSessions();

    // B51-23: After each assistant response, sync checkpoint to backend
    if (message.role === "assistant" && this.manager.isRunning()) {
      try {
        await this.manager.sendRequest("checkpoint.create", {
          session: this._currentSession,
          messages: [message],
        });
      } catch (err) {
        log.warn("checkpoint.create failed:", err);
      }
    }
  }

  private _extractResponseText(result: unknown): string | undefined {
    if (!result || typeof result !== "object") {
      return undefined;
    }
    const candidate = (result as { response?: unknown }).response;
    return typeof candidate === "string" ? candidate : undefined;
  }

  private _handleStopGeneration(): void {
    this.streamProcessor.stop();
  }

  /**
   * Handle an image pasted or dropped in the webview.
   * Logs a notification and provides visual feedback in the status bar.
   * The image is already added to the webview's local attachments array;
   * this method logs the event and shows a VS Code notification.
   */
  private _handlePasteImage(
    name: string,
    mimeType: string,
    _dataUrl: string,
    size: number,
  ): void {
    const sizeKB = (size / 1024).toFixed(1);
    this._executionOutput.appendLine(
      `[pasteImage] Received: ${name} (${mimeType}, ${sizeKB} KB)`,
    );
    // Show a brief status bar notification
    void vscode.window.setStatusBarMessage(
      t(MessageKeys.imagePasted, `${name} (${sizeKB} KB)`),
      3000,
    );
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
            case "pasteImage":
              this._handlePasteImage(
                message.name,
                message.mimeType,
                message.dataUrl,
                message.size,
              );
              break;
            case "stopGeneration":
              this._handleStopGeneration();
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
    // Current mode is request/response (non-streaming).
    // When streaming transport is enabled, this handler should emit
    // incremental token updates to the webview instead of awaiting one final
    // payload.
    if (!this._view) return;

    try {
      // Create user message
      const userMessage = {
        role: "user",
        content: text,
        timestamp: new Date().toISOString(),
      } as ChatMessage;

      // Send message to UI first (sync — no race), then persist.
      // Bug 10: persist only after confirming the view can display it.
      this._view.webview.postMessage({
        type: "addMessage",
        ...userMessage,
      });
      await this._addMessageToCurrentSession(userMessage);

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

      // Send to Go-On — attempt streaming first, fall back to sendRequest
      let responseText: string | undefined;

      if (typeof this.manager.sendStreamingRequest === "function") {
        // Streaming path
        const tokenAccumulator: string[] = [];

        // Start the stream processor first to create the abort signal,
        // then notify the UI — ordering prevents a race where the UI
        // expects a signal that isn't ready yet.
        const signal = this.streamProcessor.start();

        this._view.webview.postMessage({
          type: "streamStart",
        });

        try {
          responseText = await this.manager.sendStreamingRequest(
            "chat",
            { messages: messagesPayload },
            {
              signal,
              callbacks: {
                onToken: (token: string) => {
                  tokenAccumulator.push(token);
                  const count = this.streamProcessor.incrementTokens();
                  // Send incremental token to webview
                  this._view?.webview.postMessage({
                    type: "streamToken",
                    token,
                    tokenCount: count,
                  });
                },
                onDone: () => {
                  this.streamProcessor.reset();
                },
                onError: (error: Error) => {
                  this.streamProcessor.reset();
                  // If we had partial content, keep it; otherwise propagate
                  if (tokenAccumulator.length > 0) {
                    responseText = tokenAccumulator.join("");
                  }
                  throw error;
                },
              },
            },
          );
          // Ensure the stream processor is reset on success
          this.streamProcessor.reset();
        } catch (streamError: unknown) {
          this.streamProcessor.reset();
          // Only re-throw if we have no accumulated content
          const streamErrMsg = getErrorMessage(streamError);
          if (streamErrMsg === "Request aborted") {
            // User stopped — don't add an error message, just return
            this._view?.webview.postMessage({
              type: "streamDone",
              aborted: true,
              content: tokenAccumulator.join(""),
              tokenCount: this.streamProcessor.tokenCount,
            });
            // Still persist whatever was accumulated
            if (tokenAccumulator.length > 0) {
              const partialMessage = {
                role: "assistant" as ChatRole,
                content: tokenAccumulator.join(""),
                timestamp: new Date().toISOString(),
              };
              await this._addMessageToCurrentSession(partialMessage);
            }
            return;
          }
          // Check for provider-not-ready errors
          if (
            streamErrMsg.includes("No runtime-ready AI provider") ||
            streamErrMsg.includes("providerNotReady")
          ) {
            this._view?.webview.postMessage({
              type: "streamError",
              message: streamErrMsg,
            });
            const systemMessage = {
              role: "system" as ChatRole,
              content:
                "⚠️ No API key configured. Please set up an AI provider API key in Go-On Settings to start chatting.",
              timestamp: new Date().toISOString(),
            };
            await this._addMessageToCurrentSession(systemMessage);
            this._view?.webview.postMessage({
              type: "addMessage",
              ...systemMessage,
            });
            const action = await vscode.window.showWarningMessage(
              "Go-On needs an AI provider API key to process your request. Open Settings to configure one?",
              "Open Settings",
              "Later",
            );
            if (action === "Open Settings") {
              Promise.resolve(
                vscode.commands.executeCommand("go-on.openSettings"),
              ).catch((err: unknown) => {
                log.error("Failed to open settings:", err);
              });
            }
            return;
          }
          // Other streaming error — fall through to the bottom error handler
          // by throwing again so the outer catch block handles it uniformly
          if (tokenAccumulator.length > 0) {
            // We have partial content, save it with the error note
            const partialContent = tokenAccumulator.join("");
            this._view?.webview.postMessage({
              type: "streamError",
              message: streamErrMsg,
              content: partialContent,
              tokenCount: this.streamProcessor.tokenCount,
            });
            const partialMessage = {
              role: "assistant" as ChatRole,
              content:
                partialContent + `\n\n*⚠️ Stream interrupted: ${streamErrMsg}*`,
              timestamp: new Date().toISOString(),
            };
            await this._addMessageToCurrentSession(partialMessage);
            return;
          }
          // No content at all — fall through to the outer catch
          throw streamError;
        }

        // Finalize streaming in the UI
        if (responseText !== undefined) {
          this._view?.webview.postMessage({
            type: "streamDone",
            content: responseText,
            tokenCount: this.streamProcessor.tokenCount,
          });

          const assistantMessage = {
            role: "assistant",
            content: responseText,
            timestamp: new Date().toISOString(),
          } as ChatMessage;
          await this._addMessageToCurrentSession(assistantMessage);
        }
      } else {
        // Non-streaming fallback path
        const result = await this.manager.sendRequest("chat", {
          messages: messagesPayload,
        });
        responseText = this._extractResponseText(result);

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
      }
    } catch (error: unknown) {
      const errorMsg = getErrorMessage(error);

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
          Promise.resolve(
            vscode.commands.executeCommand("go-on.openSettings"),
          ).catch((err: unknown) => {
            log.error("Failed to open settings:", err);
          });
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
    try {
      this._view?.webview.postMessage(message);
    } catch (err) {
      log.warn("postMessage failed:", err);
    }
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
    this.streamProcessor.stop();
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
            } catch (err) {
              log.warn("JSON preview parse failed:", err);
              result = "⚠️ Only JSON expressions are supported for preview";
            }
          } catch (e: unknown) {
            result = `Error: ${getErrorMessage(e)}`;
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
        result: t(MessageKeys.executionFailed, getErrorMessage(error)),
      });
      this._executionOutput.appendLine(
        `[error] ${new Date().toISOString()} ${getErrorMessage(error)}`,
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
      allowedShellPaths: config.get<string[]>("execution.allowedShellPaths", [
        "/bin/bash",
        "/bin/sh",
        "/usr/bin/bash",
        "/bin/zsh",
        "/usr/bin/zsh",
        "cmd.exe",
        "C:\\Windows\\System32\\cmd.exe",
      ]),
    };
  }

  private _isPathAllowed(shellPath: string): boolean {
    const { allowedShellPaths } = this._getExecutionConfig();
    if (allowedShellPaths.length === 0) {
      return true;
    }
    const resolved = path.resolve(shellPath);
    return allowedShellPaths.some(
      (allowed) => path.resolve(allowed) === resolved,
    );
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
    let messages = this._getCurrentSessionMessages();

    // B51-23: Try to load messages from backend checkpoint and merge
    if (this.manager.isRunning()) {
      try {
        const remote = await this.manager.sendRequest("checkpoint.load", {
          session: sessionName,
        });
        if (remote && Array.isArray(remote)) {
          const remoteMessages = remote as ChatMessage[];
          // Merge: prefer local messages not yet on the backend
          // If remote has more messages, extend local state
          if (remoteMessages.length > messages.length) {
            messages = remoteMessages;
            this._sessions.set(sessionName, messages);
          }
        }
      } catch (err) {
        log.warn("_switchSession failed:", err);
      }
    }

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
    return getChatHtml(webview, this._extensionUri, this.manager.isRunning());
  }
}
