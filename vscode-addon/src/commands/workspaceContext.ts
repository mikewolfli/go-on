//! Workspace Context Commands — native ACP/MCP integration for VS Code.
//!
//! Provides IDE-native commands that bridge the Go-On backend with VS Code's
//! workspace, editor, and language features:
//!
//! - **`go-on.sendSelection`** — Send selected code as context to the Go-On chat.
//! - **`go-on.sendFile`** — Send the current file as workspace context.
//! - **`go-on.semanticSearch`** — Search workspace symbols via the code index.
//! - **`go-on.workspaceContext`** — Send full workspace context (open files, project structure).

import * as vscode from "vscode";
import { i18n, MessageKeys } from "../i18n";

interface WorkspaceContextDeps {
  isRunning: () => boolean;
  sendRequest: (method: string, params?: unknown) => Promise<unknown>;
}

/**
 * Collect selected text and metadata from the active editor.
 */
function getSelectionContext(): {
  text: string;
  fileName: string;
  language: string;
  selectionRange: string;
} | null {
  const editor = vscode.window.activeTextEditor;
  if (!editor) return null;

  const selection = editor.selection;
  const text = editor.document.getText(selection);
  if (!text || text.trim().length === 0) return null;

  return {
    text,
    fileName: editor.document.fileName,
    language: editor.document.languageId,
    selectionRange: `${selection.start.line + 1}:${selection.start.character + 1}-${selection.end.line + 1}:${selection.end.character + 1}`,
  };
}

/**
 * Collect workspace-level context: open files, project root, language stats.
 */
async function getWorkspaceContext(): Promise<Record<string, unknown>> {
  const workspaceFolders = vscode.workspace.workspaceFolders;
  if (!workspaceFolders || workspaceFolders.length === 0) {
    return { error: "No workspace folder open" };
  }

  const rootPath = workspaceFolders[0].uri.fsPath;

  // Collect open text documents
  const openFiles: Array<{
    fileName: string;
    language: string;
    lineCount: number;
  }> = [];
  for (const doc of vscode.workspace.textDocuments) {
    if (!doc.isUntitled && doc.uri.scheme === "file") {
      openFiles.push({
        fileName: doc.fileName,
        language: doc.languageId,
        lineCount: doc.lineCount,
      });
    }
  }

  return {
    rootPath,
    openFiles,
    workspaceFolders: workspaceFolders.map((wf) => ({
      name: wf.name,
      uri: wf.uri.fsPath,
    })),
  };
}

/**
 * Ensure the Go-On runtime is running; show error if not.
 */
async function ensureRunning(deps: WorkspaceContextDeps): Promise<boolean> {
  if (!deps.isRunning()) {
    await vscode.window.showErrorMessage(
      i18n.getMessage(MessageKeys.workspaceNotRunning),
    );
    return false;
  }
  return true;
}

/**
 * Register all workspace context commands.
 */
export function registerWorkspaceContextCommands(
  _context: vscode.ExtensionContext,
  deps: WorkspaceContextDeps,
): vscode.Disposable[] {
  const disposables: vscode.Disposable[] = [];

  // ── go-on.sendSelection ──────────────────────────────────────────────
  disposables.push(
    vscode.commands.registerCommand("go-on.sendSelection", async () => {
      if (!(await ensureRunning(deps))) return;
      const ctx = getSelectionContext();
      if (!ctx) {
        vscode.window.showWarningMessage(
          i18n.getMessage(MessageKeys.workspaceNoCodeSelected),
        );
        return;
      }
      await deps.sendRequest("chat", {
        messages: [
          {
            role: "user",
            content: `[Code Context - ${ctx.fileName} (${ctx.language}) at ${ctx.selectionRange}]\n\n\`\`\`${ctx.language}\n${ctx.text}\n\`\`\``,
          },
        ],
      });
      vscode.window.showInformationMessage(
        i18n.getMessage(
          MessageKeys.workspaceSentSelection,
          ctx.text.split("\n").length.toString(),
          ctx.language,
        ),
      );
    }),
  );

  // ── go-on.sendFile ──────────────────────────────────────────────────
  disposables.push(
    vscode.commands.registerCommand("go-on.sendFile", async () => {
      if (!(await ensureRunning(deps))) return;
      const editor = vscode.window.activeTextEditor;
      if (!editor) {
        vscode.window.showWarningMessage(
          i18n.getMessage(MessageKeys.workspaceNoActiveEditor),
        );
        return;
      }
      const text = editor.document.getText();
      const fileName = editor.document.fileName;
      const language = editor.document.languageId;
      await deps.sendRequest("chat", {
        messages: [
          {
            role: "user",
            content: `[Full File Context - ${fileName} (${language}, ${editor.document.lineCount} lines)]\n\n\`\`\`${language}\n${text}\n\`\`\``,
          },
        ],
      });
      vscode.window.showInformationMessage(
        i18n.getMessage(
          MessageKeys.workspaceSentFile,
          fileName,
          editor.document.lineCount.toString(),
        ),
      );
    }),
  );

  // ── go-on.semanticSearch ────────────────────────────────────────────
  disposables.push(
    vscode.commands.registerCommand("go-on.semanticSearch", async () => {
      if (!(await ensureRunning(deps))) return;
      const query = await vscode.window.showInputBox({
        prompt: i18n.getMessage(MessageKeys.workspaceSearchPrompt),
        placeHolder: i18n.getMessage(MessageKeys.workspaceSearchPlaceholder),
        ignoreFocusOut: true,
      });
      if (!query || query.trim().length === 0) return;

      // First ensure index is built
      const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
      if (!workspaceRoot) {
        vscode.window.showWarningMessage(
          i18n.getMessage(MessageKeys.workspaceNoWorkspaceFolder),
        );
        return;
      }

      await deps.sendRequest("tool", {
        name: "code_index_search",
        arguments: {
          operation: "build",
          directory: workspaceRoot,
        },
      });

      // Then search
      const result = await deps.sendRequest("tool", {
        name: "code_index_search",
        arguments: {
          operation: "search",
          query,
          limit: 30,
        },
      });

      const resultStr =
        typeof result === "string" ? result : JSON.stringify(result, null, 2);
      const doc = await vscode.workspace.openTextDocument({
        content: resultStr,
        language: "json",
      });
      await vscode.window.showTextDocument(doc, {
        preview: true,
        preserveFocus: false,
      });

      vscode.window.showInformationMessage(
        i18n.getMessage(MessageKeys.workspaceSearchComplete, query ?? ""),
      );
    }),
  );

  // ── go-on.workspaceContext ──────────────────────────────────────────
  disposables.push(
    vscode.commands.registerCommand("go-on.workspaceContext", async () => {
      if (!(await ensureRunning(deps))) return;
      const ctx = await getWorkspaceContext();
      await deps.sendRequest("chat", {
        messages: [
          {
            role: "user",
            content: `[Workspace Context]\n\n\`\`\`json\n${JSON.stringify(ctx, null, 2)}\n\`\`\``,
          },
        ],
      });
      vscode.window.showInformationMessage(
        i18n.getMessage(MessageKeys.workspaceContextSent),
      );
    }),
  );

  // ── Inline code action: Explain Code ──────────────────────────────
  disposables.push(
    vscode.languages.registerCodeActionsProvider(
      { scheme: "file", pattern: "**/*" },
      {
        provideCodeActions(
          document: vscode.TextDocument,
          range: vscode.Range,
        ): vscode.CodeAction[] {
          const editor = vscode.window.activeTextEditor;
          if (
            !editor ||
            editor.document.uri.toString() !== document.uri.toString()
          )
            return [];
          const text = document.getText(range);
          if (!text || text.trim().length === 0) return [];

          const explain = new vscode.CodeAction(
            i18n.getMessage(MessageKeys.workspaceExplainAction),
            vscode.CodeActionKind.QuickFix,
          );
          explain.command = {
            command: "go-on.sendSelection",
            title: i18n.getMessage(MessageKeys.workspaceExplainAction),
            arguments: [],
          };
          explain.diagnostics = [];
          return [explain];
        },
      },
      { providedCodeActionKinds: [vscode.CodeActionKind.QuickFix] },
    ),
  );

  return disposables;
}
