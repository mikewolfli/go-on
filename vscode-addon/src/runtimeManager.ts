import { ChildProcess, spawn, exec } from "child_process";
import * as os from "os";
import * as vscode from "vscode";
import { i18n, MessageKeys } from "./i18n";
import {
  isAllowedProtocolMode,
  normalizeProtocolMode,
  protocolContract,
} from "./protocolContract";

interface JsonRpcRequest {
  jsonrpc: "2.0";
  id: number;
  method: string;
  params?: unknown;
}

interface JsonRpcResponse {
  jsonrpc: "2.0";
  id: number;
  result?: unknown;
  error?: {
    code: number;
    message: string;
    data?: unknown;
  };
}

interface PendingRequest {
  resolve: (_value: unknown) => void;
  reject: (_reason?: unknown) => void;
}

function asRecord(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null
    ? (value as Record<string, unknown>)
    : {};
}

function secretNameForEnvVar(envVar: string): string {
  const normalized = String(envVar || "").trim();
  if (!normalized) {
    return "";
  }
  if (normalized === "GITHUB_COPILOT_TOKEN") {
    return "github_copilot_token";
  }
  return normalized.toLowerCase();
}

export class GoOnManager {
  private process: ChildProcess | null = null;
  private requestId = 0;
  private pendingRequests = new Map<number, PendingRequest>();
  private statusItems: vscode.TreeItem[] = [];
  private runtimeEnvOverrides: Record<string, string> = {};
  private providerReadyCache?: { checkedAt: number; ready: boolean };
  private lastWizardPromptAt = 0;
  private _outputChannel?: vscode.OutputChannel;
  private stdoutBuffer = "";
  private _reconnectAttempts = 0;
  private readonly maxReconnectAttempts = 3;
  private _shutdownInProgress = false;
  private _closeListener: (() => void) | null = null;
  private _reconnectTimer: ReturnType<typeof setTimeout> | undefined;
  private _startupConfig?: {
    configPath: string;
    executablePath: string;
    cwd: string;
    protocolMode: string;
  };

  /** Connect a VS Code OutputChannel so Go-On process output is visible to users. */
  setOutputChannel(channel: vscode.OutputChannel): void {
    this._outputChannel = channel;
  }

  private classifyRpcErrorKind(message: string, data?: unknown): string {
    const details = asRecord(data);
    const explicit =
      typeof details.kind === "string" ? details.kind : undefined;
    if (explicit && explicit.trim().length > 0) {
      return explicit;
    }

    const lower = String(message || "").toLowerCase();
    if (lower.includes("pua")) return "PuaViolation";
    if (lower.includes("budget")) return "BudgetExceeded";
    if (
      lower.includes("hardening policy denied") ||
      lower.includes("sandbox")
    ) {
      return "SandboxBlocked";
    }
    return "GeneralError";
  }

  private formatRpcError(error: {
    code: number;
    message: string;
    data?: unknown;
  }): string {
    const kind = this.classifyRpcErrorKind(error.message, error.data);
    const errorData = asRecord(error.data);
    const detail = typeof errorData.detail === "string" ? errorData.detail : "";
    const context = detail.includes(
      protocolContract.errors.requestErrorContextPrefix,
    )
      ? protocolContract.errors.requestErrorContextPrefix
      : "none";
    return `rpc_error:${error.code}:${kind}:${error.message} (context=${context})`;
  }

  constructor() {
    this.updateStatus();
  }

  async start(
    configPath: string,
    executablePath: string,
    cwd: string,
    protocolMode: string,
  ): Promise<void> {
    if (this._shutdownInProgress) {
      throw new Error("Go-On is shutting down. Please wait and try again.");
    }
    if (this.process) {
      throw new Error("Go-On is already running");
    }

    // Store config for potential reconnection
    this._startupConfig = { configPath, executablePath, cwd, protocolMode };

    return new Promise((resolve, reject) => {
      let settled = false;
      let resolved = false;
      let stderrBuffer = "";

      const args = ["--config", configPath, "--verbose"];
      const normalizedProtocolMode = normalizeProtocolMode(
        protocolMode || "from_config",
      );
      if (!isAllowedProtocolMode(normalizedProtocolMode)) {
        reject(
          new Error(
            `Invalid protocol mode '${protocolMode}'. Allowed values: from_config, ${protocolContract.protocol.supportedModes.join(", ")}`,
          ),
        );
        return;
      }

      if (normalizedProtocolMode !== "from_config") {
        args.push("--protocol-mode", normalizedProtocolMode);
      }

      this.process = spawn(executablePath, args, {
        cwd,
        env: {
          ...process.env,
          ...this.runtimeEnvOverrides,
        },
        stdio: ["pipe", "pipe", "pipe"],
      });

      let startupTimeout: NodeJS.Timeout | undefined = setTimeout(() => {
        this.process?.kill();
        reject(new Error("Go-On startup timeout"));
      }, 10000);

      this.process.stdout?.on("data", (data: Buffer) => {
        const output = data.toString();
        this._outputChannel?.appendLine(output.trimEnd());

        // Buffered line-frame protocol: accumulate data and split by newlines
        this.stdoutBuffer += output;
        const lines = this.stdoutBuffer.split("\n");
        // Keep the last (potentially incomplete) fragment in the buffer
        this.stdoutBuffer = lines.pop() || "";

        for (const line of lines) {
          const trimmed = line.trim();
          if (!trimmed) {
            continue;
          }
          try {
            const response: JsonRpcResponse = JSON.parse(trimmed);
            const pending = this.pendingRequests.get(response.id);
            if (pending) {
              this.pendingRequests.delete(response.id);
              if (response.error) {
                pending.reject(new Error(this.formatRpcError(response.error)));
              } else {
                pending.resolve(response.result);
              }
            }
          } catch {
            // Not a complete JSON-RPC response yet, wait for more data.
          }
        }

        if (startupTimeout) {
          clearTimeout(startupTimeout);
          startupTimeout = undefined;
          resolved = true;
          resolve();
        }
      });

      this.process.stderr?.on("data", (data: Buffer) => {
        const text = data.toString();
        stderrBuffer += text;
        if (stderrBuffer.length > 4000) {
          stderrBuffer = stderrBuffer.slice(-4000);
        }
        this._outputChannel?.appendLine("[stderr] " + text.trimEnd());
      });

      this.process.on("close", (code: number) => {
        if (settled) return;
        this._outputChannel?.appendLine(`[exit] code ${code}`);
        this._shutdownInProgress = false;
        if (!this._startupConfig) return;
        const failedBeforeStartup = !resolved;
        this.process = null;

        // Reject all pending requests — process exited unexpectedly
        for (const [, pending] of this.pendingRequests) {
          pending.reject(new Error("Go-On process exited unexpectedly"));
        }
        this.pendingRequests.clear();

        this.updateStatus();

        if (startupTimeout) {
          clearTimeout(startupTimeout);
          startupTimeout = undefined;
        }

        if (failedBeforeStartup) {
          settled = true;
          const details = stderrBuffer.trim();
          reject(
            new Error(
              `Go-On process exited prematurely with code ${code}. ${details || "No stderr output."}`,
            ),
          );
        } else {
          // process exited after startup — attempt reconnect
          this._handleProcessExit();
        }
      });

      this.process.on("error", (error: Error) => {
        if (settled) return;
        settled = true;
        clearTimeout(startupTimeout);
        this._outputChannel?.appendLine(`[error] ${error.message}`);
        this.process = null;
        reject(error);
      });
    });
  }

  private async forceKillProcess(proc: ChildProcess): Promise<void> {
    if (os.platform() === "win32") {
      try {
        await new Promise<void>((resolve, reject) => {
          const kill = exec(`taskkill /F /T /PID ${proc.pid}`, (err) => {
            if (err) reject(err);
            else resolve();
          });
          kill.unref();
        });
      } catch {
        proc.kill(); // fallback
      }
    } else {
      proc.kill("SIGKILL");
    }
  }

  stop(): void {
    this._shutdownInProgress = true;
    this._reconnectAttempts = 0;
    // Save startupConfig before clearing, so we can restore it
    // if a concurrent start() call spawned a new process during our await
    const savedConfig = this._startupConfig;
    this._startupConfig = undefined;

    // Clean up timers first
    if (this._reconnectTimer) {
      clearTimeout(this._reconnectTimer);
      this._reconnectTimer = undefined;
    }

    // Save process reference to local variable before clearing
    const proc = this.process;
    this.process = null;

    if (proc) {
      // Graceful shutdown: send SIGTERM first
      proc.kill("SIGTERM");

      // If process doesn't exit within 5 seconds, force kill
      // Use local variable so timer still works after this.process is cleared
      const forceKillTimer = setTimeout(() => {
        if (proc) {
          this._outputChannel?.appendLine(
            "[shutdown] SIGTERM timeout, sending SIGKILL",
          );
          void this.forceKillProcess(proc);
        }
      }, 5000);

      // Remove previous close listener if any (shouldn't happen, but be safe)
      this._closeListener?.();
      const closeHandler = () => {
        clearTimeout(forceKillTimer);
        this._shutdownInProgress = false;
      };
      proc.on("close", closeHandler);
      // Store cleanup so it can be removed if stop() is called again
      this._closeListener = () => {
        proc.off("close", closeHandler);
        this._closeListener = null;
      };
    }

    // If start() already created a new process during our await, restore startupConfig
    if (this.process && !this._startupConfig) {
      this._startupConfig = savedConfig;
    }

    this.updateStatus();
  }

  private _handleProcessExit(): void {
    if (this._shutdownInProgress || !this._startupConfig) return;
    void this.attemptReconnect();
  }

  private _scheduleReconnect(): void {
    this._reconnectTimer = setTimeout(() => {
      void this.attemptReconnect();
    }, 5000);
  }

  private async attemptReconnect(): Promise<void> {
    if (this._shutdownInProgress || !this._startupConfig) return;

    this._reconnectAttempts++;
    if (this._reconnectAttempts > this.maxReconnectAttempts) {
      this._outputChannel?.appendLine(
        `[reconnect] Max reconnect attempts (${this.maxReconnectAttempts}) reached, giving up.`,
      );
      void vscode.window.showWarningMessage(
        i18n.getMessage(MessageKeys.reconnectMaxAttempts, [
          String(this.maxReconnectAttempts),
        ]),
      );
      this._reconnectAttempts = 0;
      return;
    }

    this._outputChannel?.appendLine(
      `[reconnect] Attempt ${this._reconnectAttempts}/${this.maxReconnectAttempts} in 2 seconds...`,
    );

    // Wait before reconnecting
    await new Promise<void>((resolve) => {
      this._reconnectTimer = setTimeout(resolve, 2000);
    });

    // Check again after delay — stop() may have been called
    if (this._shutdownInProgress || !this._startupConfig) return;

    try {
      await this.start(
        this._startupConfig.configPath,
        this._startupConfig.executablePath,
        this._startupConfig.cwd,
        this._startupConfig.protocolMode,
      );
      this._outputChannel?.appendLine(
        `[reconnect] Reconnect attempt ${this._reconnectAttempts} succeeded.`,
      );
      this._reconnectAttempts = 0;
    } catch (error) {
      this._outputChannel?.appendLine(
        `[reconnect] Attempt ${this._reconnectAttempts} failed: ${error}`,
      );
      // Only schedule retry if not shutting down
      if (!this._shutdownInProgress) {
        this._scheduleReconnect();
      }
    }
  }

  isRunning(): boolean {
    return this.process !== null;
  }

  /** Expose reconnect state for diagnostics. */
  getReconnectState(): { attempts: number; maxAttempts: number } {
    return {
      attempts: this._reconnectAttempts,
      maxAttempts: this.maxReconnectAttempts,
    };
  }

  setRuntimeEnvOverrides(overrides: Record<string, string>): void {
    this.runtimeEnvOverrides = {
      ...this.runtimeEnvOverrides,
      ...overrides,
    };
  }

  async sendRequest(
    method: string,
    params?: unknown,
    options?: { skipProviderGuard?: boolean },
  ): Promise<unknown> {
    if (!this.process) {
      throw new Error("Go-On is not running");
    }

    if (!options?.skipProviderGuard && this.requiresAiProvider(method)) {
      const ready = await this.isAnyAiProviderReady();
      if (!ready) {
        await this.notifyAndOpenSetupWizard();
        throw new Error(
          `${protocolContract.errors.providerNotReady} ${protocolContract.errors.setupWizardOpened}`,
        );
      }
    }

    const id = ++this.requestId;
    const request: JsonRpcRequest = {
      jsonrpc: "2.0",
      id,
      method,
      params,
    };

    return new Promise((resolve, reject) => {
      const timeoutId = setTimeout(() => {
        if (this.pendingRequests.has(id)) {
          this.pendingRequests.delete(id);
          reject(new Error("Request timeout"));
        }
      }, 30000);

      this.pendingRequests.set(id, {
        resolve: (v: unknown) => {
          clearTimeout(timeoutId);
          resolve(v);
        },
        reject: (e: unknown) => {
          clearTimeout(timeoutId);
          reject(e);
        },
      });

      const requestStr = JSON.stringify(request) + "\n";
      if (!this.process || !this.process.stdin) {
        reject(new Error("Go-On process not available or stdin not connected"));
        this.pendingRequests.delete(id);
        return;
      }
      const canWrite = this.process.stdin.write(requestStr);
      if (!canWrite) {
        this._outputChannel?.appendLine("[warn] RPC stdin backpressure");
      }
    });
  }

  private requiresAiProvider(method: string): boolean {
    return new Set([
      "chat",
      "workflow.execute",
      "task.plan",
      "task.execute",
      "learning.summary",
      "primary_secondary.summary",
    ]).has(method);
  }

  public async isAnyAiProviderReady(): Promise<boolean> {
    const now = Date.now();
    if (
      this.providerReadyCache &&
      now - this.providerReadyCache.checkedAt < 5000
    ) {
      return this.providerReadyCache.ready;
    }

    try {
      const report = asRecord(
        await this.sendRequest("health.probes", undefined, {
          skipProviderGuard: true,
        }),
      );

      const probes = asRecord(report.probes);
      const dependencies = Array.isArray(probes.dependencies)
        ? (probes.dependencies as unknown[])
        : [];

      const providerComponent = dependencies
        .map((component) => asRecord(component))
        .find((component) => component.name === "provider_dependencies");

      if (!providerComponent) {
        this.providerReadyCache = { checkedAt: now, ready: false };
        return false;
      }

      const details = asRecord(providerComponent.details);
      const ready = Number(details.ready ?? 0);
      const total = Number(details.total ?? 0);
      const isReady = total > 0 && ready > 0;
      this.providerReadyCache = { checkedAt: now, ready: isReady };
      return isReady;
    } catch {
      this.providerReadyCache = { checkedAt: now, ready: false };
      return false;
    }
  }

  private async fetchAvailableProviders(): Promise<Array<{
    name: string;
    label: string;
    description: string;
    detail: string;
    envVar?: string;
    apiKeyEnv?: string;
    secretKeyEnv?: string;
  }> | null> {
    try {
      const catalog = await this.sendRequest(
        "provider.catalog",
        {},
        { skipProviderGuard: true },
      );
      const result = catalog as Record<string, unknown>;
      const providers = result?.providers as
        | Array<Record<string, unknown>>
        | undefined;
      if (!Array.isArray(providers)) return null;

      return providers.map((p: Record<string, unknown>) => ({
        name: String(p.name ?? ""),
        label:
          String(p.name ?? "")
            .charAt(0)
            .toUpperCase() + String(p.name ?? "").slice(1),
        description: String(p.agent_type || p.url || ""),
        detail: `Model: ${p.model || "auto"} | Env: ${p.api_key_env || p.secret_key_env || "N/A"}`,
        envVar: String(p.api_key_env || p.secret_key_env || ""),
        apiKeyEnv: String(p.api_key_env || ""),
        secretKeyEnv: String(p.secret_key_env || ""),
      }));
    } catch {
      return null;
    }
  }

  private async notifyAndOpenSetupWizard(): Promise<void> {
    const now = Date.now();
    if (now - this.lastWizardPromptAt < 5000) {
      return;
    }
    this.lastWizardPromptAt = now;

    const action = await vscode.window.showWarningMessage(
      "Go-On needs an AI provider API key to process your request.",
      "Quick Setup",
      "Open Settings",
      "Later",
    );

    if (action === "Later") return;

    if (action === "Open Settings") {
      await vscode.commands.executeCommand("go-on.openSettings");
      return;
    }

    // === Quick Setup flow ===

    // Step 1: Pick provider
    const providers = await this.fetchAvailableProviders();
    if (!providers || providers.length === 0) {
      vscode.window.showErrorMessage(
        "No AI providers found in your configuration.",
      );
      return;
    }

    const providerItems = providers.map((p) => ({
      label: p.label,
      description: p.description,
      detail: p.detail,
      provider: p,
    }));

    const picked = await vscode.window.showQuickPick(providerItems, {
      placeHolder: "Select an AI provider to configure",
      title: "Quick Setup — Step 1/2",
    });

    if (!picked) return;
    const selectedProvider = picked.provider;

    // Step 2: Enter API key
    const apiKey = await vscode.window.showInputBox({
      prompt: `Enter your API key for ${selectedProvider.label}`,
      password: true,
      placeHolder: "sk-...",
      validateInput: (value: string) => {
        if (!value || value.trim().length < 4) {
          return "API key must be at least 4 characters";
        }
        return null;
      },
    });

    if (!apiKey || apiKey.trim().length < 4) return;

    let secretKey = "";
    if (selectedProvider.secretKeyEnv) {
      const providedSecret = await vscode.window.showInputBox({
        prompt: `Enter your secret key for ${selectedProvider.label}`,
        password: true,
        placeHolder: "secret-...",
        validateInput: (value: string) => {
          if (!value || value.trim().length < 4) {
            return "Secret key must be at least 4 characters";
          }
          return null;
        },
      });

      if (!providedSecret || providedSecret.trim().length < 4) return;
      secretKey = providedSecret.trim();
    }

    // Step 3: Save to keyring and configure
    try {
      // Save to keyring
      const envVarName = selectedProvider.envVar || selectedProvider.apiKeyEnv;
      if (envVarName) {
        await vscode.commands.executeCommand("go-on.keyringSet", {
          name: secretNameForEnvVar(envVarName),
          value: apiKey.trim(),
        });
      }

      if (selectedProvider.secretKeyEnv && secretKey) {
        await vscode.commands.executeCommand("go-on.keyringSet", {
          name: secretNameForEnvVar(selectedProvider.secretKeyEnv),
          value: secretKey,
        });
      }

      // If provider has a known env var, also set it as runtime env override
      const envOverrides: Record<string, string> = {};
      if (selectedProvider.envVar) {
        envOverrides[selectedProvider.envVar] = apiKey.trim();
      }
      if (selectedProvider.secretKeyEnv && secretKey) {
        envOverrides[selectedProvider.secretKeyEnv] = secretKey;
      }
      if (Object.keys(envOverrides).length > 0) {
        this.setRuntimeEnvOverrides({
          ...envOverrides,
        });
      }

      // Save provider selection to config.toml
      if (selectedProvider.name) {
        await vscode.commands.executeCommand(
          "go-on.applyDefaultConfigTemplate",
          {
            template: "config.toml.autopilot-adaptive",
          },
        );
      }

      vscode.window.showInformationMessage(
        `✅ ${selectedProvider.label} API key configured! You can now send messages.`,
      );
    } catch (error: unknown) {
      const msg = error instanceof Error ? error.message : String(error);
      vscode.window.showErrorMessage(
        `Setup failed: ${msg}. Try Open Settings instead.`,
      );
    }
  }

  private updateStatus(): void {
    this.statusItems = [
      new vscode.TreeItem(
        `Status: ${this.isRunning() ? "Running" : "Stopped"}`,
        vscode.TreeItemCollapsibleState.None,
      ),
    ];
    vscode.commands.executeCommand("go-on-status.refresh");
    vscode.commands.executeCommand("go-on.refreshStatusMonitor");
  }

  getStatusItems(): vscode.TreeItem[] {
    return this.statusItems;
  }
}

export class GoOnStatusProvider implements vscode.TreeDataProvider<vscode.TreeItem> {
  private _onDidChangeTreeData: vscode.EventEmitter<
    vscode.TreeItem | undefined | null | void
  > = new vscode.EventEmitter<vscode.TreeItem | undefined | null | void>();
  readonly onDidChangeTreeData: vscode.Event<
    vscode.TreeItem | undefined | null | void
  > = this._onDidChangeTreeData.event;
  private manager: GoOnManager;

  constructor(_manager: GoOnManager) {
    this.manager = _manager;
  }

  refresh(): void {
    this._onDidChangeTreeData.fire();
  }

  getTreeItem(element: vscode.TreeItem): vscode.TreeItem {
    return element;
  }

  getChildren(element?: vscode.TreeItem): Thenable<vscode.TreeItem[]> {
    if (!element) {
      return Promise.resolve(this.manager.getStatusItems());
    }
    return Promise.resolve([]);
  }
}
