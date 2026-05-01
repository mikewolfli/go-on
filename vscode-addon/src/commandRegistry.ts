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
        vscode.window.showWarningMessage(
          `Go-On executable is not ready: ${message}. Please set a valid .exe, .bat, or .sh path in Settings.`,
        );
        return;
      }

      const opened = await deps.revealGoOnView("chat");
      if (!opened) {
        vscode.window.showWarningMessage(
          "Go-On Chat view is not available yet. Reload Window after installing/updating the extension.",
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
        vscode.window.showInformationMessage(
          "Go-On chat closed. Running backend was stopped.",
        );
      } else {
        vscode.window.showInformationMessage(
          "Go-On chat closed. Backend was already stopped.",
        );
      }
    },
  );

  const openSettingsCommand = vscode.commands.registerCommand(
    "go-on.openSettings",
    async () => {
      const opened = await deps.revealGoOnView("settings");
      if (!opened) {
        vscode.window.showWarningMessage(
          "Go-On Settings view is not available yet. Reload Window after installing/updating the extension.",
        );
      }
    },
  );

  const createWorkflowCommand = vscode.commands.registerCommand(
    "go-on.createWorkflow",
    async () => {
      const opened = await deps.revealGoOnView("workflow");
      if (!opened) {
        vscode.window.showWarningMessage(
          "Go-On Workflow view is not available yet. Reload Window after installing/updating the extension.",
        );
      }
    },
  );

  const runWorkflowCommand = vscode.commands.registerCommand(
    "go-on.runWorkflow",
    () => {
      vscode.window.showInformationMessage(
        "Select a workflow to run from the Workflow panel",
      );
    },
  );

  const showProcessFlowCommand = vscode.commands.registerCommand(
    "go-on.showProcessFlow",
    async () => {
      const opened = await deps.revealGoOnView("process-flow");
      if (!opened) {
        vscode.window.showWarningMessage(
          "Go-On Process Flow view is not available yet. Reload Window after installing/updating the extension.",
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
        vscode.window.showErrorMessage(`Failed to create session: ${message}`);
      }
    },
  );

  const switchSessionCommand = vscode.commands.registerCommand(
    "go-on.switchSession",
    async () => {
      // Fetch sessions dynamically from backend via conversation.checkpoint.list RPC.
      let sessionNames: string[] = ["default"];
      try {
        const result = await deps.sendRequest("conversation.checkpoint.list", {
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
