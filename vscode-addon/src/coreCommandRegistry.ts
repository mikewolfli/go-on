import * as vscode from "vscode";
import * as fs from "fs";
import { i18n, MessageKeys } from "./i18n";
import { RuntimeResolution } from "./runtimeBinaryService";

interface CoreCommandRegistryDeps {
  context: vscode.ExtensionContext;
  ensureBinary: (
    _workspaceRoot: string | undefined,
    _config: vscode.WorkspaceConfiguration,
    _context: vscode.ExtensionContext,
  ) => Promise<RuntimeResolution>;
  resolveConfigPath: (
    _workspaceRoot: string,
    _configuredConfigPath: string,
    _runtimeDir: string,
  ) => Promise<string>;
  parseMissingEnvVariableNames: (_message: string) => string[];
  buildPlaceholderEnvValues: (
    _missingEnvVarNames: string[],
  ) => Record<string, string>;
  start: (
    _configPath: string,
    _executablePath: string,
    _cwd: string,
    _protocolMode: string,
  ) => Promise<void>;
  stop: () => void;
  isRunning: () => boolean;
  sendRequest: (_method: string, _params?: unknown) => Promise<unknown>;
  setRuntimeEnvOverrides: (_overrides: Record<string, string>) => void;
}

function getErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function asRecord(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null
    ? (value as Record<string, unknown>)
    : {};
}

async function ensureRunning(deps: CoreCommandRegistryDeps): Promise<boolean> {
  if (!deps.isRunning()) {
    await vscode.window.showErrorMessage(
      i18n.getMessage(MessageKeys.goOnNotRunningRpc),
    );
    return false;
  }
  return true;
}

export function registerCoreCommands(
  deps: CoreCommandRegistryDeps,
): vscode.Disposable[] {
  const diagnoseCommand = vscode.commands.registerCommand(
    "go-on.diagnose",
    async () => {
      const output = vscode.window.createOutputChannel("Go-On Diagnosis");
      deps.context.subscriptions.push(output);
      output.show(true);
      output.appendLine("=== Go-On Diagnosis Report ===");
      output.appendLine(`Time: ${new Date().toISOString()}`);

      const config = vscode.workspace.getConfiguration("go-on");
      const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
      if (!workspaceFolder) {
        output.appendLine(
          `✗ ${i18n.getMessage(MessageKeys.noWorkspaceFolderOpen)}`,
        );
        vscode.window.showWarningMessage(
          i18n.getMessage(MessageKeys.diagnosisIssue),
        );
        return;
      }

      output.appendLine(`Workspace: ${workspaceFolder.uri.fsPath}`);

      output.appendLine("\n1. Runtime binary check");
      let runtimeDir = "";
      let executablePath = "";
      try {
        const runtime = await deps.ensureBinary(
          workspaceFolder.uri.fsPath,
          config,
          deps.context,
        );
        runtimeDir = runtime.runtimeDir;
        executablePath = runtime.executablePath;
        const exists = fs.existsSync(executablePath);
        output.appendLine(
          `${exists ? "✓" : "✗"} Executable: ${executablePath}`,
        );
      } catch (error: unknown) {
        output.appendLine(
          `✗ Runtime resolve failed: ${getErrorMessage(error)}`,
        );
      }

      output.appendLine("\n2. Config file check");
      const configuredConfigPath = config.get<string>(
        "configPath",
        "./config.toml",
      );
      try {
        const resolvedConfigPath = await deps.resolveConfigPath(
          workspaceFolder.uri.fsPath,
          configuredConfigPath,
          runtimeDir || workspaceFolder.uri.fsPath,
        );
        const exists = fs.existsSync(resolvedConfigPath);
        output.appendLine(
          `${exists ? "✓" : "✗"} Config: ${resolvedConfigPath}`,
        );
      } catch (error: unknown) {
        output.appendLine(`✗ Config resolve failed: ${getErrorMessage(error)}`);
      }

      output.appendLine("\n3. Protocol mode check");
      const protocolMode = String(
        config.get("runtime.protocolMode", "from_config"),
      );
      output.appendLine(`Configured protocol mode: ${protocolMode}`);

      output.appendLine("\n4. Runtime health probe");
      if (!deps.isRunning()) {
        output.appendLine("! Go-On runtime is not running (skip RPC probe)");
      } else {
        try {
          const result = await deps.sendRequest("runtime.health");
          output.appendLine(`✓ runtime.health: ${JSON.stringify(result)}`);
        } catch (error: unknown) {
          output.appendLine(
            `✗ runtime.health failed: ${getErrorMessage(error)}`,
          );
        }
      }

      output.appendLine("\n=== Diagnosis Complete ===");
      vscode.window.showInformationMessage(
        i18n.getMessage(MessageKeys.diagnosisCompleted),
      );
    },
  );

  const startCommand = vscode.commands.registerCommand(
    "go-on.start",
    async () => {
      const config = vscode.workspace.getConfiguration("go-on");
      const configuredConfigPath = config.get<string>(
        "configPath",
        "./config.toml",
      );
      const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
      if (!workspaceFolder) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.noWorkspaceFolderOpen),
        );
        return;
      }

      const tryStart = async () => {
        const runtime = await deps.ensureBinary(
          workspaceFolder.uri.fsPath,
          config,
          deps.context,
        );
        const fullConfigPath = await deps.resolveConfigPath(
          workspaceFolder.uri.fsPath,
          configuredConfigPath,
          runtime.runtimeDir,
        );

        const protocolMode = config.get<string>(
          "runtime.protocolMode",
          "from_config",
        );
        await deps.start(
          fullConfigPath,
          runtime.executablePath,
          workspaceFolder.uri.fsPath,
          protocolMode,
        );
      };

      try {
        await tryStart();
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.goOnStarted),
        );
      } catch (error: unknown) {
        const errorMessage = getErrorMessage(error);
        const missingEnvVars = deps.parseMissingEnvVariableNames(errorMessage);
        if (missingEnvVars.length > 0) {
          try {
            const envValues = deps.buildPlaceholderEnvValues(missingEnvVars);
            deps.setRuntimeEnvOverrides(envValues);
            await tryStart();
            vscode.window.showWarningMessage(
              i18n.getMessage(MessageKeys.goOnStartedWithoutKeys),
            );
            return;
          } catch (retryError: unknown) {
            const retryMessage = getErrorMessage(retryError);
            const retryMissingEnvVars =
              deps.parseMissingEnvVariableNames(retryMessage);
            if (retryMissingEnvVars.length > 0) {
              // i18n
              vscode.window.showErrorMessage(
                i18n.getMessage(MessageKeys.goOnStartFailedMissingEnv, [
                  retryMissingEnvVars.join(", "),
                ]),
              );
            } else {
              // i18n
              vscode.window.showErrorMessage(
                i18n.getMessage(MessageKeys.goOnStartFailed, [retryMessage]),
              );
            }
            throw retryError;
          }
        }

        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.goOnStartFailed, [errorMessage]),
        );
        throw error;
      }
    },
  );

  const stopCommand = vscode.commands.registerCommand("go-on.stop", () => {
    deps.stop();
    vscode.window.showInformationMessage(
      i18n.getMessage(MessageKeys.goOnStopped),
    );
  });

  const sendRequestCommand = vscode.commands.registerCommand(
    "go-on.sendRequest",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }

      const message = await vscode.window.showInputBox({
        prompt: i18n.getMessage(MessageKeys.enterMessage),
        placeHolder: i18n.getMessage(MessageKeys.messagePlaceholder),
      });

      if (!message) {
        return;
      }

      try {
        const result = await deps.sendRequest("chat", {
          messages: [{ role: "user", content: message }],
        });
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.responseLabel, [JSON.stringify(result)]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.requestFailed, [getErrorMessage(error)]),
        );
      }
    },
  );

  const healthCheckCommand = vscode.commands.registerCommand(
    "go-on.healthCheck",
    async () => {
      try {
        const result = await deps.sendRequest("runtime.health");
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.healthCheckResult, [
            JSON.stringify(result),
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.healthCheckFailed, [
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const healthProbesCommand = vscode.commands.registerCommand(
    "go-on.healthProbes",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }

      try {
        const result = asRecord(await deps.sendRequest("health.probes"));
        const probes = asRecord(result.probes);
        const liveness = asRecord(probes.liveness);
        const readiness = asRecord(probes.readiness);
        const summary = asRecord(probes.summary);
        const locks = asRecord(probes.locks);
        const timeouts = asRecord(probes.timeouts);

        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.healthProbesLabel, [
            String(liveness.status ?? "unknown"),
            String(readiness.status ?? "unknown"),
            String(locks.status ?? "unknown"),
            String(locks.poisoned_total ?? 0),
            String(locks.slow_wait_total ?? 0),
            String(timeouts.status ?? "unknown"),
            String(timeouts.agent_request_total ?? 0),
            String(timeouts.review_gate_total ?? 0),
            String(timeouts.runtime_probe_total ?? 0),
            String(summary.error ?? 0),
            String(summary.warn ?? 0),
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.healthProbesFailed, [
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const lockStatusCommand = vscode.commands.registerCommand(
    "go-on.lockStatus",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }

      try {
        const result = asRecord(
          await deps.sendRequest("lock.status", { top_n: 5 }),
        );
        const locks = asRecord(result.locks);
        const contentionTop = Array.isArray(locks.contention_top)
          ? locks.contention_top
          : [];
        const topLabel =
          contentionTop.length > 0
            ? String(asRecord(contentionTop[0]).name ?? "-")
            : "-";

        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.lockStatusLabel, [
            String(locks.status ?? "unknown"),
            String(locks.components_tracked ?? 0),
            String(locks.poisoned_total ?? 0),
            String(locks.recovered_total ?? 0),
            String(locks.slow_wait_total ?? 0),
            Number(locks.max_wait_ms ?? 0).toFixed(3),
            topLabel,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.lockStatusFailed, [
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const breakerStatusCommand = vscode.commands.registerCommand(
    "go-on.breakerStatus",
    async () => {
      try {
        const result = await deps.sendRequest("breaker.status");
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.breakerStatusResult, [
            JSON.stringify(result),
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.breakerStatusFailed, [
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const cacheClearCommand = vscode.commands.registerCommand(
    "go-on.cacheClear",
    async () => {
      try {
        await deps.sendRequest("cache.clear");
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.cacheCleared),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.cacheClearFailed, [
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const vectorClearCommand = vscode.commands.registerCommand(
    "go-on.vectorClear",
    async () => {
      try {
        await deps.sendRequest("vector.clear");
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.vectorCleared),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.vectorClearFailed, [
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const configReloadCommand = vscode.commands.registerCommand(
    "go-on.configReload",
    async () => {
      try {
        await deps.sendRequest("config.reload");
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.configReloaded),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.configReloadFailed, [
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const shutdownCommand = vscode.commands.registerCommand(
    "go-on.shutdown",
    async () => {
      try {
        await deps.sendRequest("shutdown");
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.goOnShutdown),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.goOnShutdownFailed, [
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  return [
    diagnoseCommand,
    startCommand,
    stopCommand,
    sendRequestCommand,
    healthCheckCommand,
    healthProbesCommand,
    lockStatusCommand,
    breakerStatusCommand,
    cacheClearCommand,
    vectorClearCommand,
    configReloadCommand,
    shutdownCommand,
  ];
}
