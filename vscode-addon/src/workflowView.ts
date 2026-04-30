import * as vscode from "vscode";
import { RuntimeManagerLike } from "./managerTypes";
import { t, MessageKeys } from "./i18n";

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
    this.context.subscriptions.push(
      new vscode.Disposable(() => this._messageSubscription?.dispose()),
    );
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
        switch (message.type) {
          case "createWorkflow":
            await this._createWorkflow(message.workflowData);
            break;
          case "runWorkflow":
            await this._runWorkflow(message.workflowId);
            break;
          case "deleteWorkflow":
            this._deleteWorkflow(message.workflowId);
            break;
        }
      },
      undefined,
    );
  }

  private getErrorMessage(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
  }

  private async _createWorkflow(workflowData: WorkflowData) {
    const workflows = this.context.globalState.get<WorkflowStore>(
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
    await this.context.globalState.update("go-on-workflows", workflows);

    this._view?.webview.postMessage({
      type: "workflowCreated",
      workflow: workflows[workflowId],
    });

    vscode.window.showInformationMessage(
      `Workflow "${workflowData.name}" created successfully!`,
    );
  }

  private async _runWorkflow(workflowId: string) {
    const workflows = this.context.globalState.get<WorkflowStore>(
      "go-on-workflows",
      {},
    );
    const workflow = workflows[workflowId];

    if (!workflow) {
      vscode.window.showErrorMessage("Workflow not found");
      return;
    }

    // Update status
    workflow.status = "running";
    await this.context.globalState.update("go-on-workflows", workflows);

    this._view?.webview.postMessage({
      type: "workflowStatusUpdate",
      workflowId,
      status: "running",
    });

    try {
      // Execute workflow steps
      for (let i = 0; i < workflow.steps.length; i++) {
        const step = workflow.steps[i];

        this._view?.webview.postMessage({
          type: "stepStatusUpdate",
          workflowId,
          stepIndex: i,
          status: "running",
        });

        // Execute step based on type
        switch (step.type) {
          case "chat":
            await this.manager.sendRequest("chat", {
              messages: [{ role: "user", content: step.prompt }],
            });
            break;
          case "code":
            // Code execution would be handled by the chat view
            break;
          case "delay":
            await new Promise((resolve) =>
              setTimeout(resolve, Number(step.delay || 0) * 1000),
            );
            break;
        }

        this._view?.webview.postMessage({
          type: "stepStatusUpdate",
          workflowId,
          stepIndex: i,
          status: "completed",
        });
      }

      workflow.status = "completed";
      await this.context.globalState.update("go-on-workflows", workflows);

      this._view?.webview.postMessage({
        type: "workflowStatusUpdate",
        workflowId,
        status: "completed",
      });

      vscode.window.showInformationMessage(
        `Workflow "${workflow.name}" completed successfully!`,
      );
    } catch (error: unknown) {
      workflow.status = "failed";
      await this.context.globalState.update("go-on-workflows", workflows);
      const message = this.getErrorMessage(error);

      this._view?.webview.postMessage({
        type: "workflowStatusUpdate",
        workflowId,
        status: "failed",
        error: message,
      });

      vscode.window.showErrorMessage(`Workflow failed: ${message}`);
    }
  }

  private async _deleteWorkflow(workflowId: string) {
    try {
      const workflows = this.context.globalState.get<WorkflowStore>(
        "go-on-workflows",
        {},
      );
      delete workflows[workflowId];
      await this.context.globalState.update("go-on-workflows", workflows);

      this._view?.webview.postMessage({
        type: "workflowDeleted",
        workflowId,
      });

      vscode.window.showInformationMessage("Workflow deleted successfully!");
    } catch (error) {
      vscode.window.showErrorMessage(
        `Failed to delete workflow: ${error instanceof Error ? error.message : "Unknown error"}`,
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

function getNonce() {
  let text = "";
  const possible =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  for (let i = 0; i < 32; i++) {
    text += possible.charAt(Math.floor(Math.random() * possible.length));
  }
  return text;
}
