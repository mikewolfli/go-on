import * as vscode from "vscode";
import { i18n, MessageKeys } from "./i18n";
import { getNonce } from "./utils";

/**
 * Generate the HTML for the main Settings webview.
 * Extracted from settingsView.ts to separate HTML template logic.
 *
 * @param webview - The VS Code webview instance.
 * @param extensionUri - URI of the extension root for resolving resource paths.
 * @param isRunning - Whether the Go-On runtime is currently running.
 * @returns A complete HTML document string.
 */
export function getSettingsHtml(
  webview: vscode.Webview,
  extensionUri: vscode.Uri,
  isRunning: boolean,
): string {
  const styleResetUri = webview.asWebviewUri(
    vscode.Uri.joinPath(extensionUri, "media", "reset.css"),
  );
  const styleVSCodeUri = webview.asWebviewUri(
    vscode.Uri.joinPath(extensionUri, "media", "vscode.css"),
  );
  const scriptUri = webview.asWebviewUri(
    vscode.Uri.joinPath(extensionUri, "media", "settings.js"),
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
                <title>Go-On Settings</title>
                <style>
                    .settings-container {
                        padding: 10px;
                        height: 100%;
                        overflow-y: auto;
                    }
                    .setting-group {
                        margin-bottom: 20px;
                        border: 1px solid var(--vscode-panel-border);
                        border-radius: 3px;
                        padding: 10px;
                    }
                    .setting-group h3 {
                        margin: 0 0 10px 0;
                        color: var(--vscode-textLink-foreground);
                        border-bottom: 1px solid var(--vscode-panel-border);
                        padding-bottom: 5px;
                    }
                    .setting-item {
                        margin-bottom: 10px;
                    }
                    .setting-item label {
                        display: block;
                        margin-bottom: 4px;
                        font-weight: bold;
                    }
                    .setting-item input, .setting-item select {
                        width: 100%;
                        padding: 4px 8px;
                        border: 1px solid var(--vscode-input-border);
                        border-radius: 3px;
                        background: var(--vscode-input-background);
                        color: var(--vscode-input-foreground);
                    }
                    .setting-item input[type="checkbox"] {
                        width: auto;
                        margin-right: 8px;
                    }
                    .setting-item input[type="number"] {
                        width: 80px;
                    }
                    .action-buttons {
                        margin-top: 20px;
                        display: flex;
                        flex-wrap: wrap;
                        gap: 5px;
                    }
                    .action-button {
                        padding: 6px 12px;
                        background: var(--vscode-button-background);
                        color: var(--vscode-button-foreground);
                        border: none;
                        border-radius: 3px;
                        cursor: pointer;
                        font-size: 0.9em;
                    }
                    .action-button:hover {
                        background: var(--vscode-button-hoverBackground);
                    }
                    .action-button.danger {
                        background: var(--vscode-notificationsErrorIcon-foreground);
                    }
                    .status-indicator {
                        display: inline-block;
                        width: 8px;
                        height: 8px;
                        border-radius: 50%;
                        margin-right: 5px;
                    }
                    .status-indicator.connected {
                        background: var(--vscode-charts-green);
                    }
                    .status-indicator.disconnected {
                        background: var(--vscode-notificationsErrorIcon-foreground);
                    }
                </style>
            </head>
            <body>
                <div class="settings-container">
                    <div class="setting-group">
                        <h3>🖥️ System Configuration</h3>
                        <div class="setting-item">
                            <label for="configPath">Config File Path:</label>
                            <input type="text" id="configPath" data-setting="go-on.configPath">
                        </div>
                        <div class="setting-item">
                            <label for="executablePath">Executable Path:</label>
                            <input type="text" id="executablePath" data-setting="go-on.executablePath">
                        </div>
                        <div class="setting-item">
                            <label>
                                <input type="checkbox" id="autoDownloadBinary" data-setting="go-on.autoDownloadBinary">
                                Auto-download app binary when missing
                            </label>
                        </div>
                        <div class="setting-item">
                            <label for="releaseRepository">Release Repository (owner/repo):</label>
                            <input type="text" id="releaseRepository" data-setting="go-on.releaseRepository">
                        </div>
                        <div class="setting-item">
                            <label for="releaseTag">Release Tag:</label>
                            <input type="text" id="releaseTag" data-setting="go-on.releaseTag">
                        </div>
                        <div class="setting-item">
                            <label>
                                <input type="checkbox" id="autoStart" data-setting="go-on.autoStart">
                                Auto-start Go-On on workspace open
                            </label>
                        </div>
                        <div class="setting-item">
                            <label for="defaultTemplate">Default Config Template:</label>
                            <select id="defaultTemplate">
                                <option value="config.toml.autopilot-adaptive">autopilot-adaptive</option>
                            </select>
                        </div>
                        <div class="action-buttons">
                            <button class="action-button" id="applyDefaultTemplate">Apply As Active config.toml</button>
                            <button class="action-button" id="openConfigWizard">Open Config Wizard</button>
                        </div>
                    </div>

                    <div class="setting-group">
                        <h3>🔐 System Keyring (Preferred)</h3>
                        <div class="setting-item">
                            <label for="secretName">Secret Name:</label>
                          <select id="secretName"></select>
                        </div>
                        <div class="setting-item">
                            <label for="secretValue">Secret Value:</label>
                            <input type="password" id="secretValue" autocomplete="off" placeholder="Enter API key or token">
                        </div>
                        <div class="action-buttons">
                            <button class="action-button" id="setKeyringSecret">Save to Keyring</button>
                            <button class="action-button" id="getKeyringSecret">Read from Keyring</button>
                            <button class="action-button" id="listKeyringSecrets">List Key Status</button>
                            <button class="action-button danger" id="deleteKeyringSecret">Delete Key</button>
                        </div>
                        <div class="setting-item" style="margin-top: 8px;">
                            <label for="keyringOutput">Keyring Output:</label>
                            <textarea id="keyringOutput" rows="5" style="width: 100%;"></textarea>
                        </div>
                    </div>

                    <div class="setting-group">
                        <h3>📜 Rules Settings</h3>
                        <div class="setting-item">
                            <label for="globalRules">Global Rules (RULES/global.md, one per line):</label>
                            <textarea id="globalRules" rows="5" style="width: 100%;" placeholder="Rule line 1&#10;Rule line 2"></textarea>
                        </div>
                        <div class="setting-item">
                            <label for="commonRules">Common Rules (RULES/common.md, one per line):</label>
                            <textarea id="commonRules" rows="5" style="width: 100%;" placeholder="Rule line 1&#10;Rule line 2"></textarea>
                        </div>
                        <div class="setting-item">
                            <label for="phaseRules">Per-Phase Rules (format: phase|rule text):</label>
                            <textarea id="phaseRules" rows="6" style="width: 100%;" placeholder="coding|Must include tests&#10;review|Fail closed on uncertainty"></textarea>
                        </div>
                        <div class="action-buttons">
                            <button class="action-button" id="applyRulesSettings">Save Rules</button>
                        </div>
                    </div>

                    <div class="setting-group">
                        <h3>🧭 Workflow And AI Routing</h3>
                        <div class="setting-item">
                            <label for="defaultPhaseInput">Default Phase:</label>
                            <input type="text" id="defaultPhaseInput" placeholder="coding">
                        </div>
                        <div class="setting-item">
                            <label for="workflowMapping">Node Mapping JSON:</label>
                            <textarea id="workflowMapping" rows="12" style="width: 100%;" placeholder='{"coding":{"agents":["copilot","deepseek"],"fallback":true,"principles":["Prefer safe changes"],"switchRules":{"circuitBreakerFailures":3,"circuitBreakerOpenSeconds":30}}}'></textarea>
                        </div>
                        <div class="action-buttons">
                            <button class="action-button" id="applyWorkflowMapping">Save Workflow Mapping</button>
                        </div>
                    </div>

                    <div class="setting-group">
                        <h3>🤖 Provider Model Routing</h3>
                        <div class="setting-item">
                            <label for="providerSelect">Provider:</label>
                            <select id="providerSelect"></select>
                        </div>
                        <div class="setting-item">
                            <label for="providerModelSelect">Model:</label>
                            <select id="providerModelSelect"></select>
                        </div>
                        <div class="setting-item">
                            <label for="providerEnvVar">API Key Env Var:</label>
                            <input type="text" id="providerEnvVar" placeholder="Optional, inferred when empty">
                        </div>
                        <div class="setting-item" id="copilotAuthPanel" style="display: none;">
                          <label for="copilotOauthClientId">GitHub OAuth Client ID For Device Flow:</label>
                          <input type="text" id="copilotOauthClientId" placeholder="Required only for device flow">
                          <div class="action-buttons" style="margin-top: 8px;">
                            <button class="action-button" id="authorizeCopilotGitHubSession">Authorize With GitHub Login</button>
                            <button class="action-button" id="authorizeCopilotDeviceFlow">Authorize With Device Code</button>
                            <button class="action-button" id="refreshCopilotModels">Refresh Copilot Models</button>
                            <button class="action-button danger" id="cancelCopilotDeviceFlow">Cancel Device Flow</button>
                            <button class="action-button danger" id="deleteCopilotAuthorization">Delete Stored Copilot Token</button>
                          </div>
                          <div class="setting-item" style="margin-top: 8px;">
                            <label for="copilotAuthOutput">Copilot Authorization And Model Status:</label>
                            <textarea id="copilotAuthOutput" rows="6" style="width: 100%;"></textarea>
                          </div>
                        </div>
                        <div class="action-buttons">
                            <button class="action-button" id="applyProviderSelection">Apply Provider/Model To config.toml</button>
                        </div>
                    </div>

                    <div class="setting-group">
                        <h3>💬 Chat Settings</h3>
                        <div class="setting-item">
                            <label for="maxHistory">Max Chat History:</label>
                            <input type="number" id="maxHistory" min="1" max="1000" data-setting="go-on.chat.maxHistory">
                        </div>
                        <div class="setting-item">
                            <label for="model">Default Model:</label>
                            <select id="model" data-setting="go-on.chat.model">
                                <option value="auto">Auto</option>
                                <option value="copilot">GitHub Copilot</option>
                                <option value="deepseek">DeepSeek</option>
                                <option value="wenxin">Wenxin</option>
                                <option value="openai_compatible">OpenAI Compatible</option>
                                <option value="doubao">Doubao</option>
                                <option value="claude">Claude</option>
                            </select>
                        </div>
                        <div class="setting-item">
                            <label for="temperature">Temperature:</label>
                            <input type="number" id="temperature" min="0" max="2" step="0.1" data-setting="go-on.chat.temperature">
                        </div>
                        <div class="setting-item">
                            <label for="maxTokens">Max Tokens:</label>
                            <input type="number" id="maxTokens" min="1" max="32768" data-setting="go-on.chat.maxTokens">
                        </div>
                        <div class="setting-item">
                            <label>
                                <input type="checkbox" id="streaming" data-setting="go-on.chat.streaming">
                                Enable streaming responses
                            </label>
                        </div>
                    </div>

                    <div class="setting-group">
                        <h3>🧠 Memory & Cache</h3>
                        <div class="setting-item">
                            <label>
                                <input type="checkbox" id="cacheEnabled" data-setting="go-on.cache.enabled">
                                Enable response caching
                            </label>
                        </div>
                        <div class="setting-item">
                            <label>
                                <input type="checkbox" id="vectorEnabled" data-setting="go-on.vector.enabled">
                                Enable vector memory
                            </label>
                        </div>
                        <div class="setting-item">
                            <label for="healthInterval">Health Check Interval (seconds):</label>
                            <input type="number" id="healthInterval" min="30" max="3600" data-setting="go-on.health.interval">
                        </div>
                    </div>

                    <div class="setting-group">
                        <h3>🎨 UI Settings</h3>
                        <div class="setting-item">
                            <label for="uiTheme">Theme:</label>
                            <select id="uiTheme" data-setting="go-on.ui.theme">
                                <option value="auto">Auto (Follow VS Code)</option>
                                <option value="light">Light</option>
                                <option value="dark">Dark</option>
                            </select>
                        </div>
                        <div class="setting-item">
                            <label for="fontSize">Font Size:</label>
                            <input type="number" id="fontSize" min="8" max="24" data-setting="go-on.ui.fontSize">
                        </div>
                    </div>

                    <div class="action-buttons">
                        <button class="action-button" id="startGoOn">Start Go-On</button>
                        <button class="action-button" id="stopGoOn">Stop Go-On</button>
                        <button class="action-button" id="healthCheck">Health Check</button>
                        <button class="action-button" id="healthProbes">Health Probes</button>
                        <button class="action-button" id="lockStatus">Lock Status</button>
                        <button class="action-button" id="observabilityAlerts">Observability Alerts</button>
                        <button class="action-button" id="securityBaseline" data-feature="entry_auth,production_strict">Security Baseline</button>
                        <button class="action-button" id="harnessStatus" data-feature="harness_bus">Harness Status</button>
                        <button class="action-button" id="breakerStatus">Breaker Status</button>
                        <button class="action-button" id="breakerRecovery">Breaker Recovery</button>
                        <button class="action-button danger" id="clearCache" data-feature="response_cache">Clear Cache</button>
                        <button class="action-button danger" id="clearVector" data-feature="vector_store">Clear Vector</button>
                        <button class="action-button" id="reloadConfig">Reload Config</button>
                        <button class="action-button" id="workflowExecute" data-feature="skills_enabled,skills_import">Workflow Execute</button>
                        <button class="action-button" id="taskPlan">Task Plan</button>
                        <button class="action-button" id="taskExecute">Task Execute</button>
                        <button class="action-button" id="learningSummary">Learning Summary</button>
                        <button class="action-button" id="learningGuardrail">Learning Guardrail</button>
                        <button class="action-button" id="learningReplay">Learning Replay</button>
                        <button class="action-button" id="knowledgeDistill">Knowledge Distill</button>
                        <button class="action-button" id="rlAlignmentEval">RL Alignment Eval</button>
                        <button class="action-button" id="hardnessStatus">Hardness Status</button>
                        <button class="action-button" id="costStatus">Cost Status</button>
                        <button class="action-button" id="configBaseline">Config Baseline</button>
                        <button class="action-button" id="errorContract">Error Contract</button>
                        <button class="action-button" id="buildRepro">Build Repro</button>
                        <button class="action-button" id="dataLifecycle">Data Lifecycle</button>
                        <button class="action-button" id="optimizationPeak">Optimization Peak</button>
                        <button class="action-button" id="releaseReadiness">Release Readiness</button>
                        <button class="action-button" id="runtimeStability">Runtime Stability</button>
                        <button class="action-button" id="autotuneStatus" data-feature="autotune">Autotune Status</button>
                        <button class="action-button" id="governanceStatus">Governance Status</button>
                        <button class="action-button" id="governancePlanGet">Governance Plan</button>
                        <button class="action-button" id="governanceAuditRecent">Governance Audit</button>
                        <button class="action-button" id="debugPanelGet">Debug Panel</button>
                        <button class="action-button" id="actionCheck">Action Check</button>
                    </div>

                    <div style="margin-top: 20px; padding: 10px; background: var(--vscode-textBlockQuote-background); border-left: 3px solid var(--vscode-textBlockQuote-border);">
                        <strong>Status:</strong>
                        <span class="status-indicator ${isRunning ? "connected" : "disconnected"}"></span>
                        ${isRunning ? "Connected" : "Disconnected"}
                    </div>

                    <div class="setting-item" style="margin-top: 8px;">
                        <label for="settingsActionOutput">Settings Action Output:</label>
                        <textarea id="settingsActionOutput" rows="4" style="width: 100%;"></textarea>
                    </div>
                </div>
                <script nonce="${nonce}" src="${scriptUri}"></script>
            </body>
            </html>`;
}

/**
 * Generate the HTML for the Config Wizard webview panel.
 *
 * @param webview - The VS Code webview instance.
 * @param config - Current configuration values for pre-fill.
 * @returns A complete HTML document string.
 */
export function getConfigWizardHtml(
  webview: vscode.Webview,
  config: {
    configPath: string;
    executablePath: string;
    autoStart: boolean;
    protocolMode: string;
  },
): string {
  const nonce = getNonce();
  const payload = JSON.stringify(config).replace(/</g, "\\u003c");

  return `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource} 'unsafe-inline'; img-src ${webview.cspSource} data:; script-src 'nonce-${nonce}';">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>${i18n.getMessage(MessageKeys.configWizardTitle)}</title>
    <style>
        body { font-family: var(--vscode-font-family); color: var(--vscode-foreground); background: var(--vscode-editor-background); padding: 20px; }
        .header { margin-bottom: 16px; }
        .title { font-size: 22px; font-weight: 700; }
        .subtitle { color: var(--vscode-descriptionForeground); margin-top: 6px; }
        .steps { display: flex; gap: 8px; margin: 18px 0 20px; }
        .step { flex: 1; border: 1px solid var(--vscode-panel-border); border-radius: 8px; padding: 10px; color: var(--vscode-descriptionForeground); }
        .step.active { border-color: var(--vscode-focusBorder); color: var(--vscode-foreground); }
        .cards, .modes { display: grid; gap: 12px; }
        .cards { grid-template-columns: repeat(3, minmax(0, 1fr)); }
        .modes { grid-template-columns: repeat(2, minmax(0, 1fr)); }
        .card { border: 1px solid var(--vscode-panel-border); border-radius: 10px; padding: 14px; cursor: pointer; background: var(--vscode-sideBar-background); }
        .card.selected { border-color: var(--vscode-focusBorder); background: var(--vscode-list-activeSelectionBackground); }
        .card-title { font-weight: 700; margin-bottom: 8px; }
        .card-desc { color: var(--vscode-descriptionForeground); line-height: 1.6; font-size: 12px; }
        .recommended { display: inline-block; margin-top: 8px; color: var(--vscode-testing-iconPassed); font-size: 12px; }
        .review { display: grid; gap: 10px; }
        .review-item { border: 1px solid var(--vscode-panel-border); border-radius: 8px; padding: 10px; }
        .review-label { font-size: 12px; color: var(--vscode-descriptionForeground); margin-bottom: 4px; }
        .review-value { font-weight: 600; word-break: break-all; }
        .actions { display: flex; justify-content: space-between; margin-top: 20px; }
        button { border: none; border-radius: 6px; padding: 8px 14px; cursor: pointer; }
        .ghost { background: var(--vscode-button-secondaryBackground); color: var(--vscode-button-secondaryForeground); }
        .primary { background: var(--vscode-button-background); color: var(--vscode-button-foreground); }
        @media (max-width: 760px) { .cards, .modes { grid-template-columns: 1fr; } }
    </style>
</head>
<body>
    <div class="header">
        <div class="title">${i18n.getMessage(MessageKeys.configWizardTitle)}</div>
        <div class="subtitle">${i18n.getMessage(MessageKeys.configWizardSubtitle)}</div>
    </div>
    <div class="steps">
        <div class="step active" data-step-indicator="0">${i18n.getMessage(MessageKeys.configWizardStep1)}</div>
        <div class="step" data-step-indicator="1">${i18n.getMessage(MessageKeys.configWizardStep2)}</div>
        <div class="step" data-step-indicator="2">${i18n.getMessage(MessageKeys.configWizardStep3)}</div>
    </div>
    <div id="step0">
        <div class="cards">
            <div class="card selected" data-scenario="local">
                <div class="card-title">${i18n.getMessage(MessageKeys.configWizardLocalTitle)}</div>
                <div class="card-desc">${i18n.getMessage(MessageKeys.configWizardLocalDesc)}</div>
            </div>
            <div class="card" data-scenario="shared">
                <div class="card-title">${i18n.getMessage(MessageKeys.configWizardSharedTitle)}</div>
                <div class="card-desc">${i18n.getMessage(MessageKeys.configWizardSharedDesc)}</div>
            </div>
            <div class="card" data-scenario="editor">
                <div class="card-title">${i18n.getMessage(MessageKeys.configWizardEditorTitle)}</div>
                <div class="card-desc">${i18n.getMessage(MessageKeys.configWizardEditorDesc)}</div>
            </div>
        </div>
    </div>
    <div id="step1" hidden>
        <div class="modes">
            <div class="card" data-mode="from_config"><div class="card-title">from_config</div><div class="card-desc">Follow project config.toml</div></div>
            <div class="card selected" data-mode="adaptive"><div class="card-title">adaptive</div><div class="card-desc">${i18n.getMessage(MessageKeys.configWizardAdaptiveDesc)}</div><span class="recommended">${i18n.getMessage(MessageKeys.configWizardRecommended)}</span></div>
            <div class="card" data-mode="acp_stdio"><div class="card-title">acp_stdio</div><div class="card-desc">${i18n.getMessage(MessageKeys.configWizardAcpStdioDesc)}</div></div>
            <div class="card" data-mode="acp_http"><div class="card-title">acp_http</div><div class="card-desc">${i18n.getMessage(MessageKeys.configWizardAcpHttpDesc)}</div></div>
            <div class="card" data-mode="mcp_stdio"><div class="card-title">mcp_stdio</div><div class="card-desc">${i18n.getMessage(MessageKeys.configWizardMcpStdioDesc)}</div></div>
            <div class="card" data-mode="mcp_http"><div class="card-title">mcp_http</div><div class="card-desc">${i18n.getMessage(MessageKeys.configWizardMcpHttpDesc)}</div></div>
        </div>
    </div>
    <div id="step2" hidden>
        <div class="review">
            <div class="review-item"><div class="review-label">${i18n.getMessage(MessageKeys.configPath)}</div><div class="review-value" id="review-config-path"></div></div>
            <div class="review-item"><div class="review-label">${i18n.getMessage(MessageKeys.executablePath)}</div><div class="review-value" id="review-executable-path"></div></div>
            <div class="review-item"><div class="review-label">${i18n.getMessage(MessageKeys.autoStart)}</div><div class="review-value" id="review-auto-start"></div></div>
            <div class="review-item"><div class="review-label">${i18n.getMessage(MessageKeys.configWizardProtocolMode)}</div><div class="review-value" id="review-protocol-mode"></div></div>
        </div>
    </div>
    <div class="actions">
        <button class="ghost" id="cancel-btn">${i18n.getMessage(MessageKeys.cancel)}</button>
        <div>
            <button class="ghost" id="prev-btn" disabled>${i18n.getMessage(MessageKeys.configWizardPrevious)}</button>
            <button class="primary" id="next-btn">${i18n.getMessage(MessageKeys.configWizardNext)}</button>
        </div>
    </div>
    <script nonce="${nonce}">
        const vscode = acquireVsCodeApi();
        const initial = ${payload};
        const state = {
            step: 0,
            scenario: 'local',
            configPath: initial.configPath,
            executablePath: initial.executablePath,
            autoStart: initial.autoStart,
            protocolMode: initial.protocolMode || 'adaptive',
        };

        const recommendations = {
            local: 'adaptive',
            shared: 'acp_http',
            editor: 'acp_stdio',
        };

        function render() {
            document.querySelectorAll('[data-step-indicator]').forEach((el, index) => {
                el.classList.toggle('active', index === state.step);
            });
            document.getElementById('step0').hidden = state.step !== 0;
            document.getElementById('step1').hidden = state.step !== 1;
            document.getElementById('step2').hidden = state.step !== 2;
            document.getElementById('prev-btn').disabled = state.step === 0;
            document.getElementById('next-btn').textContent = state.step === 2 ? '${i18n.getMessage(MessageKeys.save)}' : '${i18n.getMessage("configuration.wizard.next")}';
            document.querySelectorAll('[data-scenario]').forEach((el) => {
                el.classList.toggle('selected', el.dataset.scenario === state.scenario);
            });
            document.querySelectorAll('[data-mode]').forEach((el) => {
                el.classList.toggle('selected', el.dataset.mode === state.protocolMode);
            });
            document.getElementById('review-config-path').textContent = state.configPath || './config.toml';
            document.getElementById('review-executable-path').textContent = state.executablePath || '(empty)';
            document.getElementById('review-auto-start').textContent = state.autoStart ? 'true' : 'false';
            document.getElementById('review-protocol-mode').textContent = state.protocolMode;
        }

        document.querySelectorAll('[data-scenario]').forEach((el) => {
            el.addEventListener('click', () => {
                state.scenario = el.dataset.scenario;
                state.protocolMode = recommendations[state.scenario] || 'adaptive';
                state.autoStart = state.scenario === 'shared';
                render();
            });
        });

        document.querySelectorAll('[data-mode]').forEach((el) => {
            el.addEventListener('click', () => {
                state.protocolMode = el.dataset.mode;
                render();
            });
        });

        document.getElementById('cancel-btn').addEventListener('click', () => {
            vscode.postMessage({ command: 'cancel' });
        });

        document.getElementById('prev-btn').addEventListener('click', () => {
            if (state.step > 0) state.step -= 1;
            render();
        });

        document.getElementById('next-btn').addEventListener('click', () => {
            if (state.step < 2) {
                state.step += 1;
                render();
                return;
            }
            vscode.postMessage({ command: 'saveConfig', config: state });
        });

        window.addEventListener('message', (event) => {
            if (event.data?.command === 'close') {
                window.close();
            }
        });

        render();
    </script>
</body>
</html>`;
}
