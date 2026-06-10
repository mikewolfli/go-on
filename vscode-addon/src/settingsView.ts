import * as vscode from "vscode";
import { spawn } from "child_process";
import * as fs from "fs/promises";
import * as path from "path";
import { Logger } from "./logger";
import { i18n, MessageKeys } from "./i18n";

const log = Logger.forModule("settingsView");
import { configManager } from "./configManager";
import { RuntimeManagerLike } from "./managerTypes";
import { normalizeProtocolMode } from "./protocolContract";
import { ensureGoOnBinary } from "./runtimeBinaryService";
import { asRecord, getErrorMessage } from "./utils";
import { getSettingsHtml, getConfigWizardHtml } from "./settingsHtmlTemplate";
import { secretNameForEnvVar } from "./runtime/jsonRpc";
import {
  ProviderCatalogSpec,
  ProviderCatalogEntry,
  ProviderConfigSnapshot,
  ProviderSecretTarget,
  BUILTIN_PROVIDER_CATALOG,
  asCatalogSpec,
  dedupeCatalog,
  inferEnvVar,
  collectProviderSecretTargets,
} from "./settings/providerCatalog";
import {
  PersistedCopilotState,
  CopilotAuthState,
  ProviderModelResolution,
  CopilotTokenExchange,
  CopilotModelCache,
  PendingCopilotDeviceAuth,
  DeviceCodeResponse,
  COPILOT_ENV_VAR,
  COPILOT_SECRET_NAME,
  COPILOT_TOKEN_URL,
  COPILOT_MODELS_URL,
  GITHUB_DEVICE_CODE_URL,
  GITHUB_ACCESS_TOKEN_URL,
  COPILOT_MODEL_CACHE_KEY,
  COPILOT_STATE_KEY,
  errorMessage,
  requestJson,
  escapeRegex,
} from "./settings/copilotAuth";

// Types, constants, and utility functions moved to
// settings/providerCatalog.ts and settings/copilotAuth.ts

function parseConfiguredAgents(
  content: string,
): Map<string, ProviderConfigSnapshot> {
  const result = new Map<string, ProviderConfigSnapshot>();
  const sectionRegex = /^\[agents\.([^\]]+)\]([\s\S]*?)(?=^\[[^\]]+\]|$)/gm;

  for (const match of content.matchAll(sectionRegex)) {
    const name = String(match[1] || "").trim();
    const section = String(match[2] || "");
    const model = section.match(/^model\s*=\s*"([^"]*)"\s*$/m)?.[1];
    const apiKeyEnv = section.match(/^api_key_env\s*=\s*"([^"]*)"\s*$/m)?.[1];
    const secretKeyEnv = section.match(
      /^secret_key_env\s*=\s*"([^"]*)"\s*$/m,
    )?.[1];

    result.set(name, {
      model,
      envVar: apiKeyEnv || secretKeyEnv,
    });
  }

  return result;
}

function formatTomlValue(value: unknown): string {
  if (typeof value === "string") {
    return `"${value.replace(/"/g, '\\"')}"`;
  }
  if (typeof value === "boolean" || typeof value === "number") {
    return String(value);
  }
  return `"${String(value)}"`;
}

function upsertSectionLine(
  section: string,
  key: string,
  value: unknown,
): string {
  const keyRegex = new RegExp(`^${escapeRegex(key)}\\s*=.*$`, "m");
  const line = `${key} = ${formatTomlValue(value)}`;
  if (keyRegex.test(section)) {
    return section.replace(keyRegex, line);
  }
  const lines = section.split("\n");
  lines.splice(1, 0, line);
  return lines.join("\n");
}

function removeSectionLine(section: string, key: string): string {
  const keyRegex = new RegExp(`^${escapeRegex(key)}\\s*=.*(?:\\r?\\n)?`, "m");
  return section.replace(keyRegex, "");
}

function upsertAgentSection(
  content: string,
  providerName: string,
  fields: Record<string, unknown>,
): string {
  const header = `[agents.${providerName}]`;
  const sectionRegex = new RegExp(
    `^${escapeRegex(header)}[\\s\\S]*?(?=^\\[[^\\]]+\\]|\\Z)`,
    "m",
  );

  const applyFields = (section: string) => {
    let updated = section;
    for (const [key, value] of Object.entries(fields)) {
      if (value === undefined || value === null || value === "") {
        updated = removeSectionLine(updated, key);
      } else {
        updated = upsertSectionLine(updated, key, value);
      }
    }
    return updated;
  };

  if (sectionRegex.test(content)) {
    return content.replace(sectionRegex, (section) => applyFields(section));
  }

  let section = `${header}\n`;
  for (const [key, value] of Object.entries(fields)) {
    if (value !== undefined && value !== null && value !== "") {
      section += `${key} = ${formatTomlValue(value)}\n`;
    }
  }

  return `${content.trimEnd()}\n\n${section}`;
}

function upsertPhaseAgents(
  content: string,
  phase: string,
  agents: string[],
): string {
  const header = `[phases.${phase}]`;
  const sectionRegex = new RegExp(
    `^${escapeRegex(header)}[\\s\\S]*?(?=^\\[[^\\]]+\\]|\\Z)`,
    "m",
  );
  const agentsLine = `agents = [${agents.map((agent) => `"${agent}"`).join(", ")}]`;

  if (sectionRegex.test(content)) {
    return content.replace(sectionRegex, (section) => {
      const agentsRegex = /^agents\s*=\s*\[[^\]]*\]\s*$/m;
      if (agentsRegex.test(section)) {
        return section.replace(agentsRegex, agentsLine);
      }
      const lines = section.split("\n");
      lines.splice(1, 0, agentsLine);
      return lines.join("\n");
    });
  }

  return `${content.trimEnd()}\n\n${header}\ndescription = "${phase} phase"\n${agentsLine}\nfallback = true\n`;
}

export class GoOnSettingsViewProvider implements vscode.WebviewViewProvider {
  public static readonly viewType = "go-on-settings";
  private _view?: vscode.WebviewView;
  private _runtimeFeatures: Record<string, boolean> = {};
  private _messageSubscription?: vscode.Disposable;
  private _pendingCopilotDeviceAuth?: PendingCopilotDeviceAuth;
  private readonly manager: RuntimeManagerLike;
  private readonly context: vscode.ExtensionContext;
  private readonly _commandMessageMap: Record<string, string> = {
    startGoOn: "go-on.start",
    stopGoOn: "go-on.stop",
    healthCheck: "go-on.healthCheck",
    healthProbes: "go-on.healthProbes",
    clearCache: "go-on.cacheClear",
    breakerStatus: "go-on.breakerStatus",
    breakerRecovery: "go-on.breakerRecovery",
    observabilityAlerts: "go-on.observabilityAlerts",
    securityBaseline: "go-on.securityBaseline",
    harnessStatus: "go-on.harnessStatus",
    clearVector: "go-on.vectorClear",
    reloadConfig: "go-on.configReload",
    workflowExecute: "go-on.workflowExecute",
    taskPlan: "go-on.taskPlan",
    taskExecute: "go-on.taskExecute",
    learningSummary: "go-on.learningSummary",
    learningGuardrail: "go-on.learningGuardrail",
    learningReplay: "go-on.learningReplay",
    knowledgeDistill: "go-on.knowledgeDistill",
    rlAlignmentEval: "go-on.rlAlignmentEval",
    hardnessStatus: "go-on.hardnessStatus",
    costStatus: "go-on.costStatus",
    configBaseline: "go-on.configBaseline",
    errorContract: "go-on.errorContract",
    buildRepro: "go-on.buildRepro",
    dataLifecycle: "go-on.dataLifecycle",
    optimizationPeak: "go-on.optimizationPeak",
    releaseReadiness: "go-on.releaseReadiness",
    runtimeStability: "go-on.runtimeStability",
    autotuneStatus: "go-on.autotuneStatus",
    governanceStatus: "go-on.governanceStatus",
    governancePlanGet: "go-on.governancePlanGet",
    governanceAuditRecent: "go-on.governanceAuditRecent",
    lockStatus: "go-on.lockStatus",
    debugPanelGet: "go-on.debugPanelGet",
    actionCheck: "go-on.actionCheck",
  };

  constructor(
    private readonly _extensionUri: vscode.Uri,
    _manager: RuntimeManagerLike,
    _context: vscode.ExtensionContext,
  ) {
    this.manager = _manager;
    this.context = _context;
    this.context.subscriptions.push(
      new vscode.Disposable(() => this._messageSubscription?.dispose()),
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

    webviewView.webview.html = getSettingsHtml(
      webviewView.webview,
      this._extensionUri,
      this.manager.isRunning(),
    );

    this._messageSubscription?.dispose();
    this._messageSubscription = webviewView.webview.onDidReceiveMessage(
      async (message: Record<string, unknown>) => {
        try {
          await this._handleWebviewMessage(message);
        } catch (error: unknown) {
          this._postMessage({
            type: "settingsActionError",
            message: getErrorMessage(error),
          });
        }
      },
      undefined,
    );

    // Bug 8: Hook into onDidDispose to cancel polling when webview is closed.
    // Without this, Copilot device auth continues making HTTP requests
    // after the settings webview has been disposed.
    webviewView.onDidDispose(() => {
      if (this._pendingCopilotDeviceAuth) {
        this._pendingCopilotDeviceAuth.cancelRequested = true;
      }
    });

    this._sendCurrentSettings();
    // Request webview to scroll to credentials section
    this._postMessage({ type: "focusCredentials" });
    if (this.manager.isRunning?.()) {
      this._refreshRuntimeFeatures().catch(() => undefined);
    }
  }

  private async _refreshRuntimeFeatures(): Promise<void> {
    try {
      const response = (await this.manager.sendRequest(
        "runtime.features",
        {},
      )) as Record<string, unknown>;
      const features = (
        typeof response === "object" && response !== null
          ? response["features"]
          : undefined
      ) as Record<string, boolean> | undefined;
      if (features && typeof features === "object") {
        this._runtimeFeatures = features;
        this._view?.webview.postMessage({
          type: "runtimeFeatures",
          features: this._runtimeFeatures,
        });
      }
    } catch (err) {
      log.warn("_refreshRuntimeFeatures failed:", err);
    }
  }

  private async _handleWebviewMessage(message: Record<string, unknown>) {
    const messageType = String(message.type ?? "");
    const handlers: Record<
      string,
      (_msg: Record<string, unknown>) => Promise<void> | void
    > = {
      requestSettings: async (_message) => this._sendCurrentSettings(),
      openConfigWizard: async () => this.showConfigWizard(),
      updateSetting: async (msg) =>
        this._handleGenericSettingUpdate(String(msg.key ?? ""), msg.value),
      updateRuntimeSetting: async (msg) =>
        this._updateRuntimeSetting(String(msg.key ?? ""), msg.value),
      updateCacheSetting: async (msg) =>
        this._updateCacheSetting(String(msg.key ?? ""), msg.value),
      updateVectorSetting: async (msg) =>
        this._updateVectorSetting(String(msg.key ?? ""), msg.value),
      updateAutotuneSetting: async (msg) =>
        this._updateAutotuneSetting(String(msg.key ?? ""), msg.value),
      requestProviderModels: async (msg) =>
        this._sendProviderModels(String(msg.provider ?? "")),
      authorizeCopilotGitHubSession: async () =>
        this._authorizeCopilotWithGitHubSession(),
      authorizeCopilotDeviceFlow: async (msg) =>
        this._startCopilotDeviceAuthorization(String(msg.oauthClientId ?? "")),
      cancelCopilotDeviceFlow: async () =>
        this._cancelCopilotDeviceAuthorization(),
      deleteCopilotAuthorization: async () =>
        this._deleteCopilotAuthorization(),
      saveProviderSelection: async (msg) =>
        this._saveProviderSelection(
          String(msg.provider ?? ""),
          String(msg.model ?? "auto"),
          msg.envVar ? String(msg.envVar) : undefined,
        ),
      addAgent: async (msg) =>
        this._addAgent(String(msg.name ?? ""), msg.config),
      deleteAgent: async (msg) => this._deleteAgent(String(msg.name ?? "")),
      updatePhase: async (msg) =>
        this._updatePhase(String(msg.name ?? ""), msg.config),
      setLanguage: async (msg) => this._setLanguage(String(msg.language ?? "")),
      setKeyringSecret: async (msg) =>
        this._handleKeyringSet(String(msg.name ?? ""), String(msg.value ?? "")),
      getKeyringSecret: async (msg) =>
        this._handleKeyringGet(String(msg.name ?? "")),
      deleteKeyringSecret: async (msg) =>
        this._handleKeyringDelete(String(msg.name ?? "")),
      listKeyringSecrets: async () => this._handleKeyringList(),
      quickSetupProvider: async (msg) =>
        this._handleQuickSetupProvider(
          String(msg.provider ?? ""),
          String(msg.apiKey ?? ""),
        ),
      applyDefaultConfigTemplate: async (msg) =>
        this._handleApplyDefaultConfigTemplate(String(msg.template ?? "")),
      applyRulesSettings: async (msg) =>
        this._handleApplyRulesSettings(
          (msg.payload as {
            globalRules?: string[];
            commonRules?: string[];
            phaseRules?: Record<string, string[]>;
          }) || {},
        ),
      applyWorkflowMapping: async (msg) =>
        this._handleApplyWorkflowMapping(
          (msg.payload as {
            defaultPhase?: string;
            phases?: Record<
              string,
              {
                agents: string[];
                fallback?: boolean;
                principles?: string[];
                switchRules?: {
                  circuitBreakerFailures?: number;
                  circuitBreakerOpenSeconds?: number;
                };
              }
            >;
          }) || {},
        ),
    };

    const messageHandler = handlers[messageType];
    if (messageHandler) {
      await messageHandler(message);
      return;
    }

    const command = this._commandMessageMap[messageType];
    if (command) {
      await vscode.commands.executeCommand(command);
      return;
    }
  }

  private async _handleGenericSettingUpdate(key: string, value: unknown) {
    if (!key.startsWith("go-on.")) {
      return;
    }

    const goOnConfig = vscode.workspace.getConfiguration("go-on");
    const relativeKey = key.replace(/^go-on\./, "");

    // Keys with runtime./cache./vector./autotune. prefixes are TOML-only settings;
    // skip the VS Code config update to avoid dual-write race conditions.
    const isTomlOnly =
      relativeKey.startsWith("runtime.") ||
      relativeKey.startsWith("cache.") ||
      relativeKey.startsWith("vector.") ||
      relativeKey.startsWith("autotune.");

    if (!isTomlOnly) {
      await goOnConfig.update(
        relativeKey,
        value,
        vscode.ConfigurationTarget.Workspace,
      );
    }

    if (relativeKey.startsWith("runtime.")) {
      await this._updateRuntimeSetting(
        relativeKey.replace(/^runtime\./, ""),
        value,
      );
      return;
    }
    if (relativeKey.startsWith("cache.")) {
      await this._updateCacheSetting(
        relativeKey.replace(/^cache\./, ""),
        value,
      );
      return;
    }
    if (relativeKey.startsWith("vector.")) {
      await this._updateVectorSetting(
        relativeKey.replace(/^vector\./, ""),
        value,
      );
      return;
    }
    if (relativeKey.startsWith("autotune.")) {
      await this._updateAutotuneSetting(
        relativeKey.replace(/^autotune\./, ""),
        value,
      );
      return;
    }

    vscode.window.showInformationMessage(
      i18n.getMessage(MessageKeys.successfullySaved),
    );
  }

  private _workspaceRoot(): string {
    const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    if (!root) {
      throw new Error("No workspace folder open.");
    }
    return root;
  }

  private _resolveConfigPath(): string {
    const root = this._workspaceRoot();
    const configured =
      vscode.workspace
        .getConfiguration("go-on")
        .get<string>("configPath", "./config.toml") || "./config.toml";
    return path.isAbsolute(configured)
      ? configured
      : path.resolve(root, configured);
  }

  /**
   * Run a go-on secret command with secure stdin piping and timeout handling.
   *
   * SECURITY: Secret values are written to stdin instead of passing as
   * --secret-value CLI argument, so they are not visible in /proc/PID/cmdline
   * or ps aux. A 10-second timeout with two-stage kill (SIGTERM -> SIGKILL)
   * prevents hanging processes.
   *
   * NOTE: This shares the same security posture as the duplicate in extension.ts.
   * Both should be consolidated into a shared utility in a future refactor.
   */
  private async _runSecretCommand(
    action: "set" | "get" | "delete",
    secretName: string,
    secretValue?: string,
  ): Promise<string> {
    const config = vscode.workspace.getConfiguration("go-on");
    const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    const runtime = await ensureGoOnBinary(workspaceRoot, config, this.context);

    const args: string[] = ["--secret", action, "--secret-name", secretName];
    // SECURITY: Do NOT pass secretValue as --secret-value CLI arg.
    // Pipe it through stdin instead so it doesn't leak in process listings.
    const hasSecretValue = secretValue !== undefined;

    return new Promise<string>((resolve, reject) => {
      const proc = spawn(runtime.executablePath, args, {
        cwd: workspaceRoot || runtime.runtimeDir,
        stdio: [hasSecretValue ? "pipe" : "ignore", "pipe", "pipe"],
      });

      // Pipe secret through stdin to avoid CLI arg exposure
      if (hasSecretValue && proc.stdin) {
        proc.stdin.write(secretValue!);
        proc.stdin.end();
      } else if (proc.stdin) {
        proc.stdin.end();
      }

      let stdout = "";
      let stderr = "";
      let timedOut = false;

      // 10-second timeout with two-stage kill
      const timeoutHandle = setTimeout(() => {
        timedOut = true;
        proc.kill("SIGTERM");
        setTimeout(() => {
          try {
            if (!proc.killed) {
              proc.kill("SIGKILL");
            }
          } catch (err) {
            log.warn("forceKill failed:", err);
          }
        }, 1000);
      }, 10000);

      proc.stdout?.on("data", (chunk: Buffer) => {
        stdout += chunk.toString();
      });

      proc.stderr?.on("data", (chunk: Buffer) => {
        stderr += chunk.toString();
      });

      proc.on("error", (err) => {
        clearTimeout(timeoutHandle);
        reject(err);
      });
      proc.on("close", (code) => {
        clearTimeout(timeoutHandle);
        if (timedOut) {
          const details = (stderr || stdout || "process timed out").trim();
          reject(new Error(`go-on secret command timed out: ${details}`));
          return;
        }
        if (code === 0) {
          resolve(stdout.trim());
          return;
        }
        const details = (stderr || stdout || `exit code ${code}`).trim();
        reject(new Error(`go-on secret command failed: ${details}`));
      });
    });
  }

  private async _readCopilotToken(): Promise<string | undefined> {
    try {
      const token = await this._runSecretCommand("get", COPILOT_SECRET_NAME);
      return token.trim() || undefined;
    } catch (err) {
      log.warn("_readCopilotToken failed:", err);
      return undefined;
    }
  }

  private async _writeCopilotToken(token: string): Promise<void> {
    await this._runSecretCommand("set", COPILOT_SECRET_NAME, token);
    this.manager.setRuntimeEnvOverrides?.({
      [COPILOT_ENV_VAR]: token,
    });
  }

  private _loadPersistedCopilotState(): PersistedCopilotState {
    return (
      this.context.workspaceState.get<PersistedCopilotState>(
        COPILOT_STATE_KEY,
      ) || {}
    );
  }

  private async _updatePersistedCopilotState(
    patch: Partial<PersistedCopilotState>,
  ): Promise<void> {
    await this.context.workspaceState.update(COPILOT_STATE_KEY, {
      ...this._loadPersistedCopilotState(),
      ...patch,
    });
  }

  private _baseCopilotAuthState(
    partial?: Partial<CopilotAuthState>,
  ): CopilotAuthState {
    const stored = this._loadPersistedCopilotState();
    return {
      isAuthorized: false,
      authMode: stored.authMode || "none",
      accountLabel: stored.accountLabel || "",
      oauthClientId: stored.oauthClientId || "",
      pending: false,
      statusMessage: stored.lastStatus || "",
      lastError: stored.lastError || "",
      ...partial,
    };
  }

  private async _currentCopilotAuthState(
    partial?: Partial<CopilotAuthState>,
  ): Promise<CopilotAuthState> {
    const token = await this._readCopilotToken();
    const pending = this._pendingCopilotDeviceAuth;
    const state = this._baseCopilotAuthState({
      isAuthorized: Boolean(token),
      pending: Boolean(pending),
      userCode: pending?.userCode,
      verificationUri: pending?.verificationUri,
      expiresAt: pending?.expiresAt,
      ...partial,
    });

    if (!state.statusMessage) {
      state.statusMessage = state.isAuthorized
        ? "GitHub token is stored and ready for Copilot exchange."
        : "Authorize GitHub Copilot to fetch models and enable runtime requests.";
    }

    return state;
  }

  private async _postCopilotAuthState(
    partial?: Partial<CopilotAuthState>,
  ): Promise<void> {
    this._postMessage({
      type: "copilotAuthState",
      auth: await this._currentCopilotAuthState(partial),
    });
  }

  private async _exchangeCopilotToken(
    githubToken: string,
  ): Promise<CopilotTokenExchange> {
    const response = await requestJson(COPILOT_TOKEN_URL, {
      headers: {
        Authorization: `token ${githubToken}`,
        "User-Agent": "go-on-vscode/1.0",
      },
    });

    if (response.status < 200 || response.status >= 300) {
      throw new Error(
        `Copilot token exchange failed (${response.status}): ${response.bodyText || "empty response"}`,
      );
    }

    const body = asRecord(response.body);
    const token = typeof body.token === "string" ? body.token.trim() : "";
    if (!token) {
      throw new Error("Copilot token exchange returned no token field.");
    }

    return {
      token,
      expiresAt:
        typeof body.expires_at === "number"
          ? body.expires_at
          : Math.floor(Date.now() / 1000) + 1500,
    };
  }

  private _extractCopilotModelIds(payload: unknown): string[] {
    const result = new Set<string>();
    const root = asRecord(payload);
    const candidates = Array.isArray(payload)
      ? payload
      : Array.isArray(root.data)
        ? root.data
        : Array.isArray(root.models)
          ? root.models
          : [];

    for (const candidate of candidates) {
      if (typeof candidate === "string" && candidate.trim()) {
        result.add(candidate.trim());
        continue;
      }
      const record = asRecord(candidate);
      const fields = [record.id, record.model, record.model_id, record.name];
      for (const field of fields) {
        if (typeof field === "string" && field.trim()) {
          result.add(field.trim());
          break;
        }
      }
    }

    return Array.from(result.values()).sort((left, right) =>
      left.localeCompare(right),
    );
  }

  private _loadCopilotModelCache(): CopilotModelCache | undefined {
    return this.context.workspaceState.get<CopilotModelCache>(
      COPILOT_MODEL_CACHE_KEY,
    );
  }

  private async _storeCopilotModelCache(models: string[]): Promise<void> {
    await this.context.workspaceState.update(COPILOT_MODEL_CACHE_KEY, {
      models,
      fetchedAt: Date.now(),
    });
  }

  private async _resolveCopilotModels(): Promise<ProviderModelResolution> {
    const githubToken = await this._readCopilotToken();
    if (!githubToken) {
      const cached = this._loadCopilotModelCache();
      return {
        modelOptions: cached?.models || [],
        copilotAuth: await this._currentCopilotAuthState({
          modelSource: cached?.models?.length ? "cache" : "config",
          modelCount: cached?.models?.length || 0,
        }),
      };
    }

    try {
      const copilotToken = await this._exchangeCopilotToken(githubToken);
      const response = await requestJson(COPILOT_MODELS_URL, {
        headers: {
          Authorization: `Bearer ${copilotToken.token}`,
          "User-Agent": "go-on-vscode/1.0",
          "Editor-Version": "vscode/1.90.0",
          "Editor-Plugin-Version": "copilot-chat/0.17.0",
          "Copilot-Integration-Id": "copilot-chat",
        },
      });

      if (response.status < 200 || response.status >= 300) {
        throw new Error(
          `Copilot model request failed (${response.status}): ${response.bodyText || "empty response"}`,
        );
      }

      const models = this._extractCopilotModelIds(response.body);
      if (models.length === 0) {
        throw new Error("Copilot model request returned no model identifiers.");
      }

      await this._storeCopilotModelCache(models);
      await this._updatePersistedCopilotState({
        lastError: "",
        lastStatus: `Fetched ${models.length} Copilot models from GitHub.`,
      });
      return {
        modelOptions: models,
        copilotAuth: await this._currentCopilotAuthState({
          isAuthorized: true,
          modelSource: "network",
          modelCount: models.length,
          statusMessage: `Fetched ${models.length} Copilot models from GitHub.`,
          lastError: "",
        }),
      };
    } catch (error: unknown) {
      const cached = this._loadCopilotModelCache();
      const message = errorMessage(error);
      await this._updatePersistedCopilotState({
        lastError: message,
        lastStatus: cached?.models?.length
          ? "Using cached Copilot models after refresh failure."
          : "Copilot model refresh failed.",
      });
      return {
        modelOptions: cached?.models || [],
        copilotAuth: await this._currentCopilotAuthState({
          isAuthorized: true,
          modelSource: cached?.models?.length ? "cache" : "config",
          modelCount: cached?.models?.length || 0,
          statusMessage: cached?.models?.length
            ? "Using cached Copilot models after refresh failure."
            : "Copilot model refresh failed.",
          lastError: message,
        }),
      };
    }
  }

  private async _authorizeCopilotWithGitHubSession(): Promise<void> {
    const session = await vscode.authentication.getSession(
      "github",
      ["read:user"],
      { createIfNone: true },
    );
    if (!session?.accessToken) {
      throw new Error("GitHub authentication did not return an access token.");
    }

    await this._exchangeCopilotToken(session.accessToken);
    await this._writeCopilotToken(session.accessToken);
    await this._updatePersistedCopilotState({
      authMode: "github-session",
      accountLabel: session.account.label,
      lastError: "",
      lastStatus: `Authenticated as ${session.account.label}.`,
    });
    await this._postCopilotAuthState({
      isAuthorized: true,
      authMode: "github-session",
      accountLabel: session.account.label,
      statusMessage: `Authenticated as ${session.account.label}.`,
      lastError: "",
    });
    await this._sendProviderModels("copilot");
  }

  private async _startCopilotDeviceAuthorization(
    oauthClientId: string,
  ): Promise<void> {
    // Fall back to GO_ON_COPILOT_CLIENT_ID environment variable if not provided
    const resolvedClientId =
      oauthClientId.trim() || process.env.GO_ON_COPILOT_CLIENT_ID || "";
    const clientId = resolvedClientId.trim();
    if (!clientId) {
      throw new Error(
        "GitHub OAuth client ID is required for device authorization. " +
          "Provide it via the settings panel or set the GO_ON_COPILOT_CLIENT_ID environment variable.",
      );
    }

    const response = await requestJson(GITHUB_DEVICE_CODE_URL, {
      method: "POST",
      headers: {
        "Content-Type": "application/x-www-form-urlencoded",
        "User-Agent": "go-on-vscode/1.0",
      },
      body: new URLSearchParams({
        client_id: clientId,
        scope: "read:user",
      }).toString(),
    });

    if (response.status < 200 || response.status >= 300) {
      throw new Error(
        `GitHub device code request failed (${response.status}): ${response.bodyText || "empty response"}`,
      );
    }

    const payload = response.body as DeviceCodeResponse;
    const deviceCode = String(payload.device_code || "").trim();
    const userCode = String(payload.user_code || "").trim();
    const verificationUri = String(payload.verification_uri || "").trim();
    const expiresIn = Number(payload.expires_in || 900);
    let intervalSeconds = Math.max(1, Number(payload.interval || 5));

    if (!deviceCode || !userCode || !verificationUri) {
      throw new Error("GitHub device authorization response is incomplete.");
    }

    const expiresAt = Date.now() + expiresIn * 1000;
    this._pendingCopilotDeviceAuth = {
      cancelRequested: false,
      userCode,
      verificationUri,
      expiresAt,
    };

    await this._updatePersistedCopilotState({
      oauthClientId: clientId,
      lastError: "",
      lastStatus: `Open ${verificationUri} and enter code ${userCode}.`,
    });
    await vscode.env.openExternal(vscode.Uri.parse(verificationUri));
    await this._postCopilotAuthState({
      authMode: "device-flow",
      oauthClientId: clientId,
      pending: true,
      userCode,
      verificationUri,
      expiresAt,
      statusMessage: `Open ${verificationUri} and enter code ${userCode}.`,
      lastError: "",
    });

    const poll = async (): Promise<void> => {
      while (
        this._pendingCopilotDeviceAuth &&
        !this._pendingCopilotDeviceAuth.cancelRequested
      ) {
        if (Date.now() >= expiresAt) {
          this._pendingCopilotDeviceAuth = undefined;
          await this._updatePersistedCopilotState({
            lastError: "Device authorization expired before completion.",
            lastStatus: "GitHub device authorization expired.",
          });
          await this._postCopilotAuthState({
            pending: false,
            authMode: "device-flow",
            oauthClientId: clientId,
            statusMessage: "GitHub device authorization expired.",
            lastError: "Device authorization expired before completion.",
          });
          return;
        }

        await new Promise((resolve) =>
          setTimeout(resolve, intervalSeconds * 1000),
        );
        if (
          !this._pendingCopilotDeviceAuth ||
          this._pendingCopilotDeviceAuth.cancelRequested
        ) {
          break;
        }

        const tokenResponse = await requestJson(GITHUB_ACCESS_TOKEN_URL, {
          method: "POST",
          headers: {
            "Content-Type": "application/x-www-form-urlencoded",
            "User-Agent": "go-on-vscode/1.0",
          },
          body: new URLSearchParams({
            client_id: clientId,
            device_code: deviceCode,
            grant_type: "urn:ietf:params:oauth:grant-type:device_code",
          }).toString(),
        });

        if (tokenResponse.status < 200 || tokenResponse.status >= 300) {
          throw new Error(
            `GitHub access token polling failed (${tokenResponse.status}): ${tokenResponse.bodyText || "empty response"}`,
          );
        }

        const tokenPayload = asRecord(tokenResponse.body);
        const accessToken =
          typeof tokenPayload.access_token === "string"
            ? tokenPayload.access_token.trim()
            : "";
        if (accessToken) {
          await this._exchangeCopilotToken(accessToken);
          await this._writeCopilotToken(accessToken);
          this._pendingCopilotDeviceAuth = undefined;
          await this._updatePersistedCopilotState({
            authMode: "device-flow",
            lastError: "",
            lastStatus: "GitHub device authorization completed.",
          });
          await this._postCopilotAuthState({
            isAuthorized: true,
            authMode: "device-flow",
            oauthClientId: clientId,
            pending: false,
            statusMessage: "GitHub device authorization completed.",
            lastError: "",
          });
          await this._sendProviderModels("copilot");
          return;
        }

        const pollError =
          typeof tokenPayload.error === "string" ? tokenPayload.error : "";
        if (pollError === "authorization_pending") {
          continue;
        }
        if (pollError === "slow_down") {
          intervalSeconds += 5;
          continue;
        }

        this._pendingCopilotDeviceAuth = undefined;
        const description =
          typeof tokenPayload.error_description === "string"
            ? tokenPayload.error_description
            : pollError || "unknown error";
        await this._updatePersistedCopilotState({
          lastError: description,
          lastStatus: "GitHub device authorization failed.",
        });
        await this._postCopilotAuthState({
          pending: false,
          authMode: "device-flow",
          oauthClientId: clientId,
          statusMessage: "GitHub device authorization failed.",
          lastError: description,
        });
        return;
      }
    };

    void poll().catch(async (error: unknown) => {
      this._pendingCopilotDeviceAuth = undefined;
      const message = errorMessage(error);
      await this._updatePersistedCopilotState({
        lastError: message,
        lastStatus: "GitHub device authorization failed.",
      });
      await this._postCopilotAuthState({
        pending: false,
        authMode: "device-flow",
        oauthClientId: clientId,
        statusMessage: "GitHub device authorization failed.",
        lastError: message,
      });
    });
  }

  private async _cancelCopilotDeviceAuthorization(): Promise<void> {
    if (this._pendingCopilotDeviceAuth) {
      this._pendingCopilotDeviceAuth.cancelRequested = true;
    }
    this._pendingCopilotDeviceAuth = undefined;
    await this._updatePersistedCopilotState({
      lastStatus: "Canceled pending GitHub device authorization.",
    });
    await this._postCopilotAuthState({
      pending: false,
      statusMessage: "Canceled pending GitHub device authorization.",
    });
  }

  private async _deleteCopilotAuthorization(): Promise<void> {
    try {
      await this._runSecretCommand("delete", COPILOT_SECRET_NAME);
    } catch (err) {
      log.warn("_deleteCopilotAuthorization failed:", err);
    }
    this.manager.setRuntimeEnvOverrides?.({
      [COPILOT_ENV_VAR]: "",
    });
    await this._updatePersistedCopilotState({
      authMode: "none",
      accountLabel: "",
      lastError: "",
      lastStatus: "Removed stored GitHub Copilot authorization.",
    });
    await this._postCopilotAuthState({
      isAuthorized: false,
      authMode: "none",
      accountLabel: "",
      pending: false,
      statusMessage: "Removed stored GitHub Copilot authorization.",
      lastError: "",
    });
    await this._sendProviderModels("copilot");
  }

  private async _loadProviderCatalog(): Promise<ProviderCatalogSpec[]> {
    const runtimeCatalog = await this._loadProviderCatalogFromRuntime();
    const configuredMap = await this._loadConfiguredAgentMap();
    const configuredCatalog = Array.from(configuredMap.entries()).map(
      ([name, snapshot]) => ({
        name,
        type: name,
        model: snapshot.model,
        api_key_env: snapshot.envVar || inferEnvVar(name),
      }),
    );

    const merged = dedupeCatalog([
      ...runtimeCatalog,
      ...configuredCatalog,
      ...BUILTIN_PROVIDER_CATALOG,
    ]);
    return merged;
  }

  private async _loadProviderCatalogFromRuntime(): Promise<
    ProviderCatalogSpec[]
  > {
    if (!this.manager.isRunning()) {
      return [];
    }

    try {
      const result = await this.manager.sendRequest("provider.catalog", {});
      const record = asRecord(result);
      // Backend returns { "ok": true, "catalog": [...], "total": N }
      const providers = Array.isArray(record.catalog) ? record.catalog : [];
      const parsed = providers
        .map((item) => asCatalogSpec(item))
        .filter((item): item is ProviderCatalogSpec => Boolean(item));
      if (parsed.length > 0) {
        return parsed;
      }
    } catch (err) {
      log.warn("provider.catalog failed:", err);
    }

    try {
      const result = await this.manager.sendRequest("models/list", {});
      const payload = asRecord(result);
      const groups = Array.isArray(payload.models) ? payload.models : [];
      const byProvider = new Map<string, string[]>();
      for (const group of groups) {
        const item = asRecord(group);
        const name = String(item.provider || item.agent || "").trim();
        const modelId = this._modelIdFromRuntime(
          item.id || item.model_id || item.name,
        );
        if (!name) {
          continue;
        }
        if (!byProvider.has(name)) {
          byProvider.set(name, []);
        }
        if (modelId && !byProvider.get(name)?.includes(modelId)) {
          byProvider.get(name)?.push(modelId);
        }
      }
      const parsed = Array.from(byProvider.entries()).map(
        ([name, models]) =>
          ({
            name,
            type: name,
            model: models[0],
            api_key_env: inferEnvVar(name),
          }) as ProviderCatalogSpec,
      );
      return parsed;
    } catch (err) {
      log.warn("models/list failed:", err);
      return [];
    }
  }

  private async _loadConfiguredAgentMap(): Promise<
    Map<string, ProviderConfigSnapshot>
  > {
    try {
      const configPath = this._resolveConfigPath();
      const content = await fs.readFile(configPath, "utf8");
      return parseConfiguredAgents(content);
    } catch (err) {
      log.warn("_loadConfiguredAgentMap failed:", err);
      return new Map();
    }
  }

  private _modelIdFromRuntime(value: unknown): string | undefined {
    if (typeof value === "string" && value.trim()) {
      return value.trim();
    }
    if (typeof value !== "object" || value === null) {
      return undefined;
    }
    const record = value as Record<string, unknown>;
    const candidates = [
      record.id,
      record.model_id,
      record.modelId,
      record.name,
    ];
    for (const candidate of candidates) {
      if (typeof candidate === "string" && candidate.trim()) {
        return candidate.trim();
      }
    }
    return undefined;
  }

  private async _resolveProviderModels(
    providerName: string,
    spec?: ProviderCatalogEntry,
  ): Promise<ProviderModelResolution> {
    const modelSet = new Set<string>();
    modelSet.add("auto");
    if (spec?.defaultModel) {
      modelSet.add(spec.defaultModel);
    }

    let copilotAuth: CopilotAuthState | undefined;

    if (providerName === "copilot") {
      const copilotModels = await this._resolveCopilotModels();
      copilotAuth = copilotModels.copilotAuth;
      for (const model of copilotModels.modelOptions) {
        modelSet.add(model);
      }
    }

    if (this.manager.isRunning()) {
      try {
        const response = await this.manager.sendRequest(
          "provider.list_models",
          {
            provider: providerName,
          },
        );
        const payload = asRecord(response);
        const ids = Array.isArray(payload.model_ids) ? payload.model_ids : [];
        for (const item of ids) {
          const modelId = this._modelIdFromRuntime(item);
          if (modelId) {
            modelSet.add(modelId);
          }
        }

        const runtimeModels = Array.isArray(payload.models)
          ? payload.models
          : [];
        for (const runtimeModel of runtimeModels) {
          const modelId = this._modelIdFromRuntime(runtimeModel);
          if (modelId) {
            modelSet.add(modelId);
          }
        }

        const defaultModel = this._modelIdFromRuntime(payload.default_model);
        if (defaultModel) {
          modelSet.add(defaultModel);
        }
      } catch (err) {
        log.warn("_resolveProviderModels provider.models failed:", err);
        try {
          const response = await this.manager.sendRequest("models/list", {});
          const payload = asRecord(response);
          const groups = Array.isArray(payload.models) ? payload.models : [];
          for (const group of groups) {
            const record = asRecord(group);
            const groupProvider = String(
              record.provider || record.agent || "",
            ).trim();
            if (groupProvider !== providerName) {
              continue;
            }
            const modelId = this._modelIdFromRuntime(
              record.id || record.model_id || record.name,
            );
            if (modelId) {
              modelSet.add(modelId);
            }
          }
        } catch (err2) {
          log.warn("_resolveProviderModels models/list failed:", err2);
        }
      }
    }

    return {
      modelOptions: Array.from(modelSet.values()),
      copilotAuth,
    };
  }

  private async _buildProviderSettingsPayload() {
    const catalog = await this._loadProviderCatalog();
    const configured = await this._loadConfiguredAgentMap();
    const secretTargets = collectProviderSecretTargets(catalog);
    const providers: ProviderCatalogEntry[] = catalog
      .map((spec) => {
        const configuredValue = configured.get(spec.name);
        return {
          name: spec.name,
          agentType: spec.type,
          group: spec.group,
          defaultModel: spec.model,
          apiKeyEnv: spec.api_key_env,
          secretKeyEnv: spec.secret_key_env,
          url: spec.url,
          chatPath: spec.chat_path,
          supportsSystem: spec.supports_system,
          configuredModel: configuredValue?.model,
          configuredEnvVar: configuredValue?.envVar,
        };
      })
      .sort((a, b) => a.name.localeCompare(b.name));

    const selectedProvider =
      providers.find((item) => item.configuredModel || item.configuredEnvVar)
        ?.name ||
      providers[0]?.name ||
      "copilot";
    const selectedSpec = providers.find(
      (item) => item.name === selectedProvider,
    );
    const selectedModel =
      selectedSpec?.configuredModel || selectedSpec?.defaultModel || "auto";
    const selectedEnvVar =
      selectedSpec?.configuredEnvVar ||
      selectedSpec?.apiKeyEnv ||
      inferEnvVar(selectedProvider);
    const modelResolution = await this._resolveProviderModels(
      selectedProvider,
      selectedSpec,
    );

    return {
      providers,
      selectedProvider,
      selectedModel,
      selectedEnvVar,
      modelOptions: modelResolution.modelOptions,
      secretTargets,
      copilotAuth:
        modelResolution.copilotAuth || (await this._currentCopilotAuthState()),
    };
  }

  private async _sendProviderModels(providerName: string) {
    if (!providerName.trim()) {
      return;
    }

    const payload = await this._buildProviderSettingsPayload();
    const selectedSpec = payload.providers.find(
      (item) => item.name === providerName,
    );
    const modelResolution = await this._resolveProviderModels(
      providerName,
      selectedSpec,
    );

    this._postMessage({
      type: "providerModelsData",
      provider: providerName,
      modelOptions: modelResolution.modelOptions,
      selectedModel:
        selectedSpec?.configuredModel || selectedSpec?.defaultModel || "auto",
      selectedEnvVar:
        selectedSpec?.configuredEnvVar ||
        selectedSpec?.apiKeyEnv ||
        inferEnvVar(providerName),
      copilotAuth: modelResolution.copilotAuth,
    });
  }

  private async _saveProviderSelection(
    providerName: string,
    modelName: string,
    envVar?: string,
  ) {
    const provider = providerName.trim();
    if (!provider) {
      throw new Error("Provider cannot be empty.");
    }

    const catalog = await this._loadProviderCatalog();
    const matched = catalog.find((item) => item.name === provider);
    if (!matched) {
      throw new Error(`Unknown provider: ${provider}`);
    }

    const normalizedModel = modelName.trim() || "auto";
    const normalizedEnvVar =
      (envVar || "").trim() || matched.api_key_env || inferEnvVar(provider);
    const copilotConfigEnvVar =
      provider === "copilot" && (await this._readCopilotToken())
        ? `keyring://go-on/${COPILOT_SECRET_NAME}`
        : `keyring://go-on/${secretNameForEnvVar(normalizedEnvVar)}`;

    const configPath = this._resolveConfigPath();
    let content = "";
    try {
      content = await fs.readFile(configPath, "utf8");
    } catch (err) {
      log.warn("_saveProviderSelection read failed:", err);
      content = "";
    }

    content = upsertAgentSection(content, provider, {
      type: matched.type,
      url: matched.url,
      chat_path: matched.chat_path,
      api_key_env: copilotConfigEnvVar,
      secret_key_env: matched.secret_key_env
        ? `keyring://go-on/${secretNameForEnvVar(matched.secret_key_env)}`
        : undefined,
      anthropic_version: matched.anthropic_version,
      model: normalizedModel,
      max_tokens: matched.max_tokens,
      supports_system: matched.supports_system,
    });

    const defaultPhase = content.match(
      /^default_phase\s*=\s*"([^"]+)"\s*$/m,
    )?.[1];
    if (defaultPhase) {
      content = upsertPhaseAgents(content, defaultPhase, [provider]);
    }

    await fs.writeFile(configPath, `${content.trimEnd()}\n`, "utf8");
    this._postMessage({
      type: "settingsActionResult",
      message: `Saved provider=${provider}, model=${normalizedModel} to ${configPath}`,
    });
    await this._sendCurrentSettings();
  }

  // Settings update methods
  private async _updateRuntimeSetting(key: string, value: unknown) {
    try {
      configManager.setConfigValue(`runtime.${key}`, value);
      await configManager.saveToFile();
      vscode.window.showInformationMessage(
        i18n.getMessage(MessageKeys.successfullySaved),
      );
      this._sendCurrentSettings();
    } catch (error: unknown) {
      vscode.window.showErrorMessage(
        `${i18n.getMessage(MessageKeys.errorSaving)}: ${getErrorMessage(error)}`,
      );
    }
  }

  private async _updateCacheSetting(key: string, value: unknown) {
    try {
      configManager.setConfigValue(`cache.${key}`, value);
      await configManager.saveToFile();
      vscode.window.showInformationMessage(
        i18n.getMessage(MessageKeys.successfullySaved),
      );
      this._sendCurrentSettings();
    } catch (error: unknown) {
      vscode.window.showErrorMessage(
        `${i18n.getMessage(MessageKeys.errorSaving)}: ${getErrorMessage(error)}`,
      );
    }
  }

  private async _updateVectorSetting(key: string, value: unknown) {
    try {
      configManager.setConfigValue(`vector.${key}`, value);
      await configManager.saveToFile();
      vscode.window.showInformationMessage(
        i18n.getMessage(MessageKeys.successfullySaved),
      );
      this._sendCurrentSettings();
    } catch (error: unknown) {
      vscode.window.showErrorMessage(
        `${i18n.getMessage(MessageKeys.errorSaving)}: ${getErrorMessage(error)}`,
      );
    }
  }

  private async _updateAutotuneSetting(key: string, value: unknown) {
    try {
      configManager.setConfigValue(`autotune.${key}`, value);
      await configManager.saveToFile();
      vscode.window.showInformationMessage(
        i18n.getMessage(MessageKeys.successfullySaved),
      );
      this._sendCurrentSettings();
    } catch (error: unknown) {
      vscode.window.showErrorMessage(
        `${i18n.getMessage(MessageKeys.errorSaving)}: ${getErrorMessage(error)}`,
      );
    }
  }

  private async _addAgent(name: string, config: unknown) {
    try {
      configManager.setConfigValue(`agents.${name}`, config);
      await configManager.saveToFile();
      vscode.window.showInformationMessage(
        i18n.getMessage(MessageKeys.successfullySaved),
      );
      this._sendCurrentSettings();
    } catch (error: unknown) {
      vscode.window.showErrorMessage(
        `${i18n.getMessage(MessageKeys.errorSaving)}: ${getErrorMessage(error)}`,
      );
    }
  }

  private async _deleteAgent(name: string) {
    try {
      const config = configManager.getConfig();
      delete config.agents[name];
      await configManager.saveToFile();
      vscode.window.showInformationMessage(
        i18n.getMessage(MessageKeys.successfullySaved),
      );
      this._sendCurrentSettings();
    } catch (error: unknown) {
      vscode.window.showErrorMessage(
        `${i18n.getMessage(MessageKeys.errorSaving)}: ${getErrorMessage(error)}`,
      );
    }
  }

  private async _updatePhase(name: string, config: unknown) {
    try {
      configManager.setConfigValue(`phases.${name}`, config);
      await configManager.saveToFile();
      vscode.window.showInformationMessage(
        i18n.getMessage(MessageKeys.successfullySaved),
      );
      this._sendCurrentSettings();
    } catch (error: unknown) {
      vscode.window.showErrorMessage(
        `${i18n.getMessage(MessageKeys.errorSaving)}: ${getErrorMessage(error)}`,
      );
    }
  }

  private async _setLanguage(language: string) {
    try {
      const config = vscode.workspace.getConfiguration("go-on");
      await config.update(
        "language",
        language,
        vscode.ConfigurationTarget.Global,
      );
      configManager.setConfigValue("language", language);
      await configManager.saveToFile();
      vscode.window.showInformationMessage(
        i18n.getMessage(MessageKeys.successfullySaved),
      );
      this._sendCurrentSettings();
    } catch (error: unknown) {
      vscode.window.showErrorMessage(
        `${i18n.getMessage(MessageKeys.errorSaving)}: ${getErrorMessage(error)}`,
      );
    }
  }

  private async _sendCurrentSettings() {
    if (!this._view) return;

    const config = configManager.getConfig();
    const vsCodeConfig = vscode.workspace.getConfiguration("go-on");
    let providerSettings: {
      providers: ProviderCatalogEntry[];
      selectedProvider: string;
      selectedModel: string;
      selectedEnvVar: string;
      modelOptions: string[];
      secretTargets: ProviderSecretTarget[];
      copilotAuth: CopilotAuthState;
    } = {
      providers: [],
      selectedProvider: "copilot",
      selectedModel: "auto",
      selectedEnvVar: inferEnvVar("copilot"),
      modelOptions: ["auto"],
      secretTargets: [],
      copilotAuth: await this._currentCopilotAuthState(),
    };

    try {
      providerSettings = await this._buildProviderSettingsPayload();
    } catch (err) {
      log.warn("_buildProviderSettingsPayload failed:", err);
    }

    const settings = {
      language: i18n.getCurrentLanguage(),
      runtime: config.runtime,
      cache: config.cache,
      vector: config.vector,
      autotune: config.autotune,
      agents: config.agents,
      phases: config.phases,
      flow: config.flow,
      executablePath: vsCodeConfig.get("executablePath"),
      autoStart: vsCodeConfig.get("autoStart"),
      isRunning: this.manager.isRunning?.() || false,
      providerSettings,
    };

    this._view.webview.postMessage({
      type: "settingsData",
      data: settings,
      translations: this._getTranslations(),
      language: i18n.getCurrentLanguage(),
    });
  }

  private _getTranslations() {
    return {
      general: {
        goOn: i18n.getMessage(MessageKeys.goOn),
        settings: i18n.getMessage(MessageKeys.settings),
        start: i18n.getMessage(MessageKeys.start),
        stop: i18n.getMessage(MessageKeys.stop),
        status: i18n.getMessage(MessageKeys.status),
        running: i18n.getMessage(MessageKeys.running),
        stopped: i18n.getMessage(MessageKeys.stopped),
      },
      runtime: {
        runtime: i18n.getMessage(MessageKeys.runtime),
        runtimeSettings: i18n.getMessage(MessageKeys.runtimeSettings),
        maintenanceInterval: i18n.getMessage(MessageKeys.maintenanceInterval),
        healthInterval: i18n.getMessage(MessageKeys.healthInterval),
        shutdownDrain: i18n.getMessage(MessageKeys.shutdownDrain),
      },
      execution: {
        executionSettings: i18n.getMessage(MessageKeys.executionSettings),
        startGoOn: i18n.getMessage(MessageKeys.startGoOn),
        stopGoOn: i18n.getMessage(MessageKeys.stopGoOn),
        healthCheck: i18n.getMessage(MessageKeys.healthCheck),
        clearCache: i18n.getMessage(MessageKeys.clearCache),
      },
      workflow: {
        workflow: i18n.getMessage(MessageKeys.workflow),
        phases: i18n.getMessage(MessageKeys.phases),
        agents: i18n.getMessage(MessageKeys.agents),
        addPhase: i18n.getMessage(MessageKeys.addPhase),
        editPhase: i18n.getMessage(MessageKeys.editPhase),
        deletePhase: i18n.getMessage(MessageKeys.deletePhase),
      },
      buttons: {
        save: i18n.getMessage(MessageKeys.save),
        cancel: i18n.getMessage(MessageKeys.cancel),
        reset: i18n.getMessage(MessageKeys.reset),
        apply: i18n.getMessage(MessageKeys.apply),
        delete: i18n.getMessage(MessageKeys.delete),
        edit: i18n.getMessage(MessageKeys.edit),
        add: i18n.getMessage(MessageKeys.add),
      },
      messages: {
        successfullySaved: i18n.getMessage(MessageKeys.successfullySaved),
        errorSaving: i18n.getMessage(MessageKeys.errorSaving),
      },
      language: {
        language: i18n.getMessage(MessageKeys.language),
        simplifiedChinese: i18n.getMessage(MessageKeys.simplifiedChinese),
        traditionalChinese: i18n.getMessage(MessageKeys.traditionalChinese),
        english: i18n.getMessage(MessageKeys.english),
      },
      credentials: {
        credentials: i18n.getMessage(MessageKeys.credentials),
        apiKey: i18n.getMessage(MessageKeys.apiKey),
        secretKey: i18n.getMessage(MessageKeys.secretKey),
      },
    };
  }

  private async _handleKeyringSet(name: string, value: string) {
    try {
      await vscode.commands.executeCommand("go-on.keyringSet", { name, value });
      this._postMessage({
        type: "keyringResult",
        message: `Saved secret '${name}' to system keyring.`,
      });
    } catch (error: unknown) {
      this._postMessage({
        type: "keyringError",
        message: getErrorMessage(error),
      });
    }
  }

  private async _handleKeyringGet(name: string) {
    try {
      const value = await vscode.commands.executeCommand<string>(
        "go-on.keyringGet",
        { name },
      );
      this._postMessage({
        type: "keyringResult",
        message: `Fetched secret '${name}' from system keyring.`,
        value: value ?? "",
      });
    } catch (error: unknown) {
      this._postMessage({
        type: "keyringError",
        message: getErrorMessage(error),
      });
    }
  }

  private async _handleKeyringDelete(name: string) {
    try {
      await vscode.commands.executeCommand("go-on.keyringDelete", { name });
      this._postMessage({
        type: "keyringResult",
        message: `Deleted secret '${name}' from system keyring.`,
      });
    } catch (error: unknown) {
      this._postMessage({
        type: "keyringError",
        message: getErrorMessage(error),
      });
    }
  }

  private async _handleKeyringList() {
    try {
      const output =
        await vscode.commands.executeCommand<string>("go-on.keyringList");
      this._postMessage({
        type: "keyringResult",
        message: "Listed keyring secret status.",
        value: output ?? "",
      });
    } catch (error: unknown) {
      this._postMessage({
        type: "keyringError",
        message: getErrorMessage(error),
      });
    }
  }

  private async _handleQuickSetupProvider(
    provider: string,
    apiKey: string,
  ): Promise<void> {
    try {
      // 1. Derive keyring account name from provider name
      const accountName = secretNameForEnvVar(inferEnvVar(provider));

      // 2. Store API key in keyring (primary secure storage)
      //    The generated config.toml uses `keyring://go-on/{name}_api_key` URIs
      //    and the backend resolves these via system keyring. No env overrides
      //    are set — secrets must NOT leak to /proc/PID/environ.
      await vscode.commands.executeCommand("go-on.keyringSet", {
        name: accountName,
        value: apiKey,
      });

      // 3. Handle secret key for dual-auth providers (e.g. wenxin, qianfan)
      const catalog = await this._loadProviderCatalog();
      const matched = catalog.find((item) => item.name === provider);
      if (matched?.secret_key_env) {
        const secretKey = await vscode.window.showInputBox({
          prompt: `Enter the secret key for ${provider} (${matched.secret_key_env})`,
          password: true,
          ignoreFocusOut: true,
          placeHolder: matched.secret_key_env,
        });
        if (secretKey !== undefined && secretKey !== "") {
          await vscode.commands.executeCommand("go-on.keyringSet", {
            name: secretNameForEnvVar(matched.secret_key_env),
            value: secretKey,
          });
        }
      }

      // 4. Save provider selection to config.toml (uses keyring:// URI)
      await this._saveProviderSelection(
        provider,
        "auto",
        inferEnvVar(provider),
      );

      this._postMessage({
        type: "quickSetupResult",
        message: `✅ ${provider} configured successfully. API key saved to keyring.`,
      });
    } catch (error: unknown) {
      this._postMessage({
        type: "quickSetupError",
        message: `Setup failed: ${getErrorMessage(error)}`,
      });
    }
  }

  private async _handleApplyDefaultConfigTemplate(template: string) {
    try {
      const configPath = await vscode.commands.executeCommand<string>(
        "go-on.applyDefaultConfigTemplate",
        { template },
      );
      this._postMessage({
        type: "settingsActionResult",
        message: `Applied template '${template}' to ${configPath}.`,
      });
    } catch (error: unknown) {
      this._postMessage({
        type: "settingsActionError",
        message: getErrorMessage(error),
      });
    }
  }

  private async _handleApplyRulesSettings(payload: {
    globalRules?: string[];
    commonRules?: string[];
    phaseRules?: Record<string, string[]>;
  }) {
    try {
      const rulesDir = await vscode.commands.executeCommand<string>(
        "go-on.updateRules",
        payload,
      );
      this._postMessage({
        type: "settingsActionResult",
        message: `Rules updated in ${rulesDir}.`,
      });
    } catch (error: unknown) {
      this._postMessage({
        type: "settingsActionError",
        message: getErrorMessage(error),
      });
    }
  }

  private async _handleApplyWorkflowMapping(payload: {
    defaultPhase?: string;
    phases?: Record<
      string,
      {
        agents: string[];
        fallback?: boolean;
        principles?: string[];
        switchRules?: {
          circuitBreakerFailures?: number;
          circuitBreakerOpenSeconds?: number;
        };
      }
    >;
  }) {
    try {
      const configPath = await vscode.commands.executeCommand<string>(
        "go-on.updateWorkflowMapping",
        payload,
      );
      this._postMessage({
        type: "settingsActionResult",
        message: `Workflow mapping saved to ${configPath}.`,
      });
    } catch (error: unknown) {
      this._postMessage({
        type: "settingsActionError",
        message: getErrorMessage(error),
      });
    }
  }

  private _postMessage(message: unknown) {
    try {
      this._view?.webview.postMessage(message);
    } catch (err) {
      log.warn("_postMessage failed:", err);
    }
  }

  public async showConfigWizard() {
    const panel = vscode.window.createWebviewPanel(
      "goOnConfigWizard",
      i18n.getMessage(MessageKeys.configWizardTitle),
      vscode.ViewColumn.One,
      { enableScripts: true },
    );

    panel.webview.html = this._getConfigWizardHtml(panel.webview);

    // Store the message listener disposable so it can be cleaned up when the panel closes
    const messageSubscription = panel.webview.onDidReceiveMessage(
      async (message: Record<string, unknown>) => {
        const command = String(message.command ?? "");
        if (command === "cancel") {
          panel.dispose();
          return;
        }
        if (command !== "saveConfig") {
          return;
        }

        const payload = (message.config ?? {}) as Record<string, unknown>;
        const goOnConfig = vscode.workspace.getConfiguration("go-on");
        const rawProtocolMode = String(payload.protocolMode ?? "from_config");
        const protocolMode =
          rawProtocolMode === "from_config"
            ? "from_config"
            : normalizeProtocolMode(rawProtocolMode);

        await Promise.all([
          goOnConfig.update(
            "configPath",
            String(payload.configPath ?? "./config.toml"),
            vscode.ConfigurationTarget.Workspace,
          ),
          goOnConfig.update(
            "executablePath",
            String(payload.executablePath ?? ""),
            vscode.ConfigurationTarget.Workspace,
          ),
          goOnConfig.update(
            "autoStart",
            Boolean(payload.autoStart),
            vscode.ConfigurationTarget.Workspace,
          ),
          goOnConfig.update(
            "runtime.protocolMode",
            protocolMode,
            vscode.ConfigurationTarget.Workspace,
          ),
        ]);

        configManager.setConfigValue("runtime.protocolMode", protocolMode);
        await configManager.saveToFile();
        await this._sendCurrentSettings();

        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.successfullySaved),
        );
        panel.dispose();
      },
    );

    // Clean up the message subscription when the panel is disposed
    panel.onDidDispose(() => {
      messageSubscription.dispose();
    });
  }

  private _getConfigWizardHtml(webview: vscode.Webview) {
    const config = vscode.workspace.getConfiguration("go-on");
    const configPath = String(config.get("configPath", "./config.toml"));
    const executablePath = String(config.get("executablePath", ""));
    const autoStart = Boolean(config.get("autoStart", false));
    const protocolMode = String(
      config.get("runtime.protocolMode", "from_config"),
    );

    return getConfigWizardHtml(webview, {
      configPath,
      executablePath,
      autoStart,
      protocolMode,
    });
  }

}
