"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.GoOnSettingsViewProvider = void 0;
const vscode = require("vscode");
const i18n_1 = require("./i18n");
const configManager_1 = require("./configManager");
class GoOnSettingsViewProvider {
    constructor(_extensionUri, manager, context) {
        this._extensionUri = _extensionUri;
        this.manager = manager;
        this.context = context;
    }
    resolveWebviewView(webviewView, context, _token) {
        this._view = webviewView;
        webviewView.webview.options = {
            enableScripts: true,
            localResourceRoots: [this._extensionUri]
        };
        webviewView.webview.html = this._getHtmlForWebview(webviewView.webview);
        webviewView.webview.onDidReceiveMessage(async (message) => {
            switch (message.type) {
                case 'requestSettings':
                    this._sendCurrentSettings();
                    break;
                case 'updateRuntimeSetting':
                    await this._updateRuntimeSetting(message.key, message.value);
                    break;
                case 'updateCacheSetting':
                    await this._updateCacheSetting(message.key, message.value);
                    break;
                case 'updateVectorSetting':
                    await this._updateVectorSetting(message.key, message.value);
                    break;
                case 'updateAutotuneSetting':
                    await this._updateAutotuneSetting(message.key, message.value);
                    break;
                case 'addAgent':
                    await this._addAgent(message.name, message.config);
                    break;
                case 'deleteAgent':
                    await this._deleteAgent(message.name);
                    break;
                case 'updatePhase':
                    await this._updatePhase(message.name, message.config);
                    break;
                case 'startGoOn':
                    vscode.commands.executeCommand('go-on.start');
                    break;
                case 'stopGoOn':
                    vscode.commands.executeCommand('go-on.stop');
                    break;
                case 'healthCheck':
                    vscode.commands.executeCommand('go-on.healthCheck');
                    break;
                case 'clearCache':
                    vscode.commands.executeCommand('go-on.cacheClear');
                    break;
                case 'setLanguage':
                    await this._setLanguage(message.language);
                    break;
                case 'setKeyringSecret':
                    await this._handleKeyringSet(message.name, message.value);
                    break;
                case 'getKeyringSecret':
                    await this._handleKeyringGet(message.name);
                    break;
                case 'deleteKeyringSecret':
                    await this._handleKeyringDelete(message.name);
                    break;
                case 'listKeyringSecrets':
                    await this._handleKeyringList();
                    break;
                case 'applyDefaultConfigTemplate':
                    await this._handleApplyDefaultConfigTemplate(message.template);
                    break;
                case 'applyRulesSettings':
                    await this._handleApplyRulesSettings(message.payload);
                    break;
                case 'applyWorkflowMapping':
                    await this._handleApplyWorkflowMapping(message.payload);
                    break;
            }
        }, undefined, this.context.subscriptions);
        this._sendCurrentSettings();
    }
    // Settings update methods
    async _updateRuntimeSetting(key, value) {
        try {
            configManager_1.configManager.setConfigValue(`runtime.${key}`, value);
            await configManager_1.configManager.saveToFile();
            vscode.window.showInformationMessage(i18n_1.i18n.getMessage(i18n_1.MessageKeys.successfullySaved));
            this._sendCurrentSettings();
        }
        catch (error) {
            vscode.window.showErrorMessage(`${i18n_1.i18n.getMessage(i18n_1.MessageKeys.errorSaving)}: ${error.message}`);
        }
    }
    async _updateCacheSetting(key, value) {
        try {
            configManager_1.configManager.setConfigValue(`cache.${key}`, value);
            await configManager_1.configManager.saveToFile();
            vscode.window.showInformationMessage(i18n_1.i18n.getMessage(i18n_1.MessageKeys.successfullySaved));
            this._sendCurrentSettings();
        }
        catch (error) {
            vscode.window.showErrorMessage(`${i18n_1.i18n.getMessage(i18n_1.MessageKeys.errorSaving)}: ${error.message}`);
        }
    }
    async _updateVectorSetting(key, value) {
        try {
            configManager_1.configManager.setConfigValue(`vector.${key}`, value);
            await configManager_1.configManager.saveToFile();
            vscode.window.showInformationMessage(i18n_1.i18n.getMessage(i18n_1.MessageKeys.successfullySaved));
            this._sendCurrentSettings();
        }
        catch (error) {
            vscode.window.showErrorMessage(`${i18n_1.i18n.getMessage(i18n_1.MessageKeys.errorSaving)}: ${error.message}`);
        }
    }
    async _updateAutotuneSetting(key, value) {
        try {
            configManager_1.configManager.setConfigValue(`autotune.${key}`, value);
            await configManager_1.configManager.saveToFile();
            vscode.window.showInformationMessage(i18n_1.i18n.getMessage(i18n_1.MessageKeys.successfullySaved));
            this._sendCurrentSettings();
        }
        catch (error) {
            vscode.window.showErrorMessage(`${i18n_1.i18n.getMessage(i18n_1.MessageKeys.errorSaving)}: ${error.message}`);
        }
    }
    async _addAgent(name, config) {
        try {
            configManager_1.configManager.setConfigValue(`agents.${name}`, config);
            await configManager_1.configManager.saveToFile();
            vscode.window.showInformationMessage(i18n_1.i18n.getMessage(i18n_1.MessageKeys.successfullySaved));
            this._sendCurrentSettings();
        }
        catch (error) {
            vscode.window.showErrorMessage(`${i18n_1.i18n.getMessage(i18n_1.MessageKeys.errorSaving)}: ${error.message}`);
        }
    }
    async _deleteAgent(name) {
        try {
            const config = configManager_1.configManager.getConfig();
            delete config.agents[name];
            await configManager_1.configManager.saveToFile();
            vscode.window.showInformationMessage(i18n_1.i18n.getMessage(i18n_1.MessageKeys.successfullySaved));
            this._sendCurrentSettings();
        }
        catch (error) {
            vscode.window.showErrorMessage(`${i18n_1.i18n.getMessage(i18n_1.MessageKeys.errorSaving)}: ${error.message}`);
        }
    }
    async _updatePhase(name, config) {
        try {
            configManager_1.configManager.setConfigValue(`phases.${name}`, config);
            await configManager_1.configManager.saveToFile();
            vscode.window.showInformationMessage(i18n_1.i18n.getMessage(i18n_1.MessageKeys.successfullySaved));
            this._sendCurrentSettings();
        }
        catch (error) {
            vscode.window.showErrorMessage(`${i18n_1.i18n.getMessage(i18n_1.MessageKeys.errorSaving)}: ${error.message}`);
        }
    }
    async _setLanguage(language) {
        try {
            const config = vscode.workspace.getConfiguration('go-on');
            await config.update('language', language, vscode.ConfigurationTarget.Global);
            configManager_1.configManager.setConfigValue('language', language);
            await configManager_1.configManager.saveToFile();
            vscode.window.showInformationMessage(i18n_1.i18n.getMessage(i18n_1.MessageKeys.successfullySaved));
            this._sendCurrentSettings();
        }
        catch (error) {
            vscode.window.showErrorMessage(`${i18n_1.i18n.getMessage(i18n_1.MessageKeys.errorSaving)}: ${error.message}`);
        }
    }
    _sendCurrentSettings() {
        if (!this._view)
            return;
        const config = configManager_1.configManager.getConfig();
        const vsCodeConfig = vscode.workspace.getConfiguration('go-on');
        const settings = {
            language: i18n_1.i18n.getCurrentLanguage(),
            runtime: config.runtime,
            cache: config.cache,
            vector: config.vector,
            autotune: config.autotune,
            agents: config.agents,
            phases: config.phases,
            flow: config.flow,
            executablePath: vsCodeConfig.get('executablePath'),
            autoStart: vsCodeConfig.get('autoStart'),
            isRunning: this.manager.isRunning?.() || false,
        };
        this._view.webview.postMessage({
            type: 'settingsData',
            data: settings,
            translations: this._getTranslations(),
            language: i18n_1.i18n.getCurrentLanguage()
        });
    }
    _getTranslations() {
        return {
            general: {
                goOn: i18n_1.i18n.getMessage(i18n_1.MessageKeys.goOn),
                settings: i18n_1.i18n.getMessage(i18n_1.MessageKeys.settings),
                start: i18n_1.i18n.getMessage(i18n_1.MessageKeys.start),
                stop: i18n_1.i18n.getMessage(i18n_1.MessageKeys.stop),
                status: i18n_1.i18n.getMessage(i18n_1.MessageKeys.status),
                running: i18n_1.i18n.getMessage(i18n_1.MessageKeys.running),
                stopped: i18n_1.i18n.getMessage(i18n_1.MessageKeys.stopped),
            },
            runtime: {
                runtime: i18n_1.i18n.getMessage(i18n_1.MessageKeys.runtime),
                runtimeSettings: i18n_1.i18n.getMessage(i18n_1.MessageKeys.runtimeSettings),
                maintenanceInterval: i18n_1.i18n.getMessage(i18n_1.MessageKeys.maintenanceInterval),
                healthInterval: i18n_1.i18n.getMessage(i18n_1.MessageKeys.healthInterval),
                shutdownDrain: i18n_1.i18n.getMessage(i18n_1.MessageKeys.shutdownDrain),
            },
            execution: {
                executionSettings: i18n_1.i18n.getMessage(i18n_1.MessageKeys.executionSettings),
                startGoOn: i18n_1.i18n.getMessage(i18n_1.MessageKeys.startGoOn),
                stopGoOn: i18n_1.i18n.getMessage(i18n_1.MessageKeys.stopGoOn),
                healthCheck: i18n_1.i18n.getMessage(i18n_1.MessageKeys.healthCheck),
                clearCache: i18n_1.i18n.getMessage(i18n_1.MessageKeys.clearCache),
            },
            workflow: {
                workflow: i18n_1.i18n.getMessage(i18n_1.MessageKeys.workflow),
                phases: i18n_1.i18n.getMessage(i18n_1.MessageKeys.phases),
                agents: i18n_1.i18n.getMessage(i18n_1.MessageKeys.agents),
                addPhase: i18n_1.i18n.getMessage(i18n_1.MessageKeys.addPhase),
                editPhase: i18n_1.i18n.getMessage(i18n_1.MessageKeys.editPhase),
                deletePhase: i18n_1.i18n.getMessage(i18n_1.MessageKeys.deletePhase),
            },
            buttons: {
                save: i18n_1.i18n.getMessage(i18n_1.MessageKeys.save),
                cancel: i18n_1.i18n.getMessage(i18n_1.MessageKeys.cancel),
                reset: i18n_1.i18n.getMessage(i18n_1.MessageKeys.reset),
                apply: i18n_1.i18n.getMessage(i18n_1.MessageKeys.apply),
                delete: i18n_1.i18n.getMessage(i18n_1.MessageKeys.delete),
                edit: i18n_1.i18n.getMessage(i18n_1.MessageKeys.edit),
                add: i18n_1.i18n.getMessage(i18n_1.MessageKeys.add),
            },
            messages: {
                successfullySaved: i18n_1.i18n.getMessage(i18n_1.MessageKeys.successfullySaved),
                errorSaving: i18n_1.i18n.getMessage(i18n_1.MessageKeys.errorSaving),
            },
            language: {
                language: i18n_1.i18n.getMessage(i18n_1.MessageKeys.language),
                simplifiedChinese: i18n_1.i18n.getMessage(i18n_1.MessageKeys.simplifiedChinese),
                traditionalChinese: i18n_1.i18n.getMessage(i18n_1.MessageKeys.traditionalChinese),
                english: i18n_1.i18n.getMessage(i18n_1.MessageKeys.english),
            },
            credentials: {
                credentials: i18n_1.i18n.getMessage(i18n_1.MessageKeys.credentials),
                apiKey: i18n_1.i18n.getMessage(i18n_1.MessageKeys.apiKey),
                secretKey: i18n_1.i18n.getMessage(i18n_1.MessageKeys.secretKey),
            },
        };
    }
    async _handleKeyringSet(name, value) {
        try {
            await vscode.commands.executeCommand('go-on.keyringSet', { name, value });
            this._postMessage({ type: 'keyringResult', message: `Saved secret '${name}' to system keyring.` });
        }
        catch (error) {
            this._postMessage({ type: 'keyringError', message: error.message || String(error) });
        }
    }
    async _handleKeyringGet(name) {
        try {
            const value = await vscode.commands.executeCommand('go-on.keyringGet', { name });
            this._postMessage({
                type: 'keyringResult',
                message: `Fetched secret '${name}' from system keyring.`,
                value: value ?? ''
            });
        }
        catch (error) {
            this._postMessage({ type: 'keyringError', message: error.message || String(error) });
        }
    }
    async _handleKeyringDelete(name) {
        try {
            await vscode.commands.executeCommand('go-on.keyringDelete', { name });
            this._postMessage({ type: 'keyringResult', message: `Deleted secret '${name}' from system keyring.` });
        }
        catch (error) {
            this._postMessage({ type: 'keyringError', message: error.message || String(error) });
        }
    }
    async _handleKeyringList() {
        try {
            const output = await vscode.commands.executeCommand('go-on.keyringList');
            this._postMessage({
                type: 'keyringResult',
                message: 'Listed keyring secret status.',
                value: output ?? ''
            });
        }
        catch (error) {
            this._postMessage({ type: 'keyringError', message: error.message || String(error) });
        }
    }
    async _handleApplyDefaultConfigTemplate(template) {
        try {
            const configPath = await vscode.commands.executeCommand('go-on.applyDefaultConfigTemplate', { template });
            this._postMessage({
                type: 'settingsActionResult',
                message: `Applied template '${template}' to ${configPath}.`
            });
        }
        catch (error) {
            this._postMessage({ type: 'settingsActionError', message: error.message || String(error) });
        }
    }
    async _handleApplyRulesSettings(payload) {
        try {
            const rulesDir = await vscode.commands.executeCommand('go-on.updateRules', payload);
            this._postMessage({
                type: 'settingsActionResult',
                message: `Rules updated in ${rulesDir}.`
            });
        }
        catch (error) {
            this._postMessage({ type: 'settingsActionError', message: error.message || String(error) });
        }
    }
    async _handleApplyWorkflowMapping(payload) {
        try {
            const configPath = await vscode.commands.executeCommand('go-on.updateWorkflowMapping', payload);
            this._postMessage({
                type: 'settingsActionResult',
                message: `Workflow mapping saved to ${configPath}.`
            });
        }
        catch (error) {
            this._postMessage({ type: 'settingsActionError', message: error.message || String(error) });
        }
    }
    _postMessage(message) {
        this._view?.webview.postMessage(message);
    }
    _getHtmlForWebview(webview) {
        const styleResetUri = webview.asWebviewUri(vscode.Uri.joinPath(this._extensionUri, 'media', 'reset.css'));
        const styleVSCodeUri = webview.asWebviewUri(vscode.Uri.joinPath(this._extensionUri, 'media', 'vscode.css'));
        const scriptUri = webview.asWebviewUri(vscode.Uri.joinPath(this._extensionUri, 'media', 'settings.js'));
        const nonce = getNonce();
        return `<!DOCTYPE html>
            <html lang="en">
            <head>
                <meta charset="UTF-8">
                <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource} 'unsafe-inline'; script-src 'nonce-${nonce}';">
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
                                <option value="config.toml.autopilot-simple">autopilot-simple</option>
                                <option value="config.toml.autopilot-complex">autopilot-complex</option>
                                <option value="config.toml.example">example</option>
                            </select>
                        </div>
                        <div class="action-buttons">
                            <button class="action-button" id="applyDefaultTemplate">Apply As Active config.toml</button>
                        </div>
                    </div>

                    <div class="setting-group">
                        <h3>🔐 System Keyring (Preferred)</h3>
                        <div class="setting-item">
                            <label for="secretName">Secret Name:</label>
                            <select id="secretName">
                                <option value="deepseek_api_key">deepseek_api_key</option>
                                <option value="wenxin_api_key">wenxin_api_key</option>
                                <option value="wenxin_secret_key">wenxin_secret_key</option>
                                <option value="anthropic_api_key">anthropic_api_key</option>
                                <option value="doubao_api_key">doubao_api_key</option>
                                <option value="openai_compatible_api_key">openai_compatible_api_key</option>
                            </select>
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
                        <button class="action-button danger" id="clearCache">Clear Cache</button>
                    </div>

                    <div style="margin-top: 20px; padding: 10px; background: var(--vscode-textBlockQuote-background); border-left: 3px solid var(--vscode-textBlockQuote-border);">
                        <strong>Status:</strong>
                        <span class="status-indicator ${this.manager.isRunning() ? 'connected' : 'disconnected'}"></span>
                        ${this.manager.isRunning() ? 'Connected' : 'Disconnected'}
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
}
exports.GoOnSettingsViewProvider = GoOnSettingsViewProvider;
GoOnSettingsViewProvider.viewType = 'go-on-settings';
function getNonce() {
    let text = '';
    const possible = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';
    for (let i = 0; i < 32; i++) {
        text += possible.charAt(Math.floor(Math.random() * possible.length));
    }
    return text;
}
//# sourceMappingURL=settingsView.js.map