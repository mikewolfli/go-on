"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.GoOnSettingsViewProvider = void 0;
const vscode = require("vscode");
const fs = require("fs/promises");
const path = require("path");
const i18n_1 = require("./i18n");
const configManager_1 = require("./configManager");
const protocolContract_1 = require("./protocolContract");
function escapeRegex(value) {
    return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}
function inferEnvVar(providerName) {
    return `${providerName.trim().toUpperCase().replace(/[-\s]+/g, '_')}_API_KEY`;
}
function parseSimpleTomlValue(raw) {
    const value = raw.trim();
    if (value.startsWith('"') && value.endsWith('"')) {
        return value.slice(1, -1);
    }
    if (value === 'true') {
        return true;
    }
    if (value === 'false') {
        return false;
    }
    const maybeNumber = Number(value);
    if (!Number.isNaN(maybeNumber)) {
        return maybeNumber;
    }
    return value;
}
function parseProviderCatalog(content) {
    const providers = [];
    let current = null;
    for (const line of content.split('\n')) {
        const trimmed = line.trim();
        if (!trimmed || trimmed.startsWith('#')) {
            continue;
        }
        if (trimmed === '[[providers]]') {
            if (current) {
                const candidate = current;
                if (candidate.name && candidate.type) {
                    providers.push(candidate);
                }
            }
            current = {};
            continue;
        }
        if (!current) {
            continue;
        }
        const match = trimmed.match(/^([A-Za-z0-9_]+)\s*=\s*(.+)$/);
        if (!match) {
            continue;
        }
        const key = match[1];
        current[key] = parseSimpleTomlValue(match[2]);
    }
    if (current) {
        const candidate = current;
        if (candidate.name && candidate.type) {
            providers.push(candidate);
        }
    }
    return providers;
}
function parseConfiguredAgents(content) {
    const result = new Map();
    const sectionRegex = /^\[agents\.([^\]]+)\]([\s\S]*?)(?=^\[[^\]]+\]|$)/gm;
    for (const match of content.matchAll(sectionRegex)) {
        const name = String(match[1] || '').trim();
        const section = String(match[2] || '');
        const model = section.match(/^model\s*=\s*"([^"]*)"\s*$/m)?.[1];
        const apiKeyEnv = section.match(/^api_key_env\s*=\s*"([^"]*)"\s*$/m)?.[1];
        const secretKeyEnv = section.match(/^secret_key_env\s*=\s*"([^"]*)"\s*$/m)?.[1];
        result.set(name, {
            model,
            envVar: apiKeyEnv || secretKeyEnv,
        });
    }
    return result;
}
function formatTomlValue(value) {
    if (typeof value === 'string') {
        return `"${value.replace(/"/g, '\\"')}"`;
    }
    if (typeof value === 'boolean' || typeof value === 'number') {
        return String(value);
    }
    return `"${String(value)}"`;
}
function upsertSectionLine(section, key, value) {
    const keyRegex = new RegExp(`^${escapeRegex(key)}\\s*=.*$`, 'm');
    const line = `${key} = ${formatTomlValue(value)}`;
    if (keyRegex.test(section)) {
        return section.replace(keyRegex, line);
    }
    const lines = section.split('\n');
    lines.splice(1, 0, line);
    return lines.join('\n');
}
function removeSectionLine(section, key) {
    const keyRegex = new RegExp(`^${escapeRegex(key)}\\s*=.*(?:\\r?\\n)?`, 'm');
    return section.replace(keyRegex, '');
}
function upsertAgentSection(content, providerName, fields) {
    const header = `[agents.${providerName}]`;
    const sectionRegex = new RegExp(`^${escapeRegex(header)}[\\s\\S]*?(?=^\\[[^\\]]+\\]|\\Z)`, 'm');
    const applyFields = (section) => {
        let updated = section;
        for (const [key, value] of Object.entries(fields)) {
            if (value === undefined || value === null || value === '') {
                updated = removeSectionLine(updated, key);
            }
            else {
                updated = upsertSectionLine(updated, key, value);
            }
        }
        return updated;
    };
    if (sectionRegex.test(content)) {
        return content.replace(sectionRegex, (section) => applyFields(section));
    }
    let section = `${header}\n`;
    for (const [key, value] of Object.entries(fields)) {
        if (value !== undefined && value !== null && value !== '') {
            section += `${key} = ${formatTomlValue(value)}\n`;
        }
    }
    return `${content.trimEnd()}\n\n${section}`;
}
function upsertPhaseAgents(content, phase, agents) {
    const header = `[phases.${phase}]`;
    const sectionRegex = new RegExp(`^${escapeRegex(header)}[\\s\\S]*?(?=^\\[[^\\]]+\\]|\\Z)`, 'm');
    const agentsLine = `agents = [${agents.map((agent) => `"${agent}"`).join(', ')}]`;
    if (sectionRegex.test(content)) {
        return content.replace(sectionRegex, (section) => {
            const agentsRegex = /^agents\s*=\s*\[[^\]]*\]\s*$/m;
            if (agentsRegex.test(section)) {
                return section.replace(agentsRegex, agentsLine);
            }
            const lines = section.split('\n');
            lines.splice(1, 0, agentsLine);
            return lines.join('\n');
        });
    }
    return `${content.trimEnd()}\n\n${header}\ndescription = "${phase} phase"\n${agentsLine}\nfallback = true\n`;
}
class GoOnSettingsViewProvider {
    constructor(_extensionUri, _manager, _context) {
        this._extensionUri = _extensionUri;
        this._runtimeFeatures = {};
        this._commandMessageMap = {
            startGoOn: 'go-on.start',
            stopGoOn: 'go-on.stop',
            healthCheck: 'go-on.healthCheck',
            healthProbes: 'go-on.healthProbes',
            clearCache: 'go-on.cacheClear',
            breakerStatus: 'go-on.breakerStatus',
            breakerRecovery: 'go-on.breakerRecovery',
            observabilityAlerts: 'go-on.observabilityAlerts',
            securityBaseline: 'go-on.securityBaseline',
            harnessStatus: 'go-on.harnessStatus',
            clearVector: 'go-on.vectorClear',
            reloadConfig: 'go-on.configReload',
            workflowExecute: 'go-on.workflowExecute',
            taskPlan: 'go-on.taskPlan',
            taskExecute: 'go-on.taskExecute',
            learningSummary: 'go-on.learningSummary',
            learningGuardrail: 'go-on.learningGuardrail',
            learningReplay: 'go-on.learningReplay',
            knowledgeDistill: 'go-on.knowledgeDistill',
            rlAlignmentEval: 'go-on.rlAlignmentEval',
            hardnessStatus: 'go-on.hardnessStatus',
            costStatus: 'go-on.costStatus',
            configBaseline: 'go-on.configBaseline',
            errorContract: 'go-on.errorContract',
            buildRepro: 'go-on.buildRepro',
            dataLifecycle: 'go-on.dataLifecycle',
            optimizationPeak: 'go-on.optimizationPeak',
            releaseReadiness: 'go-on.releaseReadiness',
            runtimeStability: 'go-on.runtimeStability',
            autotuneStatus: 'go-on.autotuneStatus',
            governanceStatus: 'go-on.governanceStatus',
            governancePlanGet: 'go-on.governancePlanGet',
            governanceAuditRecent: 'go-on.governanceAuditRecent',
            lockStatus: 'go-on.lockStatus',
            debugPanelGet: 'go-on.debugPanelGet',
            actionCheck: 'go-on.actionCheck',
        };
        this.manager = _manager;
        this.context = _context;
        this.context.subscriptions.push(new vscode.Disposable(() => this._messageSubscription?.dispose()));
    }
    resolveWebviewView(webviewView, _context, _token) {
        this._view = webviewView;
        webviewView.webview.options = {
            enableScripts: true,
            localResourceRoots: [this._extensionUri]
        };
        webviewView.webview.html = this._getHtmlForWebview(webviewView.webview);
        this._messageSubscription?.dispose();
        this._messageSubscription = webviewView.webview.onDidReceiveMessage(async (message) => {
            try {
                await this._handleWebviewMessage(message);
            }
            catch (error) {
                this._postMessage({
                    type: 'settingsActionError',
                    message: this._getErrorMessage(error),
                });
            }
        }, undefined);
        this._sendCurrentSettings();
        if (this.manager.isRunning?.()) {
            this._refreshRuntimeFeatures().catch(() => undefined);
        }
    }
    async _refreshRuntimeFeatures() {
        try {
            const response = await this.manager.sendRequest('runtime.features', {});
            const features = (typeof response === 'object' && response !== null ? response['features'] : undefined);
            if (features && typeof features === 'object') {
                this._runtimeFeatures = features;
                this._view?.webview.postMessage({ type: 'runtimeFeatures', features: this._runtimeFeatures });
            }
        }
        catch {
            // not fatal — keep previous features
        }
    }
    _getErrorMessage(error) {
        return error instanceof Error ? error.message : String(error);
    }
    async _handleWebviewMessage(message) {
        const messageType = String(message.type ?? '');
        const handlers = {
            requestSettings: async (_message) => this._sendCurrentSettings(),
            openConfigWizard: async () => this.showConfigWizard(),
            updateSetting: async (msg) => this._handleGenericSettingUpdate(String(msg.key ?? ''), msg.value),
            updateRuntimeSetting: async (msg) => this._updateRuntimeSetting(String(msg.key ?? ''), msg.value),
            updateCacheSetting: async (msg) => this._updateCacheSetting(String(msg.key ?? ''), msg.value),
            updateVectorSetting: async (msg) => this._updateVectorSetting(String(msg.key ?? ''), msg.value),
            updateAutotuneSetting: async (msg) => this._updateAutotuneSetting(String(msg.key ?? ''), msg.value),
            requestProviderModels: async (msg) => this._sendProviderModels(String(msg.provider ?? '')),
            saveProviderSelection: async (msg) => this._saveProviderSelection(String(msg.provider ?? ''), String(msg.model ?? 'auto'), msg.envVar ? String(msg.envVar) : undefined),
            addAgent: async (msg) => this._addAgent(String(msg.name ?? ''), msg.config),
            deleteAgent: async (msg) => this._deleteAgent(String(msg.name ?? '')),
            updatePhase: async (msg) => this._updatePhase(String(msg.name ?? ''), msg.config),
            setLanguage: async (msg) => this._setLanguage(String(msg.language ?? '')),
            setKeyringSecret: async (msg) => this._handleKeyringSet(String(msg.name ?? ''), String(msg.value ?? '')),
            getKeyringSecret: async (msg) => this._handleKeyringGet(String(msg.name ?? '')),
            deleteKeyringSecret: async (msg) => this._handleKeyringDelete(String(msg.name ?? '')),
            listKeyringSecrets: async () => this._handleKeyringList(),
            applyDefaultConfigTemplate: async (msg) => this._handleApplyDefaultConfigTemplate(String(msg.template ?? '')),
            applyRulesSettings: async (msg) => this._handleApplyRulesSettings(msg.payload || {}),
            applyWorkflowMapping: async (msg) => this._handleApplyWorkflowMapping(msg.payload || {}),
        };
        const messageHandler = handlers[messageType];
        if (messageHandler) {
            await messageHandler(message);
            return;
        }
        const command = this._commandMessageMap[messageType];
        if (command) {
            await vscode.commands.executeCommand(command);
            return;
        }
    }
    async _handleGenericSettingUpdate(key, value) {
        if (!key.startsWith('go-on.')) {
            return;
        }
        const goOnConfig = vscode.workspace.getConfiguration('go-on');
        const relativeKey = key.replace(/^go-on\./, '');
        await goOnConfig.update(relativeKey, value, vscode.ConfigurationTarget.Workspace);
        if (relativeKey.startsWith('runtime.')) {
            await this._updateRuntimeSetting(relativeKey.replace(/^runtime\./, ''), value);
            return;
        }
        if (relativeKey.startsWith('cache.')) {
            await this._updateCacheSetting(relativeKey.replace(/^cache\./, ''), value);
            return;
        }
        if (relativeKey.startsWith('vector.')) {
            await this._updateVectorSetting(relativeKey.replace(/^vector\./, ''), value);
            return;
        }
        if (relativeKey.startsWith('autotune.')) {
            await this._updateAutotuneSetting(relativeKey.replace(/^autotune\./, ''), value);
            return;
        }
        vscode.window.showInformationMessage(i18n_1.i18n.getMessage(i18n_1.MessageKeys.successfullySaved));
    }
    _workspaceRoot() {
        const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
        if (!root) {
            throw new Error('No workspace folder open.');
        }
        return root;
    }
    _resolveConfigPath() {
        const root = this._workspaceRoot();
        const configured = vscode.workspace.getConfiguration('go-on').get('configPath', './config.toml') || './config.toml';
        return path.isAbsolute(configured) ? configured : path.resolve(root, configured);
    }
    async _loadProviderCatalog() {
        const root = this._workspaceRoot();
        const candidates = [
            path.join(root, 'providers.toml'),
            path.resolve(root, '..', 'providers.toml'),
        ];
        for (const filePath of candidates) {
            try {
                const content = await fs.readFile(filePath, 'utf8');
                const parsed = parseProviderCatalog(content);
                if (parsed.length > 0) {
                    return parsed;
                }
            }
            catch {
                // Try next candidate.
            }
        }
        return [];
    }
    async _loadConfiguredAgentMap() {
        try {
            const configPath = this._resolveConfigPath();
            const content = await fs.readFile(configPath, 'utf8');
            return parseConfiguredAgents(content);
        }
        catch {
            return new Map();
        }
    }
    _modelIdFromRuntime(value) {
        if (typeof value === 'string' && value.trim()) {
            return value.trim();
        }
        if (typeof value !== 'object' || value === null) {
            return undefined;
        }
        const record = value;
        const candidates = [record.id, record.model_id, record.modelId, record.name];
        for (const candidate of candidates) {
            if (typeof candidate === 'string' && candidate.trim()) {
                return candidate.trim();
            }
        }
        return undefined;
    }
    async _resolveProviderModels(providerName, spec) {
        const modelSet = new Set();
        modelSet.add('auto');
        if (spec?.defaultModel) {
            modelSet.add(spec.defaultModel);
        }
        if (this.manager.isRunning()) {
            try {
                const response = await this.manager.sendRequest('models/list', {});
                const payload = (typeof response === 'object' && response !== null ? response : {});
                const groups = Array.isArray(payload.models) ? payload.models : [];
                const matched = groups.find((group) => {
                    const record = typeof group === 'object' && group !== null ? group : {};
                    return record.agent === providerName;
                });
                if (matched && typeof matched === 'object') {
                    const record = matched;
                    const defaultModel = this._modelIdFromRuntime(record.default_model);
                    if (defaultModel) {
                        modelSet.add(defaultModel);
                    }
                    const runtimeModels = Array.isArray(record.models) ? record.models : [];
                    for (const runtimeModel of runtimeModels) {
                        const modelId = this._modelIdFromRuntime(runtimeModel);
                        if (modelId) {
                            modelSet.add(modelId);
                        }
                    }
                }
            }
            catch {
                // Keep catalog-only models when runtime endpoint is unavailable.
            }
        }
        return Array.from(modelSet.values());
    }
    async _buildProviderSettingsPayload() {
        const catalog = await this._loadProviderCatalog();
        const configured = await this._loadConfiguredAgentMap();
        const providers = catalog
            .map((spec) => {
            const configuredValue = configured.get(spec.name);
            return {
                name: spec.name,
                agentType: spec.type,
                defaultModel: spec.model,
                apiKeyEnv: spec.api_key_env,
                secretKeyEnv: spec.secret_key_env,
                url: spec.url,
                chatPath: spec.chat_path,
                supportsSystem: spec.supports_system,
                configuredModel: configuredValue?.model,
                configuredEnvVar: configuredValue?.envVar,
            };
        })
            .sort((a, b) => a.name.localeCompare(b.name));
        const selectedProvider = providers.find((item) => item.configuredModel || item.configuredEnvVar)?.name
            || providers[0]?.name
            || 'copilot';
        const selectedSpec = providers.find((item) => item.name === selectedProvider);
        const selectedModel = selectedSpec?.configuredModel || selectedSpec?.defaultModel || 'auto';
        const selectedEnvVar = selectedSpec?.configuredEnvVar || selectedSpec?.apiKeyEnv || inferEnvVar(selectedProvider);
        const modelOptions = await this._resolveProviderModels(selectedProvider, selectedSpec);
        return {
            providers,
            selectedProvider,
            selectedModel,
            selectedEnvVar,
            modelOptions,
        };
    }
    async _sendProviderModels(providerName) {
        if (!providerName.trim()) {
            return;
        }
        const payload = await this._buildProviderSettingsPayload();
        const selectedSpec = payload.providers.find((item) => item.name === providerName);
        const modelOptions = await this._resolveProviderModels(providerName, selectedSpec);
        this._postMessage({
            type: 'providerModelsData',
            provider: providerName,
            modelOptions,
            selectedModel: selectedSpec?.configuredModel || selectedSpec?.defaultModel || 'auto',
            selectedEnvVar: selectedSpec?.configuredEnvVar || selectedSpec?.apiKeyEnv || inferEnvVar(providerName),
        });
    }
    async _saveProviderSelection(providerName, modelName, envVar) {
        const provider = providerName.trim();
        if (!provider) {
            throw new Error('Provider cannot be empty.');
        }
        const catalog = await this._loadProviderCatalog();
        const matched = catalog.find((item) => item.name === provider);
        if (!matched) {
            throw new Error(`Unknown provider: ${provider}`);
        }
        const normalizedModel = modelName.trim() || 'auto';
        const normalizedEnvVar = (envVar || '').trim() || matched.api_key_env || inferEnvVar(provider);
        const configPath = this._resolveConfigPath();
        let content = '';
        try {
            content = await fs.readFile(configPath, 'utf8');
        }
        catch {
            content = '';
        }
        content = upsertAgentSection(content, provider, {
            type: matched.type,
            url: matched.url,
            chat_path: matched.chat_path,
            api_key_env: normalizedEnvVar,
            secret_key_env: matched.secret_key_env,
            anthropic_version: matched.anthropic_version,
            model: normalizedModel,
            max_tokens: matched.max_tokens,
            supports_system: matched.supports_system,
        });
        const defaultPhase = content.match(/^default_phase\s*=\s*"([^"]+)"\s*$/m)?.[1];
        if (defaultPhase) {
            content = upsertPhaseAgents(content, defaultPhase, [provider]);
        }
        await fs.writeFile(configPath, `${content.trimEnd()}\n`, 'utf8');
        this._postMessage({
            type: 'settingsActionResult',
            message: `Saved provider=${provider}, model=${normalizedModel} to ${configPath}`,
        });
        await this._sendCurrentSettings();
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
            vscode.window.showErrorMessage(`${i18n_1.i18n.getMessage(i18n_1.MessageKeys.errorSaving)}: ${this._getErrorMessage(error)}`);
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
            vscode.window.showErrorMessage(`${i18n_1.i18n.getMessage(i18n_1.MessageKeys.errorSaving)}: ${this._getErrorMessage(error)}`);
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
            vscode.window.showErrorMessage(`${i18n_1.i18n.getMessage(i18n_1.MessageKeys.errorSaving)}: ${this._getErrorMessage(error)}`);
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
            vscode.window.showErrorMessage(`${i18n_1.i18n.getMessage(i18n_1.MessageKeys.errorSaving)}: ${this._getErrorMessage(error)}`);
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
            vscode.window.showErrorMessage(`${i18n_1.i18n.getMessage(i18n_1.MessageKeys.errorSaving)}: ${this._getErrorMessage(error)}`);
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
            vscode.window.showErrorMessage(`${i18n_1.i18n.getMessage(i18n_1.MessageKeys.errorSaving)}: ${this._getErrorMessage(error)}`);
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
            vscode.window.showErrorMessage(`${i18n_1.i18n.getMessage(i18n_1.MessageKeys.errorSaving)}: ${this._getErrorMessage(error)}`);
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
            vscode.window.showErrorMessage(`${i18n_1.i18n.getMessage(i18n_1.MessageKeys.errorSaving)}: ${this._getErrorMessage(error)}`);
        }
    }
    async _sendCurrentSettings() {
        if (!this._view)
            return;
        const config = configManager_1.configManager.getConfig();
        const vsCodeConfig = vscode.workspace.getConfiguration('go-on');
        let providerSettings = {
            providers: [],
            selectedProvider: 'copilot',
            selectedModel: 'auto',
            selectedEnvVar: inferEnvVar('copilot'),
            modelOptions: ['auto'],
        };
        try {
            providerSettings = await this._buildProviderSettingsPayload();
        }
        catch {
            // Keep fallback provider payload if catalog/config discovery fails.
        }
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
            providerSettings,
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
            this._postMessage({ type: 'keyringError', message: this._getErrorMessage(error) });
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
            this._postMessage({ type: 'keyringError', message: this._getErrorMessage(error) });
        }
    }
    async _handleKeyringDelete(name) {
        try {
            await vscode.commands.executeCommand('go-on.keyringDelete', { name });
            this._postMessage({ type: 'keyringResult', message: `Deleted secret '${name}' from system keyring.` });
        }
        catch (error) {
            this._postMessage({ type: 'keyringError', message: this._getErrorMessage(error) });
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
            this._postMessage({ type: 'keyringError', message: this._getErrorMessage(error) });
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
            this._postMessage({ type: 'settingsActionError', message: this._getErrorMessage(error) });
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
            this._postMessage({ type: 'settingsActionError', message: this._getErrorMessage(error) });
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
            this._postMessage({ type: 'settingsActionError', message: this._getErrorMessage(error) });
        }
    }
    _postMessage(message) {
        this._view?.webview.postMessage(message);
    }
    async showConfigWizard() {
        const panel = vscode.window.createWebviewPanel('goOnConfigWizard', i18n_1.i18n.getMessage('configuration.wizard.title'), vscode.ViewColumn.One, { enableScripts: true });
        panel.webview.html = this._getConfigWizardHtml(panel.webview);
        panel.webview.onDidReceiveMessage(async (message) => {
            const command = String(message.command ?? '');
            if (command === 'cancel') {
                panel.dispose();
                return;
            }
            if (command !== 'saveConfig') {
                return;
            }
            const payload = (message.config ?? {});
            const goOnConfig = vscode.workspace.getConfiguration('go-on');
            const rawProtocolMode = String(payload.protocolMode ?? 'from_config');
            const protocolMode = rawProtocolMode === 'from_config'
                ? 'from_config'
                : (0, protocolContract_1.normalizeProtocolMode)(rawProtocolMode);
            await Promise.all([
                goOnConfig.update('configPath', String(payload.configPath ?? './config.toml'), vscode.ConfigurationTarget.Workspace),
                goOnConfig.update('executablePath', String(payload.executablePath ?? ''), vscode.ConfigurationTarget.Workspace),
                goOnConfig.update('autoStart', Boolean(payload.autoStart), vscode.ConfigurationTarget.Workspace),
                goOnConfig.update('runtime.protocolMode', protocolMode, vscode.ConfigurationTarget.Workspace),
            ]);
            configManager_1.configManager.setConfigValue('runtime.protocolMode', protocolMode);
            await configManager_1.configManager.saveToFile();
            await this._sendCurrentSettings();
            vscode.window.showInformationMessage(i18n_1.i18n.getMessage('messages.successfullySaved'));
            panel.dispose();
        });
    }
    _getConfigWizardHtml(webview) {
        const nonce = getNonce();
        const config = vscode.workspace.getConfiguration('go-on');
        const configPath = String(config.get('configPath', './config.toml'));
        const executablePath = String(config.get('executablePath', ''));
        const autoStart = Boolean(config.get('autoStart', false));
        const protocolMode = String(config.get('runtime.protocolMode', 'from_config'));
        const payload = JSON.stringify({ configPath, executablePath, autoStart, protocolMode })
            .replace(/</g, '\\u003c');
        return `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource} 'unsafe-inline'; script-src 'nonce-${nonce}';">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>${i18n_1.i18n.getMessage('configuration.wizard.title')}</title>
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
        <div class="title">${i18n_1.i18n.getMessage('configuration.wizard.title')}</div>
        <div class="subtitle">${i18n_1.i18n.getMessage('configuration.wizard.subtitle')}</div>
    </div>
    <div class="steps">
        <div class="step active" data-step-indicator="0">${i18n_1.i18n.getMessage('configuration.wizard.step1')}</div>
        <div class="step" data-step-indicator="1">${i18n_1.i18n.getMessage('configuration.wizard.step2')}</div>
        <div class="step" data-step-indicator="2">${i18n_1.i18n.getMessage('configuration.wizard.step3')}</div>
    </div>
    <div id="step0">
        <div class="cards">
            <div class="card selected" data-scenario="local">
                <div class="card-title">${i18n_1.i18n.getMessage('configuration.wizard.localTitle')}</div>
                <div class="card-desc">${i18n_1.i18n.getMessage('configuration.wizard.localDesc')}</div>
            </div>
            <div class="card" data-scenario="shared">
                <div class="card-title">${i18n_1.i18n.getMessage('configuration.wizard.sharedTitle')}</div>
                <div class="card-desc">${i18n_1.i18n.getMessage('configuration.wizard.sharedDesc')}</div>
            </div>
            <div class="card" data-scenario="editor">
                <div class="card-title">${i18n_1.i18n.getMessage('configuration.wizard.editorTitle')}</div>
                <div class="card-desc">${i18n_1.i18n.getMessage('configuration.wizard.editorDesc')}</div>
            </div>
        </div>
    </div>
    <div id="step1" hidden>
        <div class="modes">
            <div class="card" data-mode="from_config"><div class="card-title">from_config</div><div class="card-desc">Follow project config.toml</div></div>
            <div class="card selected" data-mode="adaptive"><div class="card-title">adaptive</div><div class="card-desc">${i18n_1.i18n.getMessage('configuration.wizard.adaptiveDesc')}</div><span class="recommended">${i18n_1.i18n.getMessage('configuration.wizard.recommended')}</span></div>
            <div class="card" data-mode="acp_stdio"><div class="card-title">acp_stdio</div><div class="card-desc">${i18n_1.i18n.getMessage('configuration.wizard.acpStdioDesc')}</div></div>
            <div class="card" data-mode="acp_http"><div class="card-title">acp_http</div><div class="card-desc">${i18n_1.i18n.getMessage('configuration.wizard.acpHttpDesc')}</div></div>
            <div class="card" data-mode="mcp_stdio"><div class="card-title">mcp_stdio</div><div class="card-desc">${i18n_1.i18n.getMessage('configuration.wizard.mcpStdioDesc')}</div></div>
            <div class="card" data-mode="mcp_http"><div class="card-title">mcp_http</div><div class="card-desc">${i18n_1.i18n.getMessage('configuration.wizard.mcpHttpDesc')}</div></div>
        </div>
    </div>
    <div id="step2" hidden>
        <div class="review">
            <div class="review-item"><div class="review-label">${i18n_1.i18n.getMessage('configuration.configPath')}</div><div class="review-value" id="review-config-path"></div></div>
            <div class="review-item"><div class="review-label">${i18n_1.i18n.getMessage('configuration.executablePath')}</div><div class="review-value" id="review-executable-path"></div></div>
            <div class="review-item"><div class="review-label">${i18n_1.i18n.getMessage('configuration.autoStart')}</div><div class="review-value" id="review-auto-start"></div></div>
            <div class="review-item"><div class="review-label">${i18n_1.i18n.getMessage('configuration.wizard.protocolMode')}</div><div class="review-value" id="review-protocol-mode"></div></div>
        </div>
    </div>
    <div class="actions">
        <button class="ghost" id="cancel-btn">${i18n_1.i18n.getMessage(i18n_1.MessageKeys.cancel)}</button>
        <div>
            <button class="ghost" id="prev-btn" disabled>${i18n_1.i18n.getMessage('configuration.wizard.previous')}</button>
            <button class="primary" id="next-btn">${i18n_1.i18n.getMessage('configuration.wizard.next')}</button>
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
            document.getElementById('next-btn').textContent = state.step === 2 ? '${i18n_1.i18n.getMessage(i18n_1.MessageKeys.save)}' : '${i18n_1.i18n.getMessage('configuration.wizard.next')}';
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