import * as vscode from "vscode";
import { spawn } from "child_process";
import * as path from "path";
import * as fsPromises from "fs/promises";
import { GoOnChatViewProvider } from "./chatView";
import { GoOnSettingsViewProvider } from "./settingsView";
import { StatusMonitor } from "./statusMonitor";
import { GoOnWorkflowViewProvider } from "./workflowView";
import { GoOnProcessFlowViewProvider } from "./processFlowView";
import { GoOnAdvancedEditProvider } from "./advancedEdit";
import { i18n, MessageKeys } from "./i18n";
import { configManager } from "./configManager";
import { revealGoOnView } from "./viewRouter";
import { registerViewCommands } from "./commandRegistry";
import { registerRpcCommands } from "./rpcCommandRegistry";
import { registerCoreCommands } from "./coreCommandRegistry";
import { GoOnManager, GoOnStatusProvider } from "./runtimeManager";
import {
  ensureGoOnBinary,
  ensureProvidersTomlForConfig,
  pathExists,
  resolveConfigPath,
} from "./runtimeBinaryService";
import {
  buildPlaceholderEnvValues,
  parseMissingEnvVariableNames,
  prepareRuntimeAndStartFromChat,
  RuntimeBootstrapDeps,
} from "./runtimeBootstrap";

async function runGoOnSecretCommand(
  context: vscode.ExtensionContext,
  action: "set" | "get" | "delete" | "list",
  secretName?: string,
  secretValue?: string,
): Promise<string> {
  const config = vscode.workspace.getConfiguration("go-on");
  const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  const runtime = await ensureGoOnBinary(workspaceRoot, config, context);

  const args: string[] = ["--secret", action];
  if (secretName) {
    args.push("--secret-name", secretName);
  }
  if (secretValue !== undefined) {
    args.push("--secret-value", secretValue);
  }

  return new Promise<string>((resolve, reject) => {
    const proc = spawn(runtime.executablePath, args, {
      cwd: workspaceRoot || runtime.runtimeDir,
      stdio: ["ignore", "pipe", "pipe"],
    });

    let stdout = "";
    let stderr = "";
    let timedOut = false;

    const timeoutHandle = setTimeout(() => {
      timedOut = true;
      proc.kill("SIGTERM");
      // Give it a moment to terminate, then force kill
      setTimeout(() => {
        try {
          proc.kill("SIGKILL");
        } catch {
          // process already terminated
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

function escapeRegex(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function formatTomlStringList(items: string[]): string {
  return `[${items.map((item) => `"${item}"`).join(", ")}]`;
}

function formatTomlMultilineStringList(items: string[]): string {
  const lines = items.map((item) => `    "${item.replace(/"/g, '\\"')}"`);
  return `[
${lines.join(",\n")}
]`;
}

function upsertSectionLine(
  section: string,
  lineRegex: RegExp,
  line: string,
): string {
  if (lineRegex.test(section)) {
    return section.replace(lineRegex, line);
  }
  const lines = section.split("\n");
  lines.splice(1, 0, line);
  return lines.join("\n");
}

function upsertTopLevelString(
  content: string,
  key: string,
  value: string,
): string {
  const regex = new RegExp(`^${escapeRegex(key)}\\s*=\\s*".*"\\s*$`, "m");
  const replacement = `${key} = "${value}"`;
  if (regex.test(content)) {
    return content.replace(regex, replacement);
  }
  return `${replacement}\n${content}`;
}

function upsertFlowPhases(content: string, phases: string[]): string {
  const flowSectionRegex = /\[flow\][\s\S]*?(?=\n\[[^\]]+\]|$)/;
  const phasesLine = `phases = ${formatTomlStringList(phases)}`;
  if (flowSectionRegex.test(content)) {
    return content.replace(flowSectionRegex, (section) => {
      const phasesRegex = /^phases\s*=\s*\[[^\]]*\]\s*$/m;
      if (phasesRegex.test(section)) {
        return section.replace(phasesRegex, phasesLine);
      }
      const trimmed = section.trimEnd();
      return `${trimmed}\n${phasesLine}\n`;
    });
  }

  return `${content.trimEnd()}\n\n[flow]\nname = "Configured Flow"\n${phasesLine}\n`;
}

function upsertPhaseAgents(
  content: string,
  phase: string,
  agents: string[],
): string {
  const header = `[phases.${phase}]`;
  const escapedHeader = escapeRegex(header);
  const sectionRegex = new RegExp(
    `^${escapedHeader}[\\s\\S]*?(?=^\\[[^\\]]+\\]|\\Z)`,
    "m",
  );
  const agentsLine = `agents = ${formatTomlStringList(agents)}`;

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

function upsertPhaseFallback(
  content: string,
  phase: string,
  fallback: boolean,
): string {
  const header = `[phases.${phase}]`;
  const escapedHeader = escapeRegex(header);
  const sectionRegex = new RegExp(
    `^${escapedHeader}[\\s\\S]*?(?=^\\[[^\\]]+\\]|\\Z)`,
    "m",
  );
  const fallbackLine = `fallback = ${fallback ? "true" : "false"}`;

  if (sectionRegex.test(content)) {
    return content.replace(sectionRegex, (section) =>
      upsertSectionLine(
        section,
        /^fallback\s*=\s*(true|false)\s*$/m,
        fallbackLine,
      ),
    );
  }

  return `${content.trimEnd()}\n\n${header}\ndescription = "${phase} phase"\nagents = ["copilot"]\n${fallbackLine}\n`;
}

function upsertPhasePrinciples(
  content: string,
  phase: string,
  principles: string[],
): string {
  const header = `[phases.${phase}]`;
  const escapedHeader = escapeRegex(header);
  const sectionRegex = new RegExp(
    `^${escapedHeader}[\\s\\S]*?(?=^\\[[^\\]]+\\]|\\Z)`,
    "m",
  );
  const principlesLine = `principles = ${formatTomlMultilineStringList(principles)}`;

  if (sectionRegex.test(content)) {
    return content.replace(sectionRegex, (section) => {
      const principlesRegex = /^principles\s*=\s*\[[\s\S]*?\]\s*$/m;
      if (principlesRegex.test(section)) {
        return section.replace(principlesRegex, principlesLine);
      }
      return upsertSectionLine(
        section,
        /^principles\s*=\s*\[[\s\S]*?\]\s*$/m,
        principlesLine,
      );
    });
  }

  return `${content.trimEnd()}\n\n${header}\ndescription = "${phase} phase"\nagents = ["copilot"]\nfallback = true\n${principlesLine}\n`;
}

function upsertPhaseOptionNumber(
  content: string,
  phase: string,
  optionKey: string,
  value: number,
): string {
  const optionHeader = `[phases.${phase}.options]`;
  const escapedOptionHeader = escapeRegex(optionHeader);
  const optionSectionRegex = new RegExp(
    `^${escapedOptionHeader}[\\s\\S]*?(?=^\\[[^\\]]+\\]|\\Z)`,
    "m",
  );
  const optionLine = `${optionKey} = ${value}`;
  const keyRegex = new RegExp(
    `^${escapeRegex(optionKey)}\\s*=\\s*\\d+\\s*$`,
    "m",
  );

  if (optionSectionRegex.test(content)) {
    return content.replace(optionSectionRegex, (section) =>
      upsertSectionLine(section, keyRegex, optionLine),
    );
  }

  return `${content.trimEnd()}\n\n${optionHeader}\n${optionLine}\n`;
}

async function resolveConfigFilePath(
  context: vscode.ExtensionContext,
  configuredConfigPath?: string,
): Promise<{ workspaceRoot: string; configPath: string; runtimeDir: string }> {
  const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
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
  await ensureProvidersTomlForConfig(workspaceRoot, runtimeDir, configPath);
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

let goOnManager: GoOnManager;
let statusProvider: GoOnStatusProvider;
let goOnOutput: vscode.OutputChannel;

export function activate(context: vscode.ExtensionContext) {
  goOnOutput = vscode.window.createOutputChannel("Go-On");
  context.subscriptions.push(goOnOutput);
  goOnOutput.appendLine("Go-On extension activated");

  // Initialize i18n system
  const currentLanguage = i18n.getCurrentLanguage();
  goOnOutput.appendLine(`UI Language: ${currentLanguage}`);

  // Initialize config manager
  const config = vscode.workspace.getConfiguration("go-on");
  const configPath = config.get<string>("configPath", "./config.toml");
  // Sync VS Code language to app configuration
  syncLanguageToApp(currentLanguage);

  // Initialize config manager and GoOnManager
  (async () => {
    try {
      await configManager.initialize(configPath);
      goOnOutput.appendLine("config manager initialized");
    } catch (err) {
      const errMsg = err instanceof Error ? err.message : String(err);
      goOnOutput.appendLine(`warn: config manager init failed: ${errMsg}`);
      void vscode.window.showWarningMessage(
        i18n.getMessage(MessageKeys.runtimeInitFailed, [errMsg]),
      );
    }

    goOnManager = new GoOnManager();
    goOnManager.setOutputChannel(goOnOutput);

    // status monitor and view providers must be created after goOnManager is initialized
    const statusMonitor = new StatusMonitor(goOnManager);
    context.subscriptions.push(statusMonitor);

    statusProvider = new GoOnStatusProvider(goOnManager);

    const runtimeBootstrapDeps: RuntimeBootstrapDeps = {
      ensureBinary: ensureGoOnBinary,
      isRunning: () => goOnManager.isRunning(),
      startCommandId: "go-on.start",
    };

    // Initialize advanced edit provider
    new GoOnAdvancedEditProvider(goOnManager, context);

    // Register webview providers
    const chatProvider = new GoOnChatViewProvider(
      context.extensionUri,
      goOnManager,
      context,
      async () => {
        await prepareRuntimeAndStartFromChat(context, runtimeBootstrapDeps);
      },
    );
    const settingsProvider = new GoOnSettingsViewProvider(
      context.extensionUri,
      goOnManager,
      context,
    );
    const workflowProvider = new GoOnWorkflowViewProvider(
      context.extensionUri,
      goOnManager,
      context,
    );
    const processFlowProvider = new GoOnProcessFlowViewProvider(
      context.extensionUri,
      goOnManager,
      context,
    );

    context.subscriptions.push(
      vscode.window.registerWebviewViewProvider(
        GoOnChatViewProvider.viewType,
        chatProvider,
      ),
      vscode.window.registerWebviewViewProvider(
        GoOnSettingsViewProvider.viewType,
        settingsProvider,
      ),
      vscode.window.registerWebviewViewProvider(
        GoOnWorkflowViewProvider.viewType,
        workflowProvider,
      ),
      vscode.window.registerWebviewViewProvider(
        GoOnProcessFlowViewProvider.viewType,
        processFlowProvider,
      ),
    );

    context.subscriptions.push(
      vscode.commands.registerCommand("go-on.openConfigWizard", async () => {
        await settingsProvider.showConfigWizard();
      }),
    );

    context.subscriptions.push(
      vscode.window.registerTreeDataProvider("go-on-status", statusProvider),
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
      ) => goOnManager.start(configPath, executablePath, cwd, protocolMode),
      stop: () => goOnManager.stop(),
      isRunning: () => goOnManager.isRunning(),
      sendRequest: (method: string, params?: unknown) =>
        goOnManager.sendRequest(method, params),
      setRuntimeEnvOverrides: (overrides: Record<string, string>) =>
        goOnManager.setRuntimeEnvOverrides(overrides),
    });

    const viewCommands = registerViewCommands({
      revealGoOnView,
      ensureBinaryReady: async () => {
        const config = vscode.workspace.getConfiguration("go-on");
        const workspaceRoot =
          vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
        await ensureGoOnBinary(workspaceRoot, config, context);
      },
      prepareRuntimeAfterChatOpen: async () =>
        prepareRuntimeAndStartFromChat(context, runtimeBootstrapDeps),
      isRunning: () => goOnManager.isRunning(),
      stop: () => goOnManager.stop(),
      createSession: (sessionName: string) =>
        chatProvider.createNewSession(sessionName),
      switchSession: (sessionName: string) =>
        chatProvider.switchSession(sessionName),
      clearChat: () => chatProvider.clearChat(),
      exportChat: () => chatProvider.exportChat(),
      sendRequest: (method: string, params?: unknown) =>
        goOnManager.sendRequest(method, params),
    });
    const rpcCommands = registerRpcCommands({
      isRunning: () => goOnManager.isRunning(),
      sendRequest: (method: string, params?: unknown) =>
        goOnManager.sendRequest(method, params),
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
        statusProvider.refresh();
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
          const message =
            error instanceof Error ? error.message : String(error);
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
          const message =
            error instanceof Error ? error.message : String(error);
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
          const message =
            error instanceof Error ? error.message : String(error);
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
          const message =
            error instanceof Error ? error.message : String(error);
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
          const configPath = await applyDefaultConfigTemplate(
            context,
            template,
          );
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

    // Runtime download/start is intentionally deferred until the Chat view is opened.

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

    // Delayed check for provider readiness (warn user if API key is missing)
    const providerReadyTimer = setTimeout(async () => {
      try {
        if (goOnManager) {
          const backendRunning = goOnManager.isRunning();
          if (!backendRunning) return;
          const ready = await goOnManager.isAnyAiProviderReady();
          if (!ready) {
            const action = await vscode.window.showWarningMessage(
              "AI provider is configured but no API key is set. " +
                "Go-On needs an API key to function.",
              "Open Settings",
              "Later",
            );
            if (action === "Open Settings") {
              vscode.commands.executeCommand("go-on.openSettings");
            }
          }
        }
      } catch {
        // backend may not be running yet, ignore
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
        } catch {
          // Auto-open failed (e.g., binary not found) — don't mark as opened so it retries next time
        }
      }, 300);
      context.subscriptions.push(
        new vscode.Disposable(() => clearTimeout(autoOpenTimer)),
      );
    }
  })();
}

export function deactivate() {
  if (goOnManager) goOnManager.stop();
}

/**
 * Sync VS Code language with app configuration
 * This ensures the app uses the same language as VS Code
 */
async function syncLanguageToApp(language: string): Promise<void> {
  try {
    const workspaceFolder = vscode.workspace.workspaceFolders?.[0];

    if (!workspaceFolder) {
      return;
    }

    // Log successful sync
    goOnOutput.appendLine(
      `Language synchronized: VS Code ${language} -> App ${language}`,
    );
  } catch (error) {
    goOnOutput.appendLine(`warn: language sync failed: ${error}`);
  }
}
