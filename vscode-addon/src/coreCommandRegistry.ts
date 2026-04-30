import * as vscode from "vscode";
import * as fs from "fs";

interface RuntimeResolution {
  executablePath: string;
  runtimeDir: string;
}

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
    vscode.window.showErrorMessage("Go-On is not running. Start it first.");
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
        output.appendLine("✗ No workspace folder open");
        vscode.window.showWarningMessage(
          'Go-On diagnosis completed with issues. See "Go-On Diagnosis" output.',
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
      vscode.window.showInformationMessage("Go-On diagnosis completed.");
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
        vscode.window.showErrorMessage("No workspace folder open.");
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
        vscode.window.showInformationMessage("Go-On proxy started.");
      } catch (error: unknown) {
        const errorMessage = getErrorMessage(error);
        const missingEnvVars = deps.parseMissingEnvVariableNames(errorMessage);
        if (missingEnvVars.length > 0) {
          try {
            const envValues = deps.buildPlaceholderEnvValues(missingEnvVars);
            deps.setRuntimeEnvOverrides(envValues);
            await tryStart();
            vscode.window.showWarningMessage(
              "Go-On proxy started without API keys. Configure provider keys in Settings before using cloud agents.",
            );
            return;
          } catch (retryError: unknown) {
            const retryMessage = getErrorMessage(retryError);
            const retryMissingEnvVars =
              deps.parseMissingEnvVariableNames(retryMessage);
            if (retryMissingEnvVars.length > 0) {
              vscode.window.showErrorMessage(
                `Failed to start Go-On: missing environment variables (${retryMissingEnvVars.join(", ")}). Configure provider keys in Settings.`,
              );
            } else {
              vscode.window.showErrorMessage(
                `Failed to start Go-On: ${retryMessage}`,
              );
            }
            throw retryError;
          }
        }

        vscode.window.showErrorMessage(
          `Failed to start Go-On: ${errorMessage}`,
        );
        throw error;
      }
    },
  );

  const stopCommand = vscode.commands.registerCommand("go-on.stop", () => {
    deps.stop();
    vscode.window.showInformationMessage("Go-On proxy stopped.");
  });

  const sendRequestCommand = vscode.commands.registerCommand(
    "go-on.sendRequest",
    async () => {
      if (!(await ensureRunning(deps))) {
        return;
      }

      const message = await vscode.window.showInputBox({
        prompt: "Enter your message",
        placeHolder: "Type your chat message here...",
      });

      if (!message) {
        return;
      }

      try {
        const result = await deps.sendRequest("chat", {
          messages: [{ role: "user", content: message }],
        });
        vscode.window.showInformationMessage(
          `Response: ${JSON.stringify(result)}`,
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          `Request failed: ${getErrorMessage(error)}`,
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
          `Health: ${JSON.stringify(result)}`,
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          `Health check failed: ${getErrorMessage(error)}`,
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
          `health.probes: liveness=${String(liveness.status ?? "unknown")}, readiness=${String(readiness.status ?? "unknown")}, lock=${String(locks.status ?? "unknown")}, poisoned=${Number(locks.poisoned_total ?? 0)}, slow=${Number(locks.slow_wait_total ?? 0)}, timeout=${String(timeouts.status ?? "unknown")}, agent_timeout=${Number(timeouts.agent_request_total ?? 0)}, review_timeout=${Number(timeouts.review_gate_total ?? 0)}, probe_timeout=${Number(timeouts.runtime_probe_total ?? 0)}, error=${Number(summary.error ?? 0)}, warn=${Number(summary.warn ?? 0)}`,
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          `health.probes failed: ${getErrorMessage(error)}`,
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
          `lock.status: status=${String(locks.status ?? "unknown")}, tracked=${Number(locks.components_tracked ?? 0)}, poisoned=${Number(locks.poisoned_total ?? 0)}, recovered=${Number(locks.recovered_total ?? 0)}, slow_waits=${Number(locks.slow_wait_total ?? 0)}, max_wait_ms=${Number(locks.max_wait_ms ?? 0).toFixed(3)}, top=${topLabel}`,
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          `lock.status failed: ${getErrorMessage(error)}`,
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
          `Breaker Status: ${JSON.stringify(result)}`,
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          `Breaker status check failed: ${getErrorMessage(error)}`,
        );
      }
    },
  );

  const cacheClearCommand = vscode.commands.registerCommand(
    "go-on.cacheClear",
    async () => {
      try {
        await deps.sendRequest("cache.clear");
        vscode.window.showInformationMessage("Cache cleared.");
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          `Cache clear failed: ${getErrorMessage(error)}`,
        );
      }
    },
  );

  const vectorClearCommand = vscode.commands.registerCommand(
    "go-on.vectorClear",
    async () => {
      try {
        await deps.sendRequest("vector.clear");
        vscode.window.showInformationMessage("Vector memory cleared.");
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          `Vector clear failed: ${getErrorMessage(error)}`,
        );
      }
    },
  );

  const configReloadCommand = vscode.commands.registerCommand(
    "go-on.configReload",
    async () => {
      try {
        await deps.sendRequest("config.reload");
        vscode.window.showInformationMessage("Configuration reloaded.");
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          `Config reload failed: ${getErrorMessage(error)}`,
        );
      }
    },
  );

  const shutdownCommand = vscode.commands.registerCommand(
    "go-on.shutdown",
    async () => {
      try {
        await deps.sendRequest("shutdown");
        vscode.window.showInformationMessage("Shutdown initiated.");
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          `Shutdown failed: ${getErrorMessage(error)}`,
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
