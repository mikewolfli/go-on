import * as vscode from "vscode";
import { i18n, MessageKeys } from "../i18n";
import { asArray, getErrorMessage } from "../rpcCommandRegistry";
import { RpcCommandRegistryDeps } from "../rpcCommandRegistry";

/**
 * Register agent-related RPC commands: skill import/list/toggle.
 */
export function registerAgentRpcCommands(
  deps: RpcCommandRegistryDeps,
): vscode.Disposable[] {
  const skillListImportedRpcCommand = vscode.commands.registerCommand(
    "go-on.skillListImported",
    async () => {
      try {
        const result = (await deps.sendRequest(
          "skill.list_imported",
        )) as Record<string, unknown>;
        const skills = asArray(result.skills);
        const enabled = skills.filter(
          (s: unknown) => (s as Record<string, unknown>).enabled === true,
        ).length;
        const total = skills.length;
        const disabled = total - enabled;
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "skill.list_imported",
            `${enabled} enabled, ${disabled} disabled (${total} total)`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "skill.list_imported",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const skillImportLocalRpcCommand = vscode.commands.registerCommand(
    "go-on.skillImportLocal",
    async () => {
      try {
        const manifestPath = await vscode.window.showInputBox({
          prompt: "Enter skill manifest path",
          placeHolder: "e.g. /path/to/skill.json or ./my-skill/",
        });
        if (!manifestPath) return;
        const sha256 = await vscode.window.showInputBox({
          prompt: "Optional SHA-256 hash",
          placeHolder: "SHA-256 hash (optional)",
        });
        const result = (await deps.sendRequest("skill.import_local", {
          source: {
            kind: "local",
            path: manifestPath,
            sha256: sha256 || undefined,
          },
        })) as Record<string, unknown>;
        const skill = result.skill as Record<string, unknown> | undefined;
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            "skill.import_local",
            skill?.name
              ? `imported "${String(skill.name)}"`
              : "import succeeded",
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "skill.import_local",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  const skillToggleRpcCommand = vscode.commands.registerCommand(
    "go-on.skillToggle",
    async () => {
      try {
        const name = await vscode.window.showInputBox({
          prompt: "Enter skill name",
          placeHolder: "skill-name",
        });
        if (!name) return;
        const action = await vscode.window.showQuickPick(
          ["enable", "disable", "remove"],
          { placeHolder: "Select action" },
        );
        if (!action) return;
        const method =
          action === "enable"
            ? "skill.enable"
            : action === "disable"
              ? "skill.disable"
              : "skill.remove";
        const result = (await deps.sendRequest(method, {
          name,
        })) as Record<string, unknown>;
        const removed = result.removed as boolean | undefined;
        const skill = result.skill as Record<string, unknown> | undefined;
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.rpcCommandResult, [
            method,
            removed
              ? "removed"
              : skill?.name
                ? `"${String(skill.name)}" ${action}d`
                : `${action}d`,
          ]),
        );
      } catch (error: unknown) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.rpcCommandFailed, [
            "skill.toggle",
            getErrorMessage(error),
          ]),
        );
      }
    },
  );

  return [
    skillListImportedRpcCommand,
    skillImportLocalRpcCommand,
    skillToggleRpcCommand,
  ];
}
