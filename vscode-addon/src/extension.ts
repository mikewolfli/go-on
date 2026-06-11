import * as vscode from "vscode";
import { spawn } from "child_process";
import * as path from "path";
import * as fsPromises from "fs/promises";
import { GoOnChatViewProvider } from "./chatView";
import { GoOnSettingsViewProvider } from "./settingsView";
import { StatusMonitor } from "./statusMonitor";
import { GoOnWorkflowViewProvider } from "./workflowView";
import { GoOnProcessFlowViewProvider } from "./processFlowView";
import { ApprovalPanelProvider } from "./approvalPanel";
import { GoOnAdvancedEditProvider } from "./advancedEdit";
import { i18n, MessageKeys } from "./i18n";
import { configManager } from "./configManager";
import { revealGoOnView } from "./viewRouter";
import { registerViewCommands } from "./commandRegistry";
import { registerRpcCommands } from "./rpcCommandRegistry";
import { registerCoreCommands } from "./coreCommandRegistry";
import { GoOnManager, GoOnStatusProvider } from "./runtimeManager";
import { disposeLogger } from "./logger";
import { startStateSyncListener } from "./stateSync";
import { protocolContract } from "./protocolContract";
import {
  ensureGoOnBinary,
  pathExists,
  resolveConfigPath,
} from "./runtimeBinaryService";
import { parse as parseToml, stringify as stringifyToml } from "smol-toml";
import {
  buildPlaceholderEnvValues,
  parseMissingEnvVariableNames,
  prepareRuntimeAndStartFromChat,
  RuntimeBootstrapDeps,
} from "./runtimeBootstrap";

/**
 * DUPLICATE: This function is duplicated (with variations) as
 * `_runSecretCommand` in settingsView.ts (line ~1001).
 * The extension.ts version pipes secrets via stdin (secure) and has
 * timeout handling; the settingsView.ts version passes secrets as CLI args
 * without timeout. Consider consolidating into a shared utility.
 */
async function runGoOnSecretCommand(
  context: vscode.ExtensionContext,
  action: "set" | "get" | "delete" | "list",
  secretName?: string,
  secretValue?: string,
): Promise<string> {
  const config = vscode.workspace.getConfiguration("go-on");
  const workspaceRoot = getPrimaryWorkspaceRoot()?.fsPath;
  const runtime = await ensureGoOnBinary(workspaceRoot, config, context);

  const args: string[] = ["--secret", action];
  if (secretName) {
    args.push("--secret-name", secretName);
  }
  // Secret value is written to stdin instead of passing as --secret-value CLI
  // argument, so it's not visible in /proc/PID/cmdline or ps aux.
  const hasSecretValue = secretValue !== undefined;

  return new Promise<string>((resolve, reject) => {
    const proc = spawn(runtime.executablePath, args, {
      cwd: workspaceRoot || runtime.runtimeDir,
      stdio: [hasSecretValue ? "pipe" : "ignore", "pipe", "pipe"],
    });

    // Pipe secret through stdin so it doesn't leak in process listings
    if (hasSecretValue && proc.stdin) {
      proc.stdin.write(secretValue!);
      proc.stdin.end();
    } else if (proc.stdin) {
      proc.stdin.end();
    }

    let stdout = "";
    let stderr = "";
    let timedOut = false;

    const timeoutHandle = setTimeout(() => {
      timedOut = true;
      proc.kill("SIGTERM");
      // Give it a moment to terminate, then force kill
      setTimeout(() => {
        try {
          if (!proc.killed) {
            proc.kill("SIGKILL");
          }
        } catch (err) {
          // eslint-disable-next-line no-console
          console.warn("[extension] forceKill failed:", err);
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

/**
 * Maximum TOML file size to process (1MB).
 * Larger files are rejected to prevent OOM on malformed responses.
 */
const MAX_TOML_SIZE = 1024 * 1024;

/**
 * Returns the primary (first) workspace root URI, or undefined if no
 * workspace is open. Use this when a single workspace root is needed
 * for operations like starting the runtime or resolving config paths.
 */
function getPrimaryWorkspaceRoot(): vscode.Uri | undefined {
  return vscode.workspace.workspaceFolders?.[0]?.uri;
}

/**
 * Guard and parse TOML content. Throws if content exceeds MAX_TOML_SIZE.
 */
function guardedParse(content: string): Record<string, unknown> {
  if (content.length > MAX_TOML_SIZE) {
    throw new Error(
      `TOML content exceeds maximum size of ${MAX_TOML_SIZE} bytes`,
    );
  }
  return parseToml(content) as Record<string, unknown>;
}

/**
 * Upserts a top-level string key-value pair in a TOML config.
 * Uses the proper smol-toml parser/stringifier instead of regex.
 */
function upsertTopLevelString(
  content: string,
  key: string,
  value: string,
): string {
  const doc = guardedParse(content);
  doc[key] = value;
  return stringifyToml(doc);
}

/**
 * Upserts the `phases` list in the `[flow]` section of a TOML config.
 * Uses the proper smol-toml parser/stringifier.
 */
function upsertFlowPhases(content: string, phases: string[]): string {
  const doc = guardedParse(content);
  if (!doc.flow || typeof doc.flow !== "object") {
    doc.flow = { name: "Configured Flow" };
  }
  (doc.flow as Record<string, unknown>).phases = phases;
  return stringifyToml(doc);
}

/**
 * Upserts the `agents` list in a `[phases.{phase}]` section.
 * Uses the proper smol-toml parser/stringifier.
 */
function upsertPhaseAgents(
  content: string,
  phase: string,
  agents: string[],
): string {
  const doc = guardedParse(content);
  if (!doc.phases || typeof doc.phases !== "object") {
    doc.phases = {};
  }
  const phases = doc.phases as Record<string, unknown>;
  if (!phases[phase] || typeof phases[phase] !== "object") {
    phases[phase] = {
      description: `${phase} phase`,
      fallback: true,
      agents,
    };
  } else {
    (phases[phase] as Record<string, unknown>).agents = agents;
  }
  return stringifyToml(doc);
}

/**
 * Upserts the `fallback` boolean in a `[phases.{phase}]` section.
 * Uses the proper smol-toml parser/stringifier.
 */
function upsertPhaseFallback(
  content: string,
  phase: string,
  fallback: boolean,
): string {
  const doc = guardedParse(content);
  if (!doc.phases || typeof doc.phases !== "object") {
    doc.phases = {};
  }
  const phases = doc.phases as Record<string, unknown>;
  if (!phases[phase] || typeof phases[phase] !== "object") {
    phases[phase] = {
      description: `${phase} phase`,
      agents: ["copilot"],
      fallback,
    };
  } else {
    (phases[phase] as Record<string, unknown>).fallback = fallback;
  }
  return stringifyToml(doc);
}

/**
 * Upserts the `principles` list in a `[phases.{phase}]` section.
 * Uses the proper smol-toml parser/stringifier.
 */
function upsertPhasePrinciples(
  content: string,
  phase: string,
  principles: string[],
): string {
  const doc = guardedParse(content);
  if (!doc.phases || typeof doc.phases !== "object") {
    doc.phases = {};
  }
  const phases = doc.phases as Record<string, unknown>;
  if (!phases[phase] || typeof phases[phase] !== "object") {
    phases[phase] = {
      description: `${phase} phase`,
      agents: ["copilot"],
      fallback: true,
      principles,
    };
  } else {
    (phases[phase] as Record<string, unknown>).principles = principles;
  }
  return stringifyToml(doc);
}

/**
 * Upserts a numeric option key in a `[phases.{phase}.options]` section.
 * Uses the proper smol-toml parser/stringifier.
 */
function upsertPhaseOptionNumber(
  content: string,
  phase: string,
  optionKey: string,
  value: number,
): string {
  const doc = guardedParse(content);
  if (!doc.phases || typeof doc.phases !== "object") {
    doc.phases = {};
  }
  const phases = doc.phases as Record<string, unknown>;
  if (!phases[phase] || typeof phases[phase] !== "object") {
    phases[phase] = {};
  }
  const phaseObj = phases[phase] as Record<string, unknown>;
  if (!phaseObj.options || typeof phaseObj.options !== "object") {
    phaseObj.options = {};
  }
  (phaseObj.options as Record<string, unknown>)[optionKey] = value;
  return stringifyToml(doc);
}

async function resolveConfigFilePath(
  context: vscode.ExtensionContext,
  configuredConfigPath?: string,
): Promise<{ workspaceRoot: string; configPath: string; runtimeDir: string }> {
  const workspaceRoot = getPrimaryWorkspaceRoot()?.fsPath;
  if (!workspaceRoot) {
    throw new Error("No workspace folder open.");
  }

  const config = vscode.workspace.getConfiguration("go-on");
  const runtime = await ensureGoOnBinary(workspaceRoot, config, context);
  const settingPath =
    configuredConfigPath || config.get<string>("configPath", "./config.toml");
  const configPath = await resolveConfigPath(
    workspaceRoot,
    settingPath,
    runtime.runtimeDir,
  );
  return { workspaceRoot, configPath, runtimeDir: runtime.runtimeDir };
}

async function applyDefaultConfigTemplate(
  context: vscode.ExtensionContext,
  templateFile: string,
): Promise<string> {
  const { workspaceRoot, configPath, runtimeDir } =
    await resolveConfigFilePath(context);
  const candidates = [
    path.resolve(workspaceRoot, templateFile),
    path.join(runtimeDir, templateFile),
  ];

  let sourcePath: string | undefined;
  for (const candidate of candidates) {
    if (await pathExists(candidate)) {
      sourcePath = candidate;
      break;
    }
  }

  if (!sourcePath) {
    throw new Error(`Template not found: ${templateFile}`);
  }

  await fsPromises.copyFile(sourcePath, configPath);
  return configPath;
}

async function updateWorkflowMappingConfig(
  context: vscode.ExtensionContext,
  mapping: {
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
  },
): Promise<string> {
  const { configPath } = await resolveConfigFilePath(context);
  let content = await fsPromises.readFile(configPath, "utf8");

  const phaseEntries = Object.entries(mapping.phases || {})
    .map(([phase, config]) => {
      const phaseName = phase.trim();
      const agents = (config?.agents || [])
        .map((a) => a.trim())
        .filter(Boolean);
      const principles = (config?.principles || [])
        .map((p) => p.trim())
        .filter(Boolean);
      return [phaseName, { ...config, agents, principles }] as const;
    })
    .filter(([phase, config]) => phase.length > 0 && config.agents.length > 0);

  if (mapping.defaultPhase && mapping.defaultPhase.trim().length > 0) {
    content = upsertTopLevelString(
      content,
      "default_phase",
      mapping.defaultPhase.trim(),
    );
  }

  if (phaseEntries.length > 0) {
    const phaseNames = phaseEntries.map(([phase]) => phase);
    content = upsertFlowPhases(content, phaseNames);
    for (const [phase, phaseConfig] of phaseEntries) {
      content = upsertPhaseAgents(content, phase, phaseConfig.agents);
      if (typeof phaseConfig.fallback === "boolean") {
        content = upsertPhaseFallback(content, phase, phaseConfig.fallback);
      }
      if (phaseConfig.principles && phaseConfig.principles.length > 0) {
        content = upsertPhasePrinciples(content, phase, phaseConfig.principles);
      }

      const switchRules = phaseConfig.switchRules;
      if (switchRules) {
        if (
          typeof switchRules.circuitBreakerFailures === "number" &&
          switchRules.circuitBreakerFailures > 0
        ) {
          content = upsertPhaseOptionNumber(
            content,
            phase,
            "circuit_breaker_failures",
            Math.floor(switchRules.circuitBreakerFailures),
          );
        }
        if (
          typeof switchRules.circuitBreakerOpenSeconds === "number" &&
          switchRules.circuitBreakerOpenSeconds > 0
        ) {
          content = upsertPhaseOptionNumber(
            content,
            phase,
            "circuit_breaker_open_seconds",
            Math.floor(switchRules.circuitBreakerOpenSeconds),
          );
        }
      }
    }
  }

  await fsPromises.writeFile(configPath, content, "utf8");
  return configPath;
}

async function updateRulesMarkdownFiles(
  context: vscode.ExtensionContext,
  payload: {
    globalRules?: string[];
    commonRules?: string[];
    phaseRules?: Record<string, string[]>;
  },
): Promise<string> {
  const { configPath } = await resolveConfigFilePath(context);
  const configDir = path.dirname(configPath);
  const rulesDir = path.join(configDir, "RULES");
  await fsPromises.mkdir(rulesDir, { recursive: true });

  const writeRulesFile = async (filePath: string, rules: string[]) => {
    const normalized = rules.map((item) => item.trim()).filter(Boolean);
    const content =
      normalized.length > 0
        ? normalized.map((item) => `- ${item}`).join("\n") + "\n"
        : "# Empty rules\n";
    await fsPromises.writeFile(filePath, content, "utf8");
  };

  if (payload.globalRules) {
    await writeRulesFile(path.join(rulesDir, "global.md"), payload.globalRules);
  }
  if (payload.commonRules) {
    await writeRulesFile(path.join(rulesDir, "common.md"), payload.commonRules);
  }
  if (payload.phaseRules) {
    for (const [phase, rules] of Object.entries(payload.phaseRules)) {
      const phaseName = phase.trim();
      if (!phaseName) {
        continue;
      }
      await writeRulesFile(path.join(rulesDir, `${phaseName}.md`), rules || []);
    }
  }

  return rulesDir;
}

/**
 * Encapsulates all module-level state in a single object so we avoid
 * 7 mutable module-level variables (fixes BLUE65 V7).
 */
interface GoOnExtensionState {
  manager: GoOnManager;
  statusProvider: GoOnStatusProvider;
  output: vscode.OutputChannel;
  approvalPanel: ApprovalPanelProvider;
  chat: GoOnChatViewProvider;
  settings: GoOnSettingsViewProvider;
  runtimeBootstrap: RuntimeBootstrapDeps;
}

// Single module-level state holder, initialized in activate()
let extensionState: GoOnExtensionState | undefined;

function getState(): GoOnExtensionState {
  if (!extensionState) {
    throw new Error(
      "extensionState not initialized. activate() must be called first.",
    );
  }
  return extensionState;
}

/**
 * Initialize the i18n system and log the current UI language.
 */
function initI18n(): void {
  const currentLanguage = i18n.getCurrentLanguage();
  getState().output.appendLine(`UI Language: ${currentLanguage}`);
}

/**
 * Register all webview view providers.
 */
function registerViewProviders(context: vscode.ExtensionContext): void {
  const state = getState();
  const chat = new GoOnChatViewProvider(
    context.extensionUri,
    state.manager,
    context,
    async () => {
      await prepareRuntimeAndStartFromChat(context, state.runtimeBootstrap);
    },
  );
  const settings = new GoOnSettingsViewProvider(
    context.extensionUri,
    state.manager,
    context,
  );
  const workflowProvider = new GoOnWorkflowViewProvider(
    context.extensionUri,
    state.manager,
    context,
  );
  const processFlowProvider = new GoOnProcessFlowViewProvider(
    context.extensionUri,
    state.manager,
    context,
  );
  const approvalPanel = new ApprovalPanelProvider(
    context.extensionUri,
    state.manager,
  );

  // Update extension state with view providers
  extensionState = {
    ...state,
    approvalPanel,
    chat,
    settings,
  };

  context.subscriptions.push(
    vscode.window.registerWebviewViewProvider(
      GoOnChatViewProvider.viewType,
      chat,
    ),
    vscode.window.registerWebviewViewProvider(
      GoOnSettingsViewProvider.viewType,
      settings,
    ),
    vscode.window.registerWebviewViewProvider(
      GoOnWorkflowViewProvider.viewType,
      workflowProvider,
    ),
    vscode.window.registerWebviewViewProvider(
      GoOnProcessFlowViewProvider.viewType,
      processFlowProvider,
    ),
    vscode.window.registerWebviewViewProvider(
      ApprovalPanelProvider.viewType,
      approvalPanel,
    ),
  );
}

/**
 * Register all extension commands.
 */
function registerCommands(
  context: vscode.ExtensionContext,
  statusMonitor: StatusMonitor,
): void {
  const state = getState();

  context.subscriptions.push(
    vscode.commands.registerCommand("go-on.openConfigWizard", async () => {
      await state.settings.showConfigWizard();
    }),
  );

  context.subscriptions.push(
    vscode.window.registerTreeDataProvider(
      "go-on-status",
      state.statusProvider,
    ),
  );

  const coreCommands = registerCoreCommands({
    context,
    ensureBinary: ensureGoOnBinary,
    resolveConfigPath,
    parseMissingEnvVariableNames,
    buildPlaceholderEnvValues,
    start: (
      configPath: string,
      executablePath: string,
      cwd: string,
      protocolMode: string,
    ) => state.manager.start(configPath, executablePath, cwd, protocolMode),
    stop: () => state.manager.stop(),
    isRunning: () => state.manager.isRunning(),
    sendRequest: (method: string, params?: unknown) =>
      state.manager.sendRequest(method, params),
    setRuntimeEnvOverrides: (overrides: Record<string, string>) =>
      state.manager.setRuntimeEnvOverrides(overrides),
  });

  const viewCommands = registerViewCommands({
    revealGoOnView,
    ensureBinaryReady: async () => {
      const config = vscode.workspace.getConfiguration("go-on");
      const workspaceRoot = getPrimaryWorkspaceRoot()?.fsPath;
      await ensureGoOnBinary(workspaceRoot, config, context);
    },
    prepareRuntimeAfterChatOpen: async () =>
      prepareRuntimeAndStartFromChat(context, state.runtimeBootstrap),
    isRunning: () => state.manager.isRunning(),
    stop: () => state.manager.stop(),
    createSession: (sessionName: string) =>
      state.chat.createNewSession(sessionName),
    switchSession: (sessionName: string) =>
      state.chat.switchSession(sessionName),
    clearChat: () => state.chat.clearChat(),
    exportChat: () => state.chat.exportChat(),
    sendRequest: (method: string, params?: unknown) =>
      state.manager.sendRequest(method, params),
  });
  const rpcCommands = registerRpcCommands({
    isRunning: () => state.manager.isRunning(),
    sendRequest: (method: string, params?: unknown) =>
      state.manager.sendRequest(method, params),
  });

  // Refresh status monitor command (internal)
  const refreshStatusMonitorCommand = vscode.commands.registerCommand(
    "go-on.refreshStatusMonitor",
    () => {
      statusMonitor.refresh();
    },
  );

  // Internal command called by GoOnManager.updateStatus() to refresh tree data
  const refreshStatusTreeCommand = vscode.commands.registerCommand(
    "go-on-status.refresh",
    () => {
      state.statusProvider.refresh();
    },
  );

  const keyringSetCommand = vscode.commands.registerCommand(
    "go-on.keyringSet",
    async (payload?: { name?: string; value?: string }) => {
      try {
        const name = payload?.name;
        const value = payload?.value;
        if (!name || value === undefined) {
          throw new Error("keyring set requires name and value");
        }
        await runGoOnSecretCommand(context, "set", name, value);
      } catch (error: unknown) {
        const message = error instanceof Error ? error.message : String(error);
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.keyringSetFailed, [message]),
        );
      }
    },
  );

  const keyringGetCommand = vscode.commands.registerCommand(
    "go-on.keyringGet",
    async (payload?: { name?: string }) => {
      try {
        const name = payload?.name;
        if (!name) {
          throw new Error("keyring get requires name");
        }
        return await runGoOnSecretCommand(context, "get", name);
      } catch (error: unknown) {
        const message = error instanceof Error ? error.message : String(error);
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.keyringGetFailed, [message]),
        );
        return undefined;
      }
    },
  );

  const keyringDeleteCommand = vscode.commands.registerCommand(
    "go-on.keyringDelete",
    async (payload?: { name?: string }) => {
      try {
        const name = payload?.name;
        if (!name) {
          throw new Error("keyring delete requires name");
        }
        await runGoOnSecretCommand(context, "delete", name);
      } catch (error: unknown) {
        const message = error instanceof Error ? error.message : String(error);
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.keyringDeleteFailed, [message]),
        );
      }
    },
  );

  const keyringListCommand = vscode.commands.registerCommand(
    "go-on.keyringList",
    async () => {
      try {
        return await runGoOnSecretCommand(context, "list");
      } catch (error: unknown) {
        const message = error instanceof Error ? error.message : String(error);
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.keyringListFailed, [message]),
        );
        return undefined;
      }
    },
  );

  const applyDefaultConfigCommand = vscode.commands.registerCommand(
    "go-on.applyDefaultConfigTemplate",
    async (payload?: { template?: string }) => {
      try {
        const template = payload?.template;
        if (!template) {
          throw new Error("template is required");
        }
        const configPath = await applyDefaultConfigTemplate(context, template);
        return configPath;
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.templateRequired),
        );
        return undefined;
      }
    },
  );

  const updateWorkflowMappingCommand = vscode.commands.registerCommand(
    "go-on.updateWorkflowMapping",
    async (payload?: {
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
    }) => {
      if (!payload) {
        throw new Error(i18n.getMessage(MessageKeys.workflowMappingRequired));
      }
      return await updateWorkflowMappingConfig(context, payload);
    },
  );

  const updateRulesCommand = vscode.commands.registerCommand(
    "go-on.updateRules",
    async (payload?: {
      globalRules?: string[];
      commonRules?: string[];
      phaseRules?: Record<string, string[]>;
    }) => {
      if (!payload) {
        throw new Error(i18n.getMessage(MessageKeys.rulesPayloadRequired));
      }
      return await updateRulesMarkdownFiles(context, payload);
    },
  );

  context.subscriptions.push(
    ...coreCommands,
    ...viewCommands,
    ...rpcCommands,
    refreshStatusMonitorCommand,
    refreshStatusTreeCommand,
    keyringSetCommand,
    keyringGetCommand,
    keyringDeleteCommand,
    keyringListCommand,
    applyDefaultConfigCommand,
    updateWorkflowMappingCommand,
    updateRulesCommand,
  );
}

export function activate(context: vscode.ExtensionContext) {
  const output = vscode.window.createOutputChannel("Go-On");
  context.subscriptions.push(output);
  output.appendLine("Go-On extension activated");

  // Initialize extension state with output channel (other fields filled below)
  extensionState = {
    manager: undefined as unknown as GoOnManager,
    statusProvider: undefined as unknown as GoOnStatusProvider,
    output,
    approvalPanel: undefined as unknown as ApprovalPanelProvider,
    chat: undefined as unknown as GoOnChatViewProvider,
    settings: undefined as unknown as GoOnSettingsViewProvider,
    runtimeBootstrap: undefined as unknown as RuntimeBootstrapDeps,
  };

  // Initialize i18n system
  initI18n();

  // Initialize config manager
  const config = vscode.workspace.getConfiguration("go-on");
  const configPath = config.get<string>("configPath", "./config.toml");

  /**
   * Re-evaluate the config file path based on the current workspace roots
   * and re-initialize the config manager. Called when workspace folders change.
   */
  async function updateConfigPath(): Promise<void> {
    const cfg = vscode.workspace.getConfiguration("go-on");
    const newPath = cfg.get<string>("configPath", "./config.toml");
    output.appendLine(`Config path updated: ${newPath}`);
    try {
      await configManager.initialize(newPath);
    } catch (err) {
      const errMsg = err instanceof Error ? err.message : String(err);
      output.appendLine(`warn: config manager re-init failed: ${errMsg}`);
    }
  }

  // Initialize config manager and GoOnManager
  (async () => {
    try {
      await configManager.initialize(configPath);
      output.appendLine("config manager initialized");
    } catch (err) {
      const errMsg = err instanceof Error ? err.message : String(err);
      output.appendLine(`warn: config manager init failed: ${errMsg}`);
      void vscode.window.showWarningMessage(
        i18n.getMessage(MessageKeys.runtimeInitFailed, [errMsg]),
      );
    }

    const manager = new GoOnManager();
    manager.setOutputChannel(output);

    // status monitor and view providers must be created after manager is initialized
    const statusMonitor = new StatusMonitor(manager);
    context.subscriptions.push(statusMonitor);

    const statusProv = new GoOnStatusProvider(manager);

    const runtimeBootstrap: RuntimeBootstrapDeps = {
      ensureBinary: ensureGoOnBinary,
      isRunning: () => manager.isRunning(),
      startCommandId: "go-on.start",
    };

    // Initialize extension state with manager and runtime deps
    extensionState = {
      ...extensionState!,
      manager,
      statusProvider: statusProv,
      runtimeBootstrap,
    };

    // Initialize advanced edit provider
    new GoOnAdvancedEditProvider(manager, context);

    // Register webview providers
    registerViewProviders(context);

    // Register all commands
    registerCommands(context, statusMonitor);

    // Delayed check for provider readiness (warn user if API key is missing)
    // ── Start cross-client state sync SSE listener ─────────────────
    // BLUE67-E1: Wire SSE state sync listener to VSCode UI actions
    const baseUrl = protocolContract.runtime.baseUrl;
    const stopStateSync = startStateSyncListener(
      baseUrl,
      {
        onConfigReloaded(changedKeys) {
          const summary =
            changedKeys.length > 0 ? changedKeys.join(", ") : "configuration";
          output.appendLine(`[state-sync] Config reloaded: ${summary}`);
          vscode.window.setStatusBarMessage(
            `Go-On: config reloaded (${summary})`,
            4000,
          );
        },
        onModelsChanged(models) {
          output.appendLine(
            `[state-sync] Models changed: ${models.length} models`,
          );
          vscode.window.setStatusBarMessage(
            `Go-On: models updated (${models.length} models)`,
            3000,
          );
        },
        onAgentsChanged(added, removed) {
          if (added.length > 0) {
            output.appendLine(`[state-sync] Agents added: ${added.join(", ")}`);
          }
          if (removed.length > 0) {
            output.appendLine(
              `[state-sync] Agents removed: ${removed.join(", ")}`,
            );
          }
          vscode.window.setStatusBarMessage(
            `Go-On: agents changed (+${added.length}/-${removed.length})`,
            3000,
          );
        },
        onBackendRestarting(reason, _restartInMs) {
          output.appendLine(`[state-sync] Backend restarting: ${reason}`);
          vscode.window.showWarningMessage(
            `Go-On backend restarting: ${reason}`,
          );
        },
        onHeartbeat(_timestamp) {
          // Heartbeat is informational only
        },
      },
      output,
    );
    context.subscriptions.push({
      dispose: stopStateSync,
    });

    const providerReadyTimer = setTimeout(async () => {
      try {
        const { manager: m } = getState();
        const backendRunning = m.isRunning();
        if (!backendRunning) return;
        const ready = await m.isAnyAiProviderReady();
        if (!ready) {
          const openSettingsLabel = i18n.getMessage(MessageKeys.openSettings);
          const action = await vscode.window.showWarningMessage(
            i18n.getMessage(MessageKeys.apiKeyMissing),
            openSettingsLabel,
            i18n.getMessage(MessageKeys.later),
          );
          if (action === openSettingsLabel) {
            vscode.commands.executeCommand("go-on.openSettings");
          }
        }
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn("go-on: backend readiness check failed:", err);
      }
    }, 3000);
    context.subscriptions.push(
      new vscode.Disposable(() => clearTimeout(providerReadyTimer)),
    );

    // Open chat automatically only once (controlled by go-on.autoOpenChat config).
    const autoOpenChat = vscode.workspace
      .getConfiguration("go-on")
      .get<boolean>("autoOpenChat", true);
    const hasOpenedChat = context.globalState.get<boolean>(
      "go-on.hasOpenedChatOnce",
      false,
    );
    if (autoOpenChat && !hasOpenedChat) {
      const autoOpenTimer = setTimeout(async () => {
        try {
          await vscode.commands.executeCommand("go-on.openChat");
          await context.globalState.update("go-on.hasOpenedChatOnce", true);
        } catch (err) {
          // eslint-disable-next-line no-console
          console.warn("[extension] autoOpenChat failed:", err);
        }
      }, 300);
      context.subscriptions.push(
        new vscode.Disposable(() => clearTimeout(autoOpenTimer)),
      );
    }

    // Listen for workspace folder changes and re-evaluate config path
    context.subscriptions.push(
      vscode.workspace.onDidChangeWorkspaceFolders((event) => {
        output.appendLine(
          `Workspace folders changed: ${event.added.length} added, ${event.removed.length} removed`,
        );
        // Re-evaluate the config file path based on new workspace roots
        updateConfigPath();
      }),
    );
  })().catch(async (err) => {
    const diagnosticData = {
      error: String(err),
      workspaceFolders: vscode.workspace.workspaceFolders?.map(
        (f) => f.uri.fsPath,
      ),
      extensionVersion: context.extension.packageJSON.version,
      platform: process.platform,
      timestamp: new Date().toISOString(),
    };
    // eslint-disable-next-line no-console
    console.error(
      "Activation failed:",
      JSON.stringify(diagnosticData, null, 2),
    );

    // Retry activation once after a short delay
    const retryDelayMs = 2000;
    output.appendLine(`Activation failed, retrying in ${retryDelayMs}ms...`);
    await new Promise((resolve) => setTimeout(resolve, retryDelayMs));

    try {
      await (async () => {
        // Re-run the inline activation block from above.
        // The closure captures `context`, `output`, `configPath`, and `updateConfigPath`
        // from the outer scope. We re-execute the init sequence inline.
        const cfg = vscode.workspace.getConfiguration("go-on");
        const retryConfigPath = cfg.get<string>("configPath", "./config.toml");
        output.appendLine("Retrying activation...");

        try {
          await configManager.initialize(retryConfigPath);
          output.appendLine("config manager initialized (retry)");
        } catch (initErr) {
          const initErrMsg =
            initErr instanceof Error ? initErr.message : String(initErr);
          output.appendLine(
            `warn: config manager init failed (retry): ${initErrMsg}`,
          );
        }

        const retryManager = new GoOnManager();
        retryManager.setOutputChannel(output);

        const retryStatusMonitor = new StatusMonitor(retryManager);
        context.subscriptions.push(retryStatusMonitor);

        const retryStatusProv = new GoOnStatusProvider(retryManager);

        const retryRuntimeBootstrap: RuntimeBootstrapDeps = {
          ensureBinary: ensureGoOnBinary,
          isRunning: () => retryManager.isRunning(),
          startCommandId: "go-on.start",
        };

        extensionState = {
          ...extensionState!,
          manager: retryManager,
          statusProvider: retryStatusProv,
          runtimeBootstrap: retryRuntimeBootstrap,
        };

        // ── Start cross-client state sync SSE listener (retry) ─
        const retryBaseUrl = protocolContract.runtime.baseUrl;
        const retryStopStateSync = startStateSyncListener(
          retryBaseUrl,
          {
            onConfigReloaded(changedKeys) {
              const summary =
                changedKeys.length > 0
                  ? changedKeys.join(", ")
                  : "configuration";
              output.appendLine(
                `[state-sync] Config reloaded (retry): ${summary}`,
              );
              vscode.window.setStatusBarMessage(
                `Go-On: config reloaded (${summary})`,
                4000,
              );
            },
            onModelsChanged(models) {
              output.appendLine(
                `[state-sync] Models changed (retry): ${models.length} models`,
              );
              vscode.window.setStatusBarMessage(
                `Go-On: models updated (${models.length} models)`,
                3000,
              );
            },
            onAgentsChanged(added, removed) {
              vscode.window.setStatusBarMessage(
                `Go-On: agents changed (+${added.length}/-${removed.length})`,
                3000,
              );
            },
            onBackendRestarting(reason) {
              vscode.window.showWarningMessage(
                `Go-On backend restarting: ${reason}`,
              );
            },
            onHeartbeat() {},
          },
          output,
        );
        context.subscriptions.push({ dispose: retryStopStateSync });

        new GoOnAdvancedEditProvider(retryManager, context);
        registerViewProviders(context);
        registerCommands(context, retryStatusMonitor);

        output.appendLine("Activation completed on retry.");
      })();
    } catch (retryErr) {
      output.appendLine(`Activation retry also failed: ${retryErr}`);
      vscode.window.showErrorMessage(
        `go-on activation failed after retry: ${err}. ${retryErr}`,
      );
    }
  });
}

export async function deactivate(): Promise<void> {
  const state = extensionState;
  if (state) {
    if (state.manager) {
      await state.manager.stop();
    }
    if (state.approvalPanel) {
      state.approvalPanel.dispose();
    }
  }
  disposeLogger();
}
