import * as vscode from "vscode";

export interface RuntimeBootstrapDeps {
  ensureBinary: (
    _workspaceRoot: string | undefined,
    _config: vscode.WorkspaceConfiguration,
    _context: vscode.ExtensionContext,
  ) => Promise<unknown>;
  isRunning: () => boolean;
  startCommandId: string;
}

let runtimeReadyPromise: Promise<void> | null = null;

export async function ensureRuntimeReadyAfterChatOpen(
  context: vscode.ExtensionContext,
  deps: RuntimeBootstrapDeps,
): Promise<void> {
  if (runtimeReadyPromise) {
    await runtimeReadyPromise;
    return;
  }

  runtimeReadyPromise = (async () => {
    const config = vscode.workspace.getConfiguration("go-on");
    const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;

    await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: "Preparing Go-On runtime",
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

  try {
    await runtimeReadyPromise;
  } catch (error) {
    runtimeReadyPromise = null;
    throw error;
  }
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
      "Go-On backend is still stopped after startup attempt. Check executablePath/configPath settings.",
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
    // TODO: i18n - hardcoded English string
    vscode.window.showWarningMessage(
      `Chat is open. Backend is not ready yet: ${message}. Configure Go-On settings in the Chat/Settings view and retry.`,
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
