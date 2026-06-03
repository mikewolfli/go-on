import * as vscode from "vscode";

async function executeFirstAvailableCommand(
  commandIds: string[],
): Promise<boolean> {
  const availableCommands = await vscode.commands.getCommands(true);
  for (const commandId of commandIds) {
    if (!availableCommands.includes(commandId)) {
      continue;
    }

    await vscode.commands.executeCommand(commandId);
    return true;
  }

  return false;
}

export async function revealGoOnView(
  target: "chat" | "settings" | "workflow" | "process-flow",
): Promise<boolean> {
  const openedContainer = await executeFirstAvailableCommand([
    "workbench.view.extension.go-on",
    "workbench.view.extension.go_on",
    "workbench.view.extension.goon",
  ]);

  // Only hyphen variant command IDs are registered in package.json,
  // so underscore variants are omitted here.
  const focusCommands: Record<typeof target, string[]> = {
    chat: ["go-on-chat.focus"],
    settings: ["go-on-settings.focus"],
    workflow: ["go-on-workflow.focus"],
    "process-flow": ["go-on-process-flow.focus"],
  };

  const focused = await executeFirstAvailableCommand(focusCommands[target]);

  if (openedContainer || focused) {
    return true;
  }

  const viewIds: Record<typeof target, string> = {
    chat: "go-on-chat",
    settings: "go-on-settings",
    workflow: "go-on-workflow",
    "process-flow": "go-on-process-flow",
  };

  try {
    await vscode.commands.executeCommand(
      "workbench.action.openView",
      viewIds[target],
    );
    return true;
  } catch {
    return false;
  }
}
