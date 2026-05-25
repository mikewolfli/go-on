import * as vscode from "vscode";
import { i18n, MessageKeys } from "./i18n";

export interface ViewCommandRegistryDeps {
  revealGoOnView: (
    _target: "chat" | "settings" | "workflow" | "process-flow",
  ) => Promise<boolean>;
  ensureBinaryReady: () => Promise<void>;
  prepareRuntimeAfterChatOpen: () => Promise<void>;
  isRunning: () => boolean;
  stop: () => void;
  createSession: (_sessionName: string) => void;
  switchSession: (_sessionName: string) => void;
  clearChat: () => void;
  exportChat: () => void;
  sendRequest: (_method: string, _params?: unknown) => Promise<unknown>;
}

export function registerViewCommands(
  deps: ViewCommandRegistryDeps,
): vscode.Disposable[] {
  const openChatCommand = vscode.commands.registerCommand(
    "go-on.openChat",
    async () => {
      try {
        await deps.ensureBinaryReady();
      } catch (error: unknown) {
        await deps.revealGoOnView("settings");
        const message = error instanceof Error ? error.message : String(error);
        // i18n
        vscode.window.showWarningMessage(
          i18n.getMessage(MessageKeys.executableNotReady, [message]),
        );
        return;
      }

      const opened = await deps.revealGoOnView("chat");
      if (!opened) {
        // i18n
        vscode.window.showWarningMessage(
          i18n.getMessage(MessageKeys.chatViewNotAvailable),
        );
      }
      void deps.prepareRuntimeAfterChatOpen();
    },
  );

  const closeChatCommand = vscode.commands.registerCommand(
    "go-on.closeChat",
    async () => {
      await vscode.commands.executeCommand("workbench.view.explorer");

      if (deps.isRunning()) {
        deps.stop();
        // i18n
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.chatClosedBackendStopped),
        );
      } else {
        // i18n
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.chatClosedBackendAlreadyStopped),
        );
      }
    },
  );

  const openSettingsCommand = vscode.commands.registerCommand(
    "go-on.openSettings",
    async () => {
      const opened = await deps.revealGoOnView("settings");
      if (!opened) {
        // i18n
        vscode.window.showWarningMessage(
          i18n.getMessage(MessageKeys.settingsViewNotAvailable),
        );
      }
    },
  );

  const createWorkflowCommand = vscode.commands.registerCommand(
    "go-on.createWorkflow",
    async () => {
      const opened = await deps.revealGoOnView("workflow");
      if (!opened) {
        // i18n
        vscode.window.showWarningMessage(
          i18n.getMessage(MessageKeys.workflowViewNotAvailable),
        );
      }
    },
  );

  const runWorkflowCommand = vscode.commands.registerCommand(
    "go-on.runWorkflow",
    async () => {
      if (!deps.isRunning()) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.goOnNotRunningRpc),
        );
        return;
      }

      const objective = await vscode.window.showInputBox({
        prompt: i18n.getMessage(MessageKeys.promptWorkflowObjective),
        placeHolder: i18n.getMessage(
          MessageKeys.promptWorkflowObjectivePlaceholder,
        ),
      });
      if (!objective) {
        return;
      }

      try {
        const result = await deps.sendRequest("workflow.execute", {
          task: objective,
        });
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandCompleted, [
            "workflow.execute",
            JSON.stringify(result),
          ]),
        );
      } catch (error: unknown) {
        const message = error instanceof Error ? error.message : String(error);
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "workflow.execute",
            message,
          ]),
        );
      }
    },
  );

  const showProcessFlowCommand = vscode.commands.registerCommand(
    "go-on.showProcessFlow",
    async () => {
      const opened = await deps.revealGoOnView("process-flow");
      if (!opened) {
        // i18n
        vscode.window.showWarningMessage(
          i18n.getMessage(MessageKeys.processFlowViewNotAvailable),
        );
      }
    },
  );

  const clearChatCommand = vscode.commands.registerCommand(
    "go-on.clearChat",
    () => {
      deps.clearChat();
    },
  );

  const exportChatCommand = vscode.commands.registerCommand(
    "go-on.exportChat",
    () => {
      deps.exportChat();
    },
  );

  const newSessionCommand = vscode.commands.registerCommand(
    "go-on.newSession",
    async () => {
      try {
        const sessionName = await vscode.window.showInputBox({
          prompt: i18n.getMessage(MessageKeys.newSession),
          placeHolder: i18n.getMessage(MessageKeys.sessionNamePlaceholder),
        });
        if (sessionName) {
          deps.createSession(sessionName);
        }
      } catch (error: unknown) {
        const message = error instanceof Error ? error.message : String(error);
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.createSessionFailed, [message]),
        );
      }
    },
  );

  const switchSessionCommand = vscode.commands.registerCommand(
    "go-on.switchSession",
    async () => {
      // Fetch sessions dynamically from backend via checkpoint.list RPC.
      let sessionNames: string[] = ["default"];
      try {
        const result = await deps.sendRequest("checkpoint.list", {
          conversation_id: "default",
        });
        if (result && typeof result === "object" && "checkpoints" in result) {
          const checkpoints = (result as Record<string, unknown>).checkpoints;
          if (Array.isArray(checkpoints)) {
            const names = new Set<string>(["default"]);
            for (const cp of checkpoints) {
              if (cp && typeof cp === "object") {
                const cid = (cp as Record<string, unknown>).conversation_id;
                if (typeof cid === "string" && cid.length > 0) {
                  names.add(cid);
                }
              }
            }
            sessionNames = Array.from(names);
          }
        }
      } catch {
        // RPC failed — fall back to ["default"]
        // eslint-disable-next-line no-console
        console.warn(
          "go-on: failed to fetch session list from backend, using default",
        );
      }

      const session = await vscode.window.showQuickPick(sessionNames, {
        placeHolder: i18n.getMessage(MessageKeys.selectSession),
      });
      if (session) {
        deps.switchSession(session);
      }
    },
  );

  return [
    openChatCommand,
    closeChatCommand,
    openSettingsCommand,
    clearChatCommand,
    exportChatCommand,
    newSessionCommand,
    switchSessionCommand,
    createWorkflowCommand,
    runWorkflowCommand,
    showProcessFlowCommand,
  ];
}
