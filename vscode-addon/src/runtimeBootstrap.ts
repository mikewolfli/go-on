import * as vscode from "vscode";
import { i18n, MessageKeys } from "./i18n";

export interface RuntimeBootstrapDeps {
  ensureBinary: (
    _workspaceRoot: string | undefined,
    _config: vscode.WorkspaceConfiguration,
    _context: vscode.ExtensionContext,
  ) => Promise<unknown>;
  isRunning: () => boolean;
  startCommandId: string;
}

export async function ensureRuntimeReadyAfterChatOpen(
  context: vscode.ExtensionContext,
  deps: RuntimeBootstrapDeps,
): Promise<void> {
  // Each call manages its own promise — no shared mutable state.
  // Previously a module-level `runtimeReadyPromise` was overwritten on each
  // call, causing call A to await call B's promise if B started before A
  // resolved. Bug 6: removed the shared promise.
  const promise = (async () => {
    const config = vscode.workspace.getConfiguration("go-on");
    const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;

    await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: i18n.getMessage(MessageKeys.runtimeDownloading),
        cancellable: false,
      },
      async () => {
        await deps.ensureBinary(workspaceRoot, config, context);
      },
    );

    if (config.get<boolean>("autoStart", false) && !deps.isRunning()) {
      await vscode.commands.executeCommand(deps.startCommandId);
    }
  })();

  await promise;
}

export async function ensureGoOnStarted(
  isRunning: () => boolean,
  startCommandId: string,
): Promise<void> {
  if (isRunning()) {
    return;
  }

  await vscode.commands.executeCommand(startCommandId);
  if (!isRunning()) {
    throw new Error(
      i18n.getMessage(MessageKeys.backendNotReady, [
        "still stopped after startup",
      ]),
    );
  }
}

export async function prepareRuntimeAndStartFromChat(
  context: vscode.ExtensionContext,
  deps: RuntimeBootstrapDeps,
): Promise<void> {
  try {
    await ensureRuntimeReadyAfterChatOpen(context, deps);
    await ensureGoOnStarted(deps.isRunning, deps.startCommandId);
  } catch (error: unknown) {
    const message = error instanceof Error ? error.message : String(error);
    // i18n
    vscode.window.showWarningMessage(
      i18n.getMessage(MessageKeys.backendNotReady, [message]),
    );
  }
}

export function parseMissingEnvVariableNames(errorMessage: string): string[] {
  const matches = errorMessage.match(
    /missing required environment variables[^:]*:\s*([^\n]+)/i,
  );
  if (!matches || matches.length < 2) {
    return [];
  }

  return matches[1]
    .split(",")
    .map((name) => name.trim())
    .filter((name) => /^[A-Z0-9_]+$/.test(name));
}

export function buildPlaceholderEnvValues(
  envNames: string[],
): Record<string, string> {
  const values: Record<string, string> = {};
  for (const envName of envNames) {
    values[envName] = "__GO_ON_PLACEHOLDER__";
  }
  return values;
}
