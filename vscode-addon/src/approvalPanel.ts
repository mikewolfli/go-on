import * as vscode from "vscode";
import { RuntimeManagerLike } from "./managerTypes";
import { getNonce } from "./utils";

interface ApprovalRequest {
  id: string;
  agentName: string;
  action: string;
  riskLevel: "low" | "medium" | "high" | "critical";
  description: string;
  details?: Record<string, unknown>;
  timestamp: string;
}

const RISK_COLORS: Record<ApprovalRequest["riskLevel"], string> = {
  low: "var(--vscode-testing-iconPassed)",
  medium: "var(--vscode-notificationsInfoIcon-foreground)",
  high: "var(--vscode-notificationsWarningIcon-foreground)",
  critical: "var(--vscode-notificationsErrorIcon-foreground)",
};

export class ApprovalPanelProvider implements vscode.WebviewViewProvider {
  public static readonly viewType = "go-on-approval";
  private _view?: vscode.WebviewView;
  private _messageSubscription?: vscode.Disposable;
  private readonly manager: RuntimeManagerLike;
  private pollTimer: NodeJS.Timeout | undefined;
  private _disposed = false;

  constructor(
    private readonly _extensionUri: vscode.Uri,
    _manager: RuntimeManagerLike,
  ) {
    this.manager = _manager;
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
            case "ready":
              this._startPolling();
              break;
            case "refresh":
              await this._fetchPendingRequests();
              break;
            case "approve":
              await this._handleApprove(message.requestId);
              break;
            case "reject":
              await this._handleReject(message.requestId);
              break;
            case "approveAll":
              await this._handleApproveAll();
              break;
          }
        } catch (error: unknown) {
          const msg = error instanceof Error ? error.message : String(error);
          // eslint-disable-next-line no-console
          console.warn("[approvalPanel] message error:", msg);
          this._view?.webview.postMessage({
            type: "error",
            message: msg,
          });
        }
      },
      undefined,
    );

    this._startPolling();
  }

  private readonly POLL_INTERVAL_MS = 5000;

  private _startPolling(): void {
    this._stopPolling();
    this._fetchPendingRequests();
    this.pollTimer = setInterval(() => {
      if (this._disposed) return;
      this._fetchPendingRequests();
    }, this.POLL_INTERVAL_MS);
  }

  private _stopPolling(): void {
    if (this.pollTimer) {
      clearInterval(this.pollTimer);
      this.pollTimer = undefined;
    }
  }

  private async _fetchPendingRequests(): Promise<void> {
    if (!this._view || !this.manager.isRunning()) return;

    try {
      const result = (await this.manager.sendRequest(
        "approval.pending",
      )) as Record<string, unknown>;

      const items = Array.isArray(result?.requests)
        ? (result.requests as Array<Record<string, unknown>>)
        : [];

      const requests: ApprovalRequest[] = items.map((item) => ({
        id: String(item.id ?? ""),
        agentName: String(item.agent_name ?? item.agentName ?? "Unknown"),
        action: String(item.action ?? "Unknown"),
        riskLevel: this._normalizeRiskLevel(
          String(item.risk_level ?? item.riskLevel ?? "medium"),
        ),
        description: String(
          item.description ?? item.action ?? "No description",
        ),
        details: item.details as Record<string, unknown> | undefined,
        timestamp: String(item.timestamp ?? new Date().toISOString()),
      }));

      this._view.webview.postMessage({
        type: "requestsUpdate",
        requests,
        count: requests.length,
      });
    } catch {
      // Backend not reachable or approval not supported
    }
  }

  private _normalizeRiskLevel(level: string): ApprovalRequest["riskLevel"] {
    const l = level.toLowerCase();
    if (l.includes("critical") || l === "critical") return "critical";
    if (l.includes("high")) return "high";
    if (l.includes("medium") || l.includes("med")) return "medium";
    return "low";
  }

  private async _handleApprove(requestId: string): Promise<void> {
    if (!this.manager.isRunning()) return;

    try {
      await this.manager.sendRequest("approval.approve", {
        request_id: requestId,
      });

      this._view?.webview.postMessage({
        type: "requestResolved",
        requestId,
        outcome: "approved",
      });

      // Refresh the list
      await this._fetchPendingRequests();
    } catch (error: unknown) {
      const msg = error instanceof Error ? error.message : String(error);
      this._view?.webview.postMessage({
        type: "error",
        requestId,
        message: msg,
      });
    }
  }

  private async _handleReject(requestId: string): Promise<void> {
    if (!this.manager.isRunning()) return;

    try {
      await this.manager.sendRequest("approval.reject", {
        request_id: requestId,
      });

      this._view?.webview.postMessage({
        type: "requestResolved",
        requestId,
        outcome: "rejected",
      });

      await this._fetchPendingRequests();
    } catch (error: unknown) {
      const msg = error instanceof Error ? error.message : String(error);
      this._view?.webview.postMessage({
        type: "error",
        requestId,
        message: msg,
      });
    }
  }

  private async _handleApproveAll(): Promise<void> {
    if (!this.manager.isRunning()) return;

    try {
      await this.manager.sendRequest("approval.approve_all", {});

      this._view?.webview.postMessage({
        type: "allResolved",
        outcome: "approved",
      });

      await this._fetchPendingRequests();
    } catch (error: unknown) {
      const msg = error instanceof Error ? error.message : String(error);
      this._view?.webview.postMessage({
        type: "error",
        message: msg,
      });
    }
  }

  public dispose(): void {
    this._disposed = true;
    this._stopPolling();
    this._messageSubscription?.dispose();
  }

  private _getHtmlForWebview(webview: vscode.Webview): string {
    const styleResetUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this._extensionUri, "media", "reset.css"),
    );
    const styleVSCodeUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this._extensionUri, "media", "vscode.css"),
    );

    const nonce = getNonce();

    return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource} 'unsafe-inline'; script-src 'nonce-${nonce}';">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <link href="${styleResetUri}" rel="stylesheet">
  <link href="${styleVSCodeUri}" rel="stylesheet">
  <title>Go-On Approval</title>
  <style>
    .approval-container { padding: 8px; height: 100%; display: flex; flex-direction: column; gap: 6px; }
    .approval-header { display: flex; justify-content: space-between; align-items: center; padding-bottom: 6px; border-bottom: 1px solid var(--vscode-panel-border); }
    .approval-title { font-weight: bold; font-size: 0.9em; }
    .approval-count { font-size: 0.8em; color: var(--vscode-descriptionForeground); }
    .empty-state { text-align: center; padding: 20px; color: var(--vscode-descriptionForeground); font-size: 0.85em; }
    .request-card { border: 1px solid var(--vscode-panel-border); border-radius: 6px; padding: 8px; margin-bottom: 6px; background: var(--vscode-input-background); }
    .request-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 4px; }
    .request-agent { font-weight: bold; font-size: 0.85em; }
    .request-risk { font-size: 0.7em; padding: 2px 6px; border-radius: 4px; color: #fff; font-weight: bold; }
    .request-action { font-size: 0.8em; color: var(--vscode-textLink-foreground); margin-bottom: 2px; }
    .request-description { font-size: 0.78em; color: var(--vscode-descriptionForeground); margin-bottom: 4px; }
    .request-timestamp { font-size: 0.65em; color: var(--vscode-descriptionForeground); margin-bottom: 6px; }
    .request-controls { display: flex; gap: 6px; justify-content: flex-end; }
    .btn { padding: 4px 12px; border: none; border-radius: 3px; cursor: pointer; font-size: 0.78em; font-weight: bold; }
    .btn-approve { background: var(--vscode-testing-iconPassed); color: #fff; }
    .btn-approve:hover { opacity: 0.85; }
    .btn-reject { background: var(--vscode-notificationsErrorIcon-foreground); color: #fff; }
    .btn-reject:hover { opacity: 0.85; }
    .btn-approve-all { background: var(--vscode-button-background); color: var(--vscode-button-foreground); width: 100%; padding: 6px; margin-bottom: 6px; }
    .btn-approve-all:hover { background: var(--vscode-button-hoverBackground); }
    .btn:disabled { opacity: 0.5; cursor: default; }
    .request-risk.low { background: var(--vscode-testing-iconPassed); }
    .request-risk.medium { background: var(--vscode-notificationsInfoIcon-foreground); }
    .request-risk.high { background: var(--vscode-notificationsWarningIcon-foreground); }
    .request-risk.critical { background: var(--vscode-notificationsErrorIcon-foreground); }
    .resolved-badge { font-size: 0.72em; padding: 2px 6px; border-radius: 3px; }
    .resolved-badge.approved { background: var(--vscode-testing-iconPassed); color: #fff; }
    .resolved-badge.rejected { background: var(--vscode-notificationsErrorIcon-foreground); color: #fff; }
  </style>
</head>
<body>
  <div class="approval-container">
    <div class="approval-header">
      <span class="approval-title">Approval Requests</span>
      <span class="approval-count" id="countBadge">0 pending</span>
    </div>
    <button class="btn btn-approve-all" id="approveAllBtn">Approve All</button>
    <div id="requestList" style="flex:1;overflow-y:auto;"></div>
  </div>
  <script nonce="${nonce}">
    (function() {
      const requestList = document.getElementById('requestList');
      const countBadge = document.getElementById('countBadge');
      const approveAllBtn = document.getElementById('approveAllBtn');
      let pendingCount = 0;
      let resolvedIds = {};

      function renderRequest(req) {
        const key = req.id;
        if (resolvedIds[key]) {
          return '<div class="request-card" style="opacity:0.5;">' +
            '<div class="request-header">' +
              '<span class="request-agent">' + escapeHtml(req.agentName) + '</span>' +
              '<span class="resolved-badge ' + resolvedIds[key] + '">' + resolvedIds[key] + '</span>' +
            '</div>' +
            '<div class="request-description">' + escapeHtml(req.description) + '</div>' +
          '</div>';
        }
        return '<div class="request-card" data-id="' + escapeHtml(key) + '">' +
          '<div class="request-header">' +
            '<span class="request-agent">' + escapeHtml(req.agentName) + '</span>' +
            '<span class="request-risk ' + req.riskLevel + '">' + escapeHtml(req.riskLevel) + '</span>' +
          '</div>' +
          '<div class="request-action">' + escapeHtml(req.action) + '</div>' +
          '<div class="request-description">' + escapeHtml(req.description) + '</div>' +
          '<div class="request-timestamp">' + escapeHtml(new Date(req.timestamp).toLocaleTimeString()) + '</div>' +
          '<div class="request-controls">' +
            '<button class="btn btn-approve" data-id="' + escapeHtml(key) + '" data-action="approve">Approve</button>' +
            '<button class="btn btn-reject" data-id="' + escapeHtml(key) + '" data-action="reject">Reject</button>' +
          '</div>' +
        '</div>';
      }

      function escapeHtml(str) {
        if (!str) return '';
        return str.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
      }

      window.addEventListener('message', function(event) {
        const message = event.data;
        if (message.type === 'requestsUpdate') {
          const requests = message.requests || [];
          pendingCount = requests.length;
          countBadge.textContent = pendingCount + ' pending';
          approveAllBtn.style.display = pendingCount > 0 ? 'block' : 'none';
          let html = '';
          for (let i = 0; i < requests.length; i++) {
            html += renderRequest(requests[i]);
          }
          requestList.innerHTML = html || '<div class="empty-state">No pending approval requests</div>';

          // Attach event listeners to buttons
          requestList.querySelectorAll('.btn-approve').forEach(function(btn) {
            btn.addEventListener('click', function() {
              btn.disabled = true;
              acquireVsCodeApi().postMessage({ type: 'approve', requestId: btn.getAttribute('data-id') });
            });
          });
          requestList.querySelectorAll('.btn-reject').forEach(function(btn) {
            btn.addEventListener('click', function() {
              btn.disabled = true;
              acquireVsCodeApi().postMessage({ type: 'reject', requestId: btn.getAttribute('data-id') });
            });
          });
        } else if (message.type === 'requestResolved') {
          resolvedIds[message.requestId] = message.outcome;
        } else if (message.type === 'allResolved') {
          // refresh will clear
        } else if (message.type === 'error') {
          console.warn('Approval error:', message.message);
        }
      });

      approveAllBtn.addEventListener('click', function() {
        approveAllBtn.disabled = true;
        acquireVsCodeApi().postMessage({ type: 'approveAll' });
      });

      acquireVsCodeApi().postMessage({ type: 'ready' });
    })();
  </script>
</body>
</html>`;
  }
}
