import * as vscode from "vscode";
import { Logger } from "./logger";
import { RuntimeManagerLike } from "./managerTypes";
import { t, MessageKeys } from "./i18n";
import { getErrorMessage, getNonce } from "./utils";

const log = Logger.forModule("workflowView");

interface WorkflowStep {
  type: "chat" | "code" | "delay";
  prompt?: string;
  delay?: number;
}

interface WorkflowData {
  name: string;
  steps: WorkflowStep[];
  id?: string;
  created?: string;
  status?: "created" | "running" | "completed" | "failed";
}

type WorkflowStore = Record<string, WorkflowData>;

export class GoOnWorkflowViewProvider implements vscode.WebviewViewProvider {
  public static readonly viewType = "go-on-workflow";
  private _view?: vscode.WebviewView;
  private _messageSubscription?: vscode.Disposable;
  private readonly manager: RuntimeManagerLike;
  private readonly context: vscode.ExtensionContext;

  constructor(
    private readonly _extensionUri: vscode.Uri,
    _manager: RuntimeManagerLike,
    _context: vscode.ExtensionContext,
  ) {
    this.manager = _manager;
    this.context = _context;
  }

  public resolveWebviewView(
    webviewView: vscode.WebviewView,
    _context: vscode.WebviewViewResolveContext,
    _token: vscode.CancellationToken,
  ) {
    this._view = webviewView;

    webviewView.webview.options = {
      enableScripts: true,
      localResourceRoots: [this._extensionUri],
    };

    webviewView.webview.html = this._getHtmlForWebview(webviewView.webview);

    this._messageSubscription?.dispose();
    this._messageSubscription = webviewView.webview.onDidReceiveMessage(
      async (message) => {
        try {
          switch (message.type) {
            case "createWorkflow":
              await this._createWorkflow(message.workflowData);
              break;
            case "runWorkflow":
              await this._runWorkflow(message.workflowId);
              break;
            case "deleteWorkflow":
              try {
                await this._deleteWorkflow(message.workflowId);
              } catch (err) {
                log.warn("delete error:", err);
              }
              break;
            case "showConfirm":
              try {
                const selection = await vscode.window.showWarningMessage(
                  message.message,
                  { modal: true },
                  t(MessageKeys.ok),
                  t(MessageKeys.cancel),
                );
                this._view?.webview.postMessage({
                  type: "showConfirmResult",
                  id: message.id,
                  confirmed: selection === t(MessageKeys.ok),
                  workflowId: message.workflowId,
                });
              } catch (err) {
                log.warn("showConfirm error:", err);
              }
              break;
            case "showInputBox":
              {
                const result = await vscode.window.showInputBox({
                  prompt: message.prompt,
                  value: message.value,
                });
                this._view?.webview.postMessage({
                  type: "showInputBoxResult",
                  id: message.id,
                  value: result,
                });
              }
              break;
            case "showQuickPick":
              {
                const items: vscode.QuickPickItem[] = (message.items || []).map(
                  (item: { label: string; value: string }) => ({
                    label: item.label,
                    description: item.value,
                  }),
                );
                const picked = await vscode.window.showQuickPick(items, {
                  placeHolder: message.placeHolder,
                });
                this._view?.webview.postMessage({
                  type: "showQuickPickResult",
                  id: message.id,
                  value: picked?.description || null,
                });
              }
              break;
          }
        } catch (error: unknown) {
          const message_text =
            error instanceof Error ? error.message : String(error);
          void vscode.window.showErrorMessage(
            t(MessageKeys.workflowError, message_text),
          );
        }
      },
      undefined,
    );
  }

  private async _createWorkflow(workflowData: WorkflowData): Promise<void> {
    const workflows = this.context.workspaceState.get<WorkflowStore>(
      "go-on-workflows",
      {},
    );
    const workflowId = `workflow_${Date.now()}`;
    workflows[workflowId] = {
      ...workflowData,
      id: workflowId,
      created: new Date().toISOString(),
      status: "created",
    };
    await this.context.workspaceState.update("go-on-workflows", workflows);

    this._view?.webview.postMessage({
      type: "workflowCreated",
      workflow: workflows[workflowId],
    });

    vscode.window.showInformationMessage(
      t(MessageKeys.workflowCreatedSuccess, workflowData.name),
    );
  }

  private async _runWorkflow(workflowId: string): Promise<void> {
    const workflows = this.context.workspaceState.get<WorkflowStore>(
      "go-on-workflows",
      {},
    );
    const workflow = workflows[workflowId];

    if (!workflow) {
      vscode.window.showErrorMessage(t(MessageKeys.workflowNotFound));
      return;
    }

    // Update status
    workflow.status = "running";
    await this.context.workspaceState.update("go-on-workflows", workflows);

    this._view?.webview.postMessage({
      type: "workflowStatusUpdate",
      workflowId,
      status: "running",
    });

    try {
      // B51-24: Delegate workflow execution to backend via RPC
      await this.manager.sendRequest("workflow.execute", {
        workflowId,
        name: workflow.name,
        steps: workflow.steps,
      });

      workflow.status = "completed";
      await this.context.workspaceState.update("go-on-workflows", workflows);

      this._view?.webview.postMessage({
        type: "workflowStatusUpdate",
        workflowId,
        status: "completed",
      });

      vscode.window.showInformationMessage(
        t(MessageKeys.workflowCompletedSuccess, workflow.name),
      );
    } catch (error: unknown) {
      workflow.status = "failed";
      await this.context.workspaceState.update("go-on-workflows", workflows);
      const message = getErrorMessage(error);

      this._view?.webview.postMessage({
        type: "workflowStatusUpdate",
        workflowId,
        status: "failed",
        error: message,
      });

      vscode.window.showErrorMessage(
        t(MessageKeys.workflowExecutionFailed, message),
      );
    }
  }

  private async _deleteWorkflow(workflowId: string): Promise<void> {
    try {
      const workflows = this.context.workspaceState.get<WorkflowStore>(
        "go-on-workflows",
        {},
      );
      delete workflows[workflowId];
      await this.context.workspaceState.update("go-on-workflows", workflows);

      void this._view?.webview.postMessage({
        type: "workflowDeleted",
        workflowId,
      });

      vscode.window.showInformationMessage(
        t(MessageKeys.workflowDeletedSuccess),
      );
    } catch (error: unknown) {
      vscode.window.showErrorMessage(
        t(
          MessageKeys.workflowDeleteFailed,
          error instanceof Error ? error.message : String(error),
        ),
      );
    }
  }

  private _getHtmlForWebview(webview: vscode.Webview) {
    const styleResetUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this._extensionUri, "media", "reset.css"),
    );
    const styleVSCodeUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this._extensionUri, "media", "vscode.css"),
    );
    const scriptUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this._extensionUri, "media", "workflow.js"),
    );

    const nonce = getNonce();

    return `<!DOCTYPE html>
            <html lang="en">
            <head>
                <meta charset="UTF-8">
                <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource} 'unsafe-inline'; img-src ${webview.cspSource} data:; script-src 'nonce-${nonce}';">
                <meta name="viewport" content="width=device-width, initial-scale=1.0">
                <link href="${styleResetUri}" rel="stylesheet">
                <link href="${styleVSCodeUri}" rel="stylesheet">
                <title>Go-On Workflow</title>
                <style>
                    .workflow-container {
                        height: 100%;
                        display: flex;
                        flex-direction: column;
                        padding: 10px;
                    }
                    .workflow-list {
                        flex: 1;
                        overflow-y: auto;
                        margin-bottom: 10px;
                    }
                    .workflow-item {
                        border: 1px solid var(--vscode-panel-border);
                        border-radius: 3px;
                        padding: 8px;
                        margin-bottom: 8px;
                        background: var(--vscode-input-background);
                    }
                    .workflow-header {
                        display: flex;
                        justify-content: space-between;
                        align-items: center;
                        margin-bottom: 4px;
                    }
                    .workflow-name {
                        font-weight: bold;
                    }
                    .workflow-status {
                        font-size: 0.8em;
                        padding: 2px 6px;
                        border-radius: 3px;
                    }
                    .status-created { background: var(--vscode-notificationsInfoIcon-foreground); color: white; }
                    .status-running { background: var(--vscode-progressBar-background); color: white; }
                    .status-completed { background: var(--vscode-notificationsInfoIcon-foreground); color: white; }
                    .status-failed { background: var(--vscode-notificationsErrorIcon-foreground); color: white; }
                    .workflow-steps {
                        font-size: 0.9em;
                        color: var(--vscode-descriptionForeground);
                    }
                    .workflow-controls {
                        display: flex;
                        gap: 5px;
                    }
                    .workflow-btn {
                        padding: 4px 8px;
                        background: var(--vscode-button-background);
                        color: var(--vscode-button-foreground);
                        border: none;
                        border-radius: 3px;
                        cursor: pointer;
                        font-size: 0.8em;
                    }
                    .workflow-btn:hover {
                        background: var(--vscode-button-hoverBackground);
                    }
                    .workflow-btn.danger {
                        background: var(--vscode-notificationsErrorIcon-foreground);
                    }
                    .create-workflow {
                        margin-bottom: 10px;
                    }
                    .create-btn {
                        width: 100%;
                        padding: 8px;
                        background: var(--vscode-button-background);
                        color: var(--vscode-button-foreground);
                        border: none;
                        border-radius: 3px;
                        cursor: pointer;
                    }
                    .create-btn:hover {
                        background: var(--vscode-button-hoverBackground);
                    }
                </style>
            </head>
            <body>
                <div class="workflow-container">
                    <button class="create-btn" id="createWorkflowBtn">${t(MessageKeys.createNewWorkflow)}</button>
                    <div class="workflow-list" id="workflowList"></div>
                </div>
                <script nonce="${nonce}" src="${scriptUri}"></script>
            </body>
            </html>`;
  }
}
