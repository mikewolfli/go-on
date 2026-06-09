import * as vscode from "vscode";
import { Logger } from "./logger";
import { RuntimeManagerLike } from "./managerTypes";
import { getNonce } from "./utils";

const log = Logger.forModule("multiAgentPanel");

interface AgentInfo {
  name: string;
  status: "thinking" | "working" | "idle" | "error";
  progress: number;
  latestOutput: string;
  phase?: string;
  lastUpdated?: string;
}

const _STATUS_ICONS: Record<AgentInfo["status"], string> = {
  thinking: "$(sync~spin)",
  working: "$(tools)",
  idle: "$(circle-outline)",
  error: "$(error)",
};

const _STATUS_CLASSES: Record<AgentInfo["status"], string> = {
  thinking: "status-thinking",
  working: "status-working",
  idle: "status-idle",
  error: "status-error",
};

export class MultiAgentPanelProvider implements vscode.WebviewViewProvider {
  public static readonly viewType = "go-on-multi-agent";
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
              await this._fetchAgents();
              break;
          }
        } catch (error: unknown) {
          const msg = error instanceof Error ? error.message : String(error);
          // eslint-disable-next-line no-console
          console.warn("[multiAgentPanel] message error:", msg);
        }
      },
      undefined,
    );

    // Start polling immediately
    this._startPolling();
  }

  private readonly POLL_INTERVAL_MS = 5000;

  private _startPolling(): void {
    this._stopPolling();
    this._fetchAgents();
    this.pollTimer = setInterval(() => {
      if (this._disposed) return;
      this._fetchAgents();
    }, this.POLL_INTERVAL_MS);
  }

  private _stopPolling(): void {
    if (this.pollTimer) {
      clearInterval(this.pollTimer);
      this.pollTimer = undefined;
    }
  }

  private async _fetchAgents(): Promise<void> {
    if (!this._view || !this.manager.isRunning()) return;

    try {
      // Fetch agent status from backend health probes
      const probeResult = (await this.manager.sendRequest(
        "health.probes",
      )) as Record<string, unknown>;

      const probes = probeResult?.probes as Record<string, unknown> | undefined;
      const dependencies = Array.isArray(probes?.dependencies)
        ? (probes!.dependencies as Array<Record<string, unknown>>)
        : [];

      const agents: AgentInfo[] = [];

      // Look for dependency entries that describe agents
      for (const dep of dependencies) {
        if (typeof dep !== "object" || dep === null) continue;

        const depName = String(dep.name ?? "");
        // Only process entries that look like agents
        if (
          !depName ||
          depName === "provider_dependencies" ||
          depName === "runtime_dependencies"
        )
          continue;

        const depStatus = String(dep.status ?? "unknown");
        const depMessage = String(dep.message ?? "");

        const agent: AgentInfo = {
          name: depName,
          status: this._mapStatus(depStatus),
          progress: this._inferProgress(depStatus, dep),
          latestOutput: depMessage || depStatus || "",
          phase: String(dep.phase ?? ""),
          lastUpdated: new Date().toLocaleTimeString(),
        };
        agents.push(agent);
      }

      // If no agent-like dependencies, try an explicit agent status RPC
      if (agents.length === 0) {
        try {
          const agentStatus = (await this.manager.sendRequest(
            "agent.status",
          )) as Record<string, unknown>;
          if (agentStatus && typeof agentStatus === "object") {
            const entries = Array.isArray(agentStatus.agents)
              ? (agentStatus.agents as Array<Record<string, unknown>>)
              : [];
            for (const entry of entries) {
              agents.push({
                name: String(entry.name ?? "Agent"),
                status: this._mapStatus(String(entry.status ?? "idle")),
                progress: Number(entry.progress ?? 0),
                latestOutput: String(entry.output ?? entry.status ?? ""),
                phase: String(entry.phase ?? ""),
                lastUpdated: new Date().toLocaleTimeString(),
              });
            }
          }
        } catch (err) {
          log.warn("agent.status RPC failed:", err);
        }
      }

      // If still no agents, show a placeholder
      if (agents.length === 0) {
        agents.push({
          name: "No agents active",
          status: "idle",
          progress: 0,
          latestOutput: "No agent data available from backend",
          lastUpdated: new Date().toLocaleTimeString(),
        });
      }

      const total = agents.length;
      const running = agents.filter(
        (a) => a.status === "thinking" || a.status === "working",
      ).length;

      this._view.webview.postMessage({
        type: "agentsUpdate",
        agents,
        summary: { total, running },
      });
    } catch (err) {
      log.warn("_fetchAgents failed:", err);
    }
  }

  private _mapStatus(raw: string): AgentInfo["status"] {
    const s = raw.toLowerCase();
    if (s.includes("think") || s.includes("reason")) return "thinking";
    if (s.includes("work") || s.includes("process") || s.includes("active"))
      return "working";
    if (s.includes("error") || s.includes("fail") || s.includes("degraded"))
      return "error";
    return "idle";
  }

  private _inferProgress(
    _status: string,
    dep: Record<string, unknown>,
  ): number {
    const details = dep.details as Record<string, unknown> | undefined;
    if (details) {
      const ready = Number(details.ready ?? 0);
      const total = Number(details.total ?? 0);
      if (total > 0) return Math.round((ready / total) * 100);
    }
    return 0;
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
  <title>Go-On Agents</title>
  <style>
    .agent-container { padding: 8px; height: 100%; display: flex; flex-direction: column; gap: 6px; }
    .agent-summary { font-size: 0.85em; color: var(--vscode-descriptionForeground); padding-bottom: 6px; border-bottom: 1px solid var(--vscode-panel-border); }
    .agent-card { border: 1px solid var(--vscode-panel-border); border-radius: 6px; padding: 8px; background: var(--vscode-input-background); }
    .agent-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 4px; }
    .agent-name { font-weight: bold; font-size: 0.9em; }
    .agent-status { font-size: 0.75em; padding: 2px 6px; border-radius: 4px; }
    .status-thinking { background: var(--vscode-progressBar-background); color: #fff; }
    .status-working { background: var(--vscode-notificationsInfoIcon-foreground); color: #fff; }
    .status-idle { background: var(--vscode-editorWidget-background); color: var(--vscode-descriptionForeground); border: 1px solid var(--vscode-panel-border); }
    .status-error { background: var(--vscode-notificationsErrorIcon-foreground); color: #fff; }
    .progress-bar { height: 4px; background: var(--vscode-editorWidget-background); border-radius: 2px; margin: 4px 0; overflow: hidden; }
    .progress-fill { height: 100%; background: var(--vscode-progressBar-background); border-radius: 2px; transition: width 0.3s ease; }
    .agent-output { font-size: 0.78em; color: var(--vscode-descriptionForeground); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; margin-top: 2px; }
    .agent-phase { font-size: 0.72em; color: var(--vscode-textLink-foreground); }
    .agent-timestamp { font-size: 0.65em; color: var(--vscode-descriptionForeground); text-align: right; margin-top: 2px; }
    .empty-state { text-align: center; padding: 20px; color: var(--vscode-descriptionForeground); font-size: 0.85em; }
  </style>
</head>
<body>
  <div class="agent-container">
    <div class="agent-summary" id="summaryBar">No agents</div>
    <div id="agentList" style="flex:1;overflow-y:auto;"></div>
  </div>
  <script nonce="${nonce}">
    (function() {
      const agentList = document.getElementById('agentList');
      const summaryBar = document.getElementById('summaryBar');

      function renderAgent(agent) {
        const progressPct = Math.min(Math.max(agent.progress || 0, 0), 100);
        const statusClass = 'status-' + (agent.status || 'idle');
        const phaseHtml = agent.phase ? '<div class="agent-phase">Phase: ' + agent.phase + '</div>' : '';
        return '<div class="agent-card">' +
          '<div class="agent-header">' +
            '<span class="agent-name">' + escapeHtml(agent.name) + '</span>' +
            '<span class="agent-status ' + statusClass + '">' + escapeHtml(agent.status) + '</span>' +
          '</div>' +
          phaseHtml +
          '<div class="progress-bar"><div class="progress-fill" style="width:' + progressPct + '%"></div></div>' +
          '<div class="agent-output">' + escapeHtml(agent.latestOutput || '') + '</div>' +
          (agent.lastUpdated ? '<div class="agent-timestamp">' + agent.lastUpdated + '</div>' : '') +
        '</div>';
      }

      function escapeHtml(str) {
        if (!str) return '';
        return str.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
      }

      window.addEventListener('message', function(event) {
        const message = event.data;
        if (message.type === 'agentsUpdate') {
          const agents = message.agents || [];
          const summary = message.summary || {};
          let html = '';
          for (let i = 0; i < agents.length; i++) {
            html += renderAgent(agents[i]);
          }
          agentList.innerHTML = html || '<div class="empty-state">No agents available</div>';
          summaryBar.textContent = summary.total + ' agent' + (summary.total !== 1 ? 's' : '') +
            (summary.running > 0 ? ' (' + summary.running + ' active)' : '');
        }
      });

      // Notify the extension that the webview is ready
      acquireVsCodeApi().postMessage({ type: 'ready' });
    })();
  </script>
</body>
</html>`;
  }
}
