import * as vscode from "vscode";
import { i18n, MessageKeys } from "./i18n";
import { RuntimeManagerLike } from "./managerTypes";

interface AdvancedEditArgs {
  action?: string;
  details?: string;
}

export class GoOnAdvancedEditProvider {
  private readonly manager: RuntimeManagerLike;
  private readonly context: vscode.ExtensionContext;

  constructor(_manager: RuntimeManagerLike, _context: vscode.ExtensionContext) {
    this.manager = _manager;
    this.context = _context;

    // Register code actions
    this.registerCodeActions();

    // Register commands
    this.registerCommands();
  }

  private registerCodeActions() {
    // Provide code actions for refactoring
    this.context.subscriptions.push(
      vscode.languages.registerCodeActionsProvider(
        [
          "javascript",
          "typescript",
          "python",
          "java",
          "cpp",
          "c",
          "go",
          "rust",
        ],
        {
          provideCodeActions: (document, range, _context, _token) => {
            const actions: vscode.CodeAction[] = [];

            // Add Go-On specific code actions
            if (!range.isEmpty) {
              actions.push(this.createExplainCodeAction(document, range));
              actions.push(this.createRefactorCodeAction(document, range));
              actions.push(this.createOptimizeCodeAction(document, range));
            }

            return actions;
          },
        },
      ),
    );
  }

  private extractResponseText(result: unknown): string | undefined {
    if (!result || typeof result !== "object") {
      return undefined;
    }
    const candidate = (result as { response?: unknown }).response;
    return typeof candidate === "string" ? candidate : undefined;
  }

  private getErrorMessage(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
  }

  private registerCommands() {
    // Register advanced editing commands
    this.context.subscriptions.push(
      vscode.commands.registerCommand("go-on.editCode", async () => {
        await this.handleAdvancedEdit();
      }),

      vscode.commands.registerCommand("go-on.refactorCode", async () => {
        await this.handleRefactorCode();
      }),
    );
  }

  private createExplainCodeAction(
    document: vscode.TextDocument,
    range: vscode.Range,
  ): vscode.CodeAction {
    const action = new vscode.CodeAction(
      `Go-On: ${i18n.getMessage(MessageKeys.editingActionExplainCode)}`,
      vscode.CodeActionKind.QuickFix,
    );
    action.command = {
      command: "go-on.editCode",
      title: i18n.getMessage(MessageKeys.editingActionExplainCode),
      arguments: [{ action: "explain", document, range }],
    };
    return action;
  }

  private createRefactorCodeAction(
    document: vscode.TextDocument,
    range: vscode.Range,
  ): vscode.CodeAction {
    const action = new vscode.CodeAction(
      `Go-On: ${i18n.getMessage(MessageKeys.editingActionRefactorCode)}`,
      vscode.CodeActionKind.Refactor,
    );
    action.command = {
      command: "go-on.refactorCode",
      title: i18n.getMessage(MessageKeys.editingActionRefactorCode),
      arguments: [{ action: "refactor", document, range }],
    };
    return action;
  }

  private createOptimizeCodeAction(
    document: vscode.TextDocument,
    range: vscode.Range,
  ): vscode.CodeAction {
    const action = new vscode.CodeAction(
      `Go-On: ${i18n.getMessage(MessageKeys.editingActionOptimizeCode)}`,
      vscode.CodeActionKind.Refactor,
    );
    action.command = {
      command: "go-on.editCode",
      title: i18n.getMessage(MessageKeys.editingActionOptimizeCode),
      arguments: [{ action: "optimize", document, range }],
    };
    return action;
  }

  private async handleAdvancedEdit(args?: AdvancedEditArgs) {
    const editor = vscode.window.activeTextEditor;
    if (!editor) {
      vscode.window.showErrorMessage(
        i18n.getMessage(MessageKeys.editingNoActiveEditor),
      );
      return;
    }

    const document = editor.document;
    const selection = editor.selection;
    const selectedText = document.getText(selection);

    if (!selectedText) {
      vscode.window.showInformationMessage(
        i18n.getMessage(MessageKeys.editingSelectCodeToEdit),
      );
      return;
    }

    let action = args?.action;
    if (!action) {
      const picked = await vscode.window.showQuickPick(
        [
          {
            label: i18n.getMessage(MessageKeys.editingActionExplainCode),
            value: "explain",
          },
          {
            label: i18n.getMessage(MessageKeys.editingActionRefactorCode),
            value: "refactor",
          },
          {
            label: i18n.getMessage(MessageKeys.editingActionOptimizeCode),
            value: "optimize",
          },
          {
            label: i18n.getMessage(MessageKeys.editingActionAddComments),
            value: "comment",
          },
          {
            label: i18n.getMessage(MessageKeys.editingActionConvertToAsync),
            value: "async",
          },
          {
            label: i18n.getMessage(MessageKeys.editingActionAddErrorHandling),
            value: "error-handling",
          },
          {
            label: i18n.getMessage(MessageKeys.editingActionGenerateUnitTests),
            value: "test",
          },
          {
            label: i18n.getMessage(MessageKeys.editingActionSecurityAudit),
            value: "security-audit",
          },
        ],
        {
          placeHolder: i18n.getMessage(
            MessageKeys.editingChooseActionPlaceholder,
          ),
        },
      );
      if (!picked) return;
      action = picked.value;
    }

    if (!this.manager.isRunning()) {
      await vscode.window.showWarningMessage(
        i18n.getMessage(MessageKeys.goOnNotRunningRpc),
      );
      return;
    }

    try {
      const prompt = this.buildPrompt(
        action,
        selectedText,
        document.languageId,
        args?.details,
      );
      const result = await this.manager.sendRequest("chat", {
        messages: [{ role: "user", content: prompt }],
      });
      const responseText = this.extractResponseText(result);

      if (!responseText) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.editingNoResponseFromAi),
        );
        return;
      }

      await this.handleResultOutput(
        responseText,
        editor,
        selection,
        document.languageId,
      );
    } catch (error: unknown) {
      vscode.window.showErrorMessage(
        i18n.getMessage(MessageKeys.changesFailed, [
          this.getErrorMessage(error),
        ]),
      );
    }
  }

  private async handleResultOutput(
    resultText: string,
    editor: vscode.TextEditor,
    selection: vscode.Range,
    language: string,
  ) {
    const config = vscode.workspace.getConfiguration("go-on");
    const showDiffByDefault = config.get<boolean>(
      "advancedAI.showDiffByDefault",
      true,
    );

    let showResult: { label: string; value: string } | undefined;
    if (showDiffByDefault) {
      showResult = {
        label: i18n.getMessage(MessageKeys.showDiff),
        value: "diff",
      };
    } else {
      showResult = await vscode.window.showQuickPick(
        [
          {
            label: i18n.getMessage(MessageKeys.editingResultReplaceSelection),
            value: "replace",
          },
          {
            label: i18n.getMessage(MessageKeys.editingResultShowInNewDocument),
            value: "new-doc",
          },
          { label: i18n.getMessage(MessageKeys.showDiff), value: "diff" },
          {
            label: i18n.getMessage(MessageKeys.editingResultCopyToClipboard),
            value: "clipboard",
          },
        ],
        {
          placeHolder: i18n.getMessage(
            MessageKeys.editingChooseResultDisplayPlaceholder,
          ),
        },
      );
    }

    if (!showResult) return;

    switch (showResult.value) {
      case "replace":
        await editor.edit((editBuilder) => {
          editBuilder.replace(selection, resultText);
        });
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.editingResultApplied),
        );
        break;
      case "new-doc": {
        const doc = await vscode.workspace.openTextDocument({
          content: resultText,
          language,
        });
        await vscode.window.showTextDocument(doc, { preview: false });
        break;
      }
      case "diff": {
        const originalDoc = await vscode.workspace.openTextDocument({
          content: editor.document.getText(selection),
          language,
        });
        const refactoredDoc = await vscode.workspace.openTextDocument({
          content: resultText,
          language,
        });
        await vscode.commands.executeCommand(
          "vscode.diff",
          originalDoc.uri,
          refactoredDoc.uri,
          i18n.getMessage(MessageKeys.editingOriginalRefactoredDiffTitle),
        );
        break;
      }
      case "clipboard":
        await vscode.env.clipboard.writeText(resultText);
        vscode.window.showInformationMessage(
          i18n.getMessage(MessageKeys.codeCopied),
        );
        break;
    }
  }

  private async handleRefactorCode(args?: AdvancedEditArgs) {
    const editor = vscode.window.activeTextEditor;
    if (!editor) {
      vscode.window.showErrorMessage(
        i18n.getMessage(MessageKeys.editingNoActiveEditor),
      );
      return;
    }

    const document = editor.document;
    const selection = editor.selection;
    const selectedText = document.getText(selection);

    if (!selectedText) {
      vscode.window.showInformationMessage(
        i18n.getMessage(MessageKeys.editingSelectCodeToRefactor),
      );
      return;
    }

    let refactorType = args?.action;
    if (!refactorType) {
      const picked = await vscode.window.showQuickPick(
        [
          {
            label: i18n.getMessage(MessageKeys.editingRefactorExtractFunction),
            value: "extract-function",
          },
          {
            label: i18n.getMessage(MessageKeys.editingRefactorRenameVariables),
            value: "rename-variables",
          },
          {
            label: i18n.getMessage(MessageKeys.editingRefactorSimplifyLogic),
            value: "simplify-logic",
          },
          {
            label: i18n.getMessage(
              MessageKeys.editingRefactorImprovePerformance,
            ),
            value: "performance",
          },
          {
            label: i18n.getMessage(MessageKeys.editingRefactorAddTypeHints),
            value: "type-hints",
          },
          {
            label: i18n.getMessage(MessageKeys.editingRefactorCustom),
            value: "custom",
          },
        ],
        {
          placeHolder: i18n.getMessage(
            MessageKeys.editingChooseRefactorTypePlaceholder,
          ),
        },
      );
      if (!picked) return;
      refactorType = picked.value;
    }

    let prompt: string;
    if (refactorType === "custom") {
      const customPrompt = await vscode.window.showInputBox({
        prompt: i18n.getMessage(MessageKeys.editingDescribeRefactorPrompt),
        placeHolder: i18n.getMessage(
          MessageKeys.editingDescribeRefactorPlaceholder,
        ),
      });
      if (!customPrompt) return;
      prompt = `Please refactor this code to ${customPrompt}:\n\n${selectedText}`;
    } else {
      prompt = this.buildRefactorPrompt(
        refactorType,
        selectedText,
        document.languageId,
      );
    }

    if (!this.manager.isRunning()) {
      await vscode.window.showWarningMessage(
        i18n.getMessage(MessageKeys.goOnNotRunningRpc),
      );
      return;
    }

    try {
      const result = await this.manager.sendRequest("chat", {
        messages: [{ role: "user", content: prompt }],
      });
      const responseText = this.extractResponseText(result);

      if (!responseText) {
        vscode.window.showErrorMessage(
          i18n.getMessage(MessageKeys.editingNoResponseFromAi),
        );
        return;
      }

      await this.handleResultOutput(
        responseText,
        editor,
        selection,
        document.languageId,
      );
    } catch (error: unknown) {
      vscode.window.showErrorMessage(
        i18n.getMessage(MessageKeys.changesFailed, [
          this.getErrorMessage(error),
        ]),
      );
    }
  }

  private buildPrompt(
    action: string,
    code: string,
    language: string,
    details?: string,
  ): string {
    const config = vscode.workspace.getConfiguration("go-on");
    const advancedContext = config.get<string>(
      "advancedAI.context",
      "You are a senior engineer assistant with expertise across languages, security, testing, and performance.",
    );
    const modelHint = config.get<string>("chat.model", "auto");
    const temperature = config.get<number>("chat.temperature", 0.7);

    const base = `${advancedContext} Use ${modelHint} model settings (temperature ${temperature}). Preserve behavior while improving code quality and readability.`;
    const detail = `\n\nInput Language: ${language}\nInput Code:\n${code}\n${details ? `Additional Instructions: ${details}\n` : ""}`;

    const prompts: { [key: string]: string } = {
      explain: `${base}\n\nTask: Explain what this code does, including intent, edge cases, and potential pitfalls.${detail}`,
      refactor: `${base}\n\nTask: Refactor this code to be clean, modular, and maintainable. Keep logic equivalent.${detail}`,
      optimize: `${base}\n\nTask: Optimize this code for performance and readability. Mention tradeoffs.${detail}`,
      comment: `${base}\n\nTask: Add comprehensive inline comments and a top-level summary.${detail}`,
      async: `${base}\n\nTask: Convert this code to async/await style (or equivalent async pattern) with error handling.${detail}`,
      "error-handling": `${base}\n\nTask: Add robust error handling, validation, and edge case handling.${detail}`,
      test: `${base}\n\nTask: Generate unit tests for this code snippet with assertions and edge cases.${detail}`,
      "security-audit": `${base}\n\nTask: Perform a security audit of this code and suggest fixes for vulnerabilities.${detail}`,
    };

    return prompts[action] || `${base}\n\nTask: ${action} this code.${detail}`;
  }

  private buildRefactorPrompt(
    refactorType: string,
    code: string,
    language: string,
  ): string {
    const prompts: { [key: string]: string } = {
      "extract-function": `Please extract functions from this ${language} code to improve readability:\n\n${code}`,
      "rename-variables": `Please rename variables in this ${language} code to be more descriptive:\n\n${code}`,
      "simplify-logic": `Please simplify the logic in this ${language} code:\n\n${code}`,
      performance: `Please optimize this ${language} code for better performance:\n\n${code}`,
      "type-hints": `Please add type hints to this ${language} code:\n\n${code}`,
    };

    return (
      prompts[refactorType] ||
      `Please refactor this ${language} code:\n\n${code}`
    );
  }
}
