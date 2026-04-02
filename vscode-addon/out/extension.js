"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.deactivate = exports.activate = void 0;
const vscode = require("vscode");
const child_process_1 = require("child_process");
const path = require("path");
const fs = require("fs");
const fsPromises = require("fs/promises");
const https = require("https");
const os = require("os");
const tar = require("tar");
const AdmZip = require("adm-zip");
const chatView_1 = require("./chatView");
const settingsView_1 = require("./settingsView");
const statusMonitor_1 = require("./statusMonitor");
const workflowView_1 = require("./workflowView");
const processFlowView_1 = require("./processFlowView");
const advancedEdit_1 = require("./advancedEdit");
const i18n_1 = require("./i18n");
const configManager_1 = require("./configManager");
class GoOnManager {
    constructor() {
        this.process = null;
        this.requestId = 0;
        this.pendingRequests = new Map();
        this.statusItems = [];
        this.updateStatus();
    }
    async start(configPath, executablePath, cwd) {
        if (this.process) {
            throw new Error('Go-On is already running');
        }
        return new Promise((resolve, reject) => {
            this.process = (0, child_process_1.spawn)(executablePath, ['--config', configPath, '--verbose'], {
                cwd,
                stdio: ['pipe', 'pipe', 'pipe']
            });
            let startupTimeout = setTimeout(() => {
                reject(new Error('Go-On startup timeout'));
            }, 10000);
            this.process.stdout?.on('data', (data) => {
                const output = data.toString();
                console.log(`Go-On stdout: ${output}`);
                // Try to parse JSON-RPC responses
                try {
                    const lines = output.trim().split('\n');
                    for (const line of lines) {
                        if (line.trim()) {
                            const response = JSON.parse(line);
                            const pending = this.pendingRequests.get(response.id);
                            if (pending) {
                                this.pendingRequests.delete(response.id);
                                if (response.error) {
                                    pending.reject(new Error(response.error.message));
                                }
                                else {
                                    pending.resolve(response.result);
                                }
                            }
                        }
                    }
                }
                catch (e) {
                    // Not a JSON-RPC response, just log
                }
                // Clear startup timeout on first output
                if (startupTimeout) {
                    clearTimeout(startupTimeout);
                    startupTimeout = undefined;
                    resolve();
                }
            });
            this.process.stderr?.on('data', (data) => {
                console.error(`Go-On stderr: ${data}`);
            });
            this.process.on('close', (code) => {
                console.log(`Go-On process exited with code ${code}`);
                this.process = null;
                this.updateStatus();
            });
            this.process.on('error', (error) => {
                console.error(`Go-On process error: ${error}`);
                this.process = null;
                reject(error);
            });
        });
    }
    stop() {
        if (this.process) {
            this.process.kill();
            this.process = null;
        }
        this.updateStatus();
    }
    isRunning() {
        return this.process !== null;
    }
    async sendRequest(method, params) {
        if (!this.process) {
            throw new Error('Go-On is not running');
        }
        const id = ++this.requestId;
        const request = {
            jsonrpc: '2.0',
            id,
            method,
            params
        };
        return new Promise((resolve, reject) => {
            this.pendingRequests.set(id, { resolve, reject });
            const requestStr = JSON.stringify(request) + '\n';
            this.process.stdin.write(requestStr);
            // Timeout after 30 seconds
            setTimeout(() => {
                if (this.pendingRequests.has(id)) {
                    this.pendingRequests.delete(id);
                    reject(new Error('Request timeout'));
                }
            }, 30000);
        });
    }
    updateStatus() {
        this.statusItems = [
            new vscode.TreeItem(`Status: ${this.isRunning() ? 'Running' : 'Stopped'}`, vscode.TreeItemCollapsibleState.None)
        ];
        // Refresh the tree view
        vscode.commands.executeCommand('go-on-status.refresh');
        // Notify status monitor
        vscode.commands.executeCommand('go-on.refreshStatusMonitor');
    }
    getStatusItems() {
        return this.statusItems;
    }
}
class GoOnStatusProvider {
    constructor(manager) {
        this.manager = manager;
        this._onDidChangeTreeData = new vscode.EventEmitter();
        this.onDidChangeTreeData = this._onDidChangeTreeData.event;
    }
    refresh() {
        this._onDidChangeTreeData.fire();
    }
    getTreeItem(element) {
        return element;
    }
    getChildren(element) {
        if (!element) {
            return Promise.resolve(this.manager.getStatusItems());
        }
        return Promise.resolve([]);
    }
}
function platformAssetInfo() {
    switch (os.platform()) {
        case 'darwin':
            return { assetName: 'go-on-macos.tar.gz', executableName: 'go-on' };
        case 'linux':
            return { assetName: 'go-on-linux.tar.gz', executableName: 'go-on' };
        case 'win32':
            return { assetName: 'go-on-windows.zip', executableName: 'go-on.exe' };
        default:
            throw new Error(`Unsupported platform: ${os.platform()}`);
    }
}
async function pathExists(filePath) {
    try {
        await fsPromises.access(filePath, fs.constants.F_OK);
        return true;
    }
    catch {
        return false;
    }
}
function buildReleaseAssetUrl(repository, tag, assetName) {
    if (tag === 'latest') {
        return `https://github.com/${repository}/releases/latest/download/${assetName}`;
    }
    return `https://github.com/${repository}/releases/download/${tag}/${assetName}`;
}
async function downloadFile(url, destinationPath) {
    await fsPromises.mkdir(path.dirname(destinationPath), { recursive: true });
    await new Promise((resolve, reject) => {
        const request = https.get(url, (response) => {
            const statusCode = response.statusCode ?? 0;
            if (statusCode >= 300 && statusCode < 400 && response.headers.location) {
                response.resume();
                downloadFile(response.headers.location, destinationPath).then(resolve).catch(reject);
                return;
            }
            if (statusCode < 200 || statusCode >= 300) {
                response.resume();
                reject(new Error(`Download failed with HTTP ${statusCode}`));
                return;
            }
            const fileStream = fs.createWriteStream(destinationPath);
            response.pipe(fileStream);
            fileStream.on('finish', () => {
                fileStream.close();
                resolve();
            });
            fileStream.on('error', reject);
        });
        request.on('error', reject);
    });
}
async function extractArchive(archivePath, destinationDir) {
    if (archivePath.endsWith('.tar.gz')) {
        await tar.x({
            file: archivePath,
            cwd: destinationDir,
            strip: 1
        });
        return;
    }
    if (archivePath.endsWith('.zip')) {
        const zip = new AdmZip(archivePath);
        zip.extractAllTo(destinationDir, true);
        return;
    }
    throw new Error(`Unsupported archive format: ${archivePath}`);
}
async function resolveConfigPath(workspaceRoot, configuredConfigPath, runtimeDir) {
    const workspaceConfigPath = path.resolve(workspaceRoot, configuredConfigPath);
    if (await pathExists(workspaceConfigPath)) {
        return workspaceConfigPath;
    }
    const bundledConfigPath = path.join(runtimeDir, 'config.toml');
    if (await pathExists(bundledConfigPath)) {
        return bundledConfigPath;
    }
    throw new Error(`Config not found. Checked workspace path '${workspaceConfigPath}' and bundled path '${bundledConfigPath}'.`);
}
async function ensureGoOnBinary(workspaceRoot, config, context) {
    const configuredExecutablePath = config.get('executablePath', './target/release/go-on');
    if (workspaceRoot) {
        const resolvedWorkspaceExecutable = path.isAbsolute(configuredExecutablePath)
            ? configuredExecutablePath
            : path.resolve(workspaceRoot, configuredExecutablePath);
        if (await pathExists(resolvedWorkspaceExecutable)) {
            return {
                executablePath: resolvedWorkspaceExecutable,
                runtimeDir: path.dirname(resolvedWorkspaceExecutable)
            };
        }
    }
    else if (path.isAbsolute(configuredExecutablePath) && await pathExists(configuredExecutablePath)) {
        return {
            executablePath: configuredExecutablePath,
            runtimeDir: path.dirname(configuredExecutablePath)
        };
    }
    const autoDownloadEnabled = config.get('autoDownloadBinary', true);
    if (!autoDownloadEnabled) {
        throw new Error(`Configured executable does not exist: ${configuredExecutablePath}. Enable go-on.autoDownloadBinary or set go-on.executablePath.`);
    }
    const { assetName, executableName } = platformAssetInfo();
    const releaseRepository = config.get('releaseRepository', 'mikewolfli/go-on');
    const releaseTag = config.get('releaseTag', 'latest');
    const runtimeDir = path.join(context.globalStorageUri.fsPath, 'runtime');
    const executablePath = path.join(runtimeDir, executableName);
    if (await pathExists(executablePath)) {
        return { executablePath, runtimeDir };
    }
    await fsPromises.mkdir(runtimeDir, { recursive: true });
    const archivePath = path.join(context.globalStorageUri.fsPath, assetName);
    const downloadUrl = buildReleaseAssetUrl(releaseRepository, releaseTag, assetName);
    await downloadFile(downloadUrl, archivePath);
    await extractArchive(archivePath, runtimeDir);
    if (os.platform() !== 'win32') {
        await fsPromises.chmod(executablePath, 0o755);
    }
    if (!(await pathExists(executablePath))) {
        throw new Error(`Downloaded archive did not contain executable: ${executableName}`);
    }
    return { executablePath, runtimeDir };
}
async function runGoOnSecretCommand(context, action, secretName, secretValue) {
    const config = vscode.workspace.getConfiguration('go-on');
    const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    const runtime = await ensureGoOnBinary(workspaceRoot, config, context);
    const args = ['--secret', action];
    if (secretName) {
        args.push('--secret-name', secretName);
    }
    if (secretValue !== undefined) {
        args.push('--secret-value', secretValue);
    }
    return new Promise((resolve, reject) => {
        const proc = (0, child_process_1.spawn)(runtime.executablePath, args, {
            cwd: workspaceRoot || runtime.runtimeDir,
            stdio: ['ignore', 'pipe', 'pipe']
        });
        let stdout = '';
        let stderr = '';
        proc.stdout?.on('data', (chunk) => {
            stdout += chunk.toString();
        });
        proc.stderr?.on('data', (chunk) => {
            stderr += chunk.toString();
        });
        proc.on('error', reject);
        proc.on('close', (code) => {
            if (code === 0) {
                resolve(stdout.trim());
                return;
            }
            const details = (stderr || stdout || `exit code ${code}`).trim();
            reject(new Error(`go-on secret command failed: ${details}`));
        });
    });
}
function escapeRegex(value) {
    return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}
function formatTomlStringList(items) {
    return `[${items.map((item) => `"${item}"`).join(', ')}]`;
}
function formatTomlMultilineStringList(items) {
    const lines = items.map((item) => `    "${item.replace(/"/g, '\\"')}"`);
    return `[
${lines.join(',\n')}
]`;
}
function upsertSectionLine(section, lineRegex, line) {
    if (lineRegex.test(section)) {
        return section.replace(lineRegex, line);
    }
    const lines = section.split('\n');
    lines.splice(1, 0, line);
    return lines.join('\n');
}
function upsertTopLevelString(content, key, value) {
    const regex = new RegExp(`^${escapeRegex(key)}\\s*=\\s*".*"\\s*$`, 'm');
    const replacement = `${key} = "${value}"`;
    if (regex.test(content)) {
        return content.replace(regex, replacement);
    }
    return `${replacement}\n${content}`;
}
function upsertFlowPhases(content, phases) {
    const flowSectionRegex = /^\[flow\][\s\S]*?(?=^\[[^\]]+\]|\Z)/m;
    const phasesLine = `phases = ${formatTomlStringList(phases)}`;
    if (flowSectionRegex.test(content)) {
        return content.replace(flowSectionRegex, (section) => {
            const phasesRegex = /^phases\s*=\s*\[[^\]]*\]\s*$/m;
            if (phasesRegex.test(section)) {
                return section.replace(phasesRegex, phasesLine);
            }
            const trimmed = section.trimEnd();
            return `${trimmed}\n${phasesLine}\n`;
        });
    }
    return `${content.trimEnd()}\n\n[flow]\nname = "Configured Flow"\n${phasesLine}\n`;
}
function upsertPhaseAgents(content, phase, agents) {
    const header = `[phases.${phase}]`;
    const escapedHeader = escapeRegex(header);
    const sectionRegex = new RegExp(`^${escapedHeader}[\\s\\S]*?(?=^\\[[^\\]]+\\]|\\Z)`, 'm');
    const agentsLine = `agents = ${formatTomlStringList(agents)}`;
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
function upsertPhaseFallback(content, phase, fallback) {
    const header = `[phases.${phase}]`;
    const escapedHeader = escapeRegex(header);
    const sectionRegex = new RegExp(`^${escapedHeader}[\\s\\S]*?(?=^\\[[^\\]]+\\]|\\Z)`, 'm');
    const fallbackLine = `fallback = ${fallback ? 'true' : 'false'}`;
    if (sectionRegex.test(content)) {
        return content.replace(sectionRegex, (section) => upsertSectionLine(section, /^fallback\s*=\s*(true|false)\s*$/m, fallbackLine));
    }
    return `${content.trimEnd()}\n\n${header}\ndescription = "${phase} phase"\nagents = ["copilot"]\n${fallbackLine}\n`;
}
function upsertPhasePrinciples(content, phase, principles) {
    const header = `[phases.${phase}]`;
    const escapedHeader = escapeRegex(header);
    const sectionRegex = new RegExp(`^${escapedHeader}[\\s\\S]*?(?=^\\[[^\\]]+\\]|\\Z)`, 'm');
    const principlesLine = `principles = ${formatTomlMultilineStringList(principles)}`;
    if (sectionRegex.test(content)) {
        return content.replace(sectionRegex, (section) => {
            const principlesRegex = /^principles\s*=\s*\[[\s\S]*?\]\s*$/m;
            if (principlesRegex.test(section)) {
                return section.replace(principlesRegex, principlesLine);
            }
            return upsertSectionLine(section, /^principles\s*=\s*\[[\s\S]*?\]\s*$/m, principlesLine);
        });
    }
    return `${content.trimEnd()}\n\n${header}\ndescription = "${phase} phase"\nagents = ["copilot"]\nfallback = true\n${principlesLine}\n`;
}
function upsertPhaseOptionNumber(content, phase, optionKey, value) {
    const optionHeader = `[phases.${phase}.options]`;
    const escapedOptionHeader = escapeRegex(optionHeader);
    const optionSectionRegex = new RegExp(`^${escapedOptionHeader}[\\s\\S]*?(?=^\\[[^\\]]+\\]|\\Z)`, 'm');
    const optionLine = `${optionKey} = ${value}`;
    const keyRegex = new RegExp(`^${escapeRegex(optionKey)}\\s*=\\s*\\d+\\s*$`, 'm');
    if (optionSectionRegex.test(content)) {
        return content.replace(optionSectionRegex, (section) => upsertSectionLine(section, keyRegex, optionLine));
    }
    return `${content.trimEnd()}\n\n${optionHeader}\n${optionLine}\n`;
}
async function resolveConfigFilePath(context, configuredConfigPath) {
    const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    if (!workspaceRoot) {
        throw new Error('No workspace folder open.');
    }
    const config = vscode.workspace.getConfiguration('go-on');
    const runtime = await ensureGoOnBinary(workspaceRoot, config, context);
    const settingPath = configuredConfigPath || config.get('configPath', './config.toml');
    const configPath = await resolveConfigPath(workspaceRoot, settingPath, runtime.runtimeDir);
    return { workspaceRoot, configPath, runtimeDir: runtime.runtimeDir };
}
async function applyDefaultConfigTemplate(context, templateFile) {
    const { workspaceRoot, configPath, runtimeDir } = await resolveConfigFilePath(context);
    const candidates = [
        path.resolve(workspaceRoot, templateFile),
        path.join(runtimeDir, templateFile)
    ];
    let sourcePath;
    for (const candidate of candidates) {
        if (await pathExists(candidate)) {
            sourcePath = candidate;
            break;
        }
    }
    if (!sourcePath) {
        throw new Error(`Template not found: ${templateFile}`);
    }
    await fsPromises.copyFile(sourcePath, configPath);
    return configPath;
}
async function updateWorkflowMappingConfig(context, mapping) {
    const { configPath } = await resolveConfigFilePath(context);
    let content = await fsPromises.readFile(configPath, 'utf8');
    const phaseEntries = Object.entries(mapping.phases || {})
        .map(([phase, config]) => {
        const phaseName = phase.trim();
        const agents = (config?.agents || []).map((a) => a.trim()).filter(Boolean);
        const principles = (config?.principles || []).map((p) => p.trim()).filter(Boolean);
        return [phaseName, { ...config, agents, principles }];
    })
        .filter(([phase, config]) => phase.length > 0 && config.agents.length > 0);
    if (mapping.defaultPhase && mapping.defaultPhase.trim().length > 0) {
        content = upsertTopLevelString(content, 'default_phase', mapping.defaultPhase.trim());
    }
    if (phaseEntries.length > 0) {
        const phaseNames = phaseEntries.map(([phase]) => phase);
        content = upsertFlowPhases(content, phaseNames);
        for (const [phase, phaseConfig] of phaseEntries) {
            content = upsertPhaseAgents(content, phase, phaseConfig.agents);
            if (typeof phaseConfig.fallback === 'boolean') {
                content = upsertPhaseFallback(content, phase, phaseConfig.fallback);
            }
            if (phaseConfig.principles && phaseConfig.principles.length > 0) {
                content = upsertPhasePrinciples(content, phase, phaseConfig.principles);
            }
            const switchRules = phaseConfig.switchRules;
            if (switchRules) {
                if (typeof switchRules.circuitBreakerFailures === 'number' && switchRules.circuitBreakerFailures > 0) {
                    content = upsertPhaseOptionNumber(content, phase, 'circuit_breaker_failures', Math.floor(switchRules.circuitBreakerFailures));
                }
                if (typeof switchRules.circuitBreakerOpenSeconds === 'number' && switchRules.circuitBreakerOpenSeconds > 0) {
                    content = upsertPhaseOptionNumber(content, phase, 'circuit_breaker_open_seconds', Math.floor(switchRules.circuitBreakerOpenSeconds));
                }
            }
        }
    }
    await fsPromises.writeFile(configPath, content, 'utf8');
    return configPath;
}
async function updateRulesMarkdownFiles(context, payload) {
    const { configPath } = await resolveConfigFilePath(context);
    const configDir = path.dirname(configPath);
    const rulesDir = path.join(configDir, 'RULES');
    await fsPromises.mkdir(rulesDir, { recursive: true });
    const writeRulesFile = async (filePath, rules) => {
        const normalized = rules.map((item) => item.trim()).filter(Boolean);
        const content = normalized.length > 0
            ? normalized.map((item) => `- ${item}`).join('\n') + '\n'
            : '# Empty rules\n';
        await fsPromises.writeFile(filePath, content, 'utf8');
    };
    if (payload.globalRules) {
        await writeRulesFile(path.join(rulesDir, 'global.md'), payload.globalRules);
    }
    if (payload.commonRules) {
        await writeRulesFile(path.join(rulesDir, 'common.md'), payload.commonRules);
    }
    if (payload.phaseRules) {
        for (const [phase, rules] of Object.entries(payload.phaseRules)) {
            const phaseName = phase.trim();
            if (!phaseName) {
                continue;
            }
            await writeRulesFile(path.join(rulesDir, `${phaseName}.md`), rules || []);
        }
    }
    return rulesDir;
}
let goOnManager;
let statusProvider;
function activate(context) {
    console.log('Go-On extension is now active!');
    // Initialize i18n system
    const currentLanguage = i18n_1.i18n.getCurrentLanguage();
    console.log(`Go-On UI Language: ${currentLanguage}`);
    // Initialize config manager
    const config = vscode.workspace.getConfiguration('go-on');
    const configPath = config.get('configPath', './config.toml');
    configManager_1.configManager.initialize(configPath).catch(err => {
        console.warn('Failed to initialize config manager:', err);
    });
    // Sync VS Code language to app configuration
    syncLanguageToApp(context, currentLanguage);
    goOnManager = new GoOnManager();
    statusProvider = new GoOnStatusProvider(goOnManager);
    // Initialize status monitor
    const statusMonitor = new statusMonitor_1.StatusMonitor(goOnManager);
    context.subscriptions.push(statusMonitor);
    // Initialize advanced edit provider
    const advancedEditProvider = new advancedEdit_1.GoOnAdvancedEditProvider(goOnManager, context);
    // Register chat participant
    const chatParticipant = vscode.chat.createChatParticipant('go-on.chat', async (request, context, response, token) => {
        try {
            const result = await goOnManager.sendRequest('chat', {
                messages: [{ role: 'user', content: request.prompt }]
            });
            response.markdown(result.response || JSON.stringify(result, null, 2));
        }
        catch (error) {
            response.markdown(`Error: ${error.message}`);
        }
    });
    chatParticipant.iconPath = vscode.Uri.joinPath(context.extensionUri, 'media', 'robot.svg');
    // Register webview providers
    const chatProvider = new chatView_1.GoOnChatViewProvider(context.extensionUri, goOnManager, context);
    const settingsProvider = new settingsView_1.GoOnSettingsViewProvider(context.extensionUri, goOnManager, context);
    const workflowProvider = new workflowView_1.GoOnWorkflowViewProvider(context.extensionUri, goOnManager, context);
    const processFlowProvider = new processFlowView_1.GoOnProcessFlowViewProvider(context.extensionUri, goOnManager, context);
    context.subscriptions.push(vscode.window.registerWebviewViewProvider(chatView_1.GoOnChatViewProvider.viewType, chatProvider), vscode.window.registerWebviewViewProvider(settingsView_1.GoOnSettingsViewProvider.viewType, settingsProvider), vscode.window.registerWebviewViewProvider(workflowView_1.GoOnWorkflowViewProvider.viewType, workflowProvider), vscode.window.registerWebviewViewProvider(processFlowView_1.GoOnProcessFlowViewProvider.viewType, processFlowProvider));
    vscode.window.registerTreeDataProvider('go-on-status', statusProvider);
    // Command to start Go-On proxy
    let startCommand = vscode.commands.registerCommand('go-on.start', async () => {
        const config = vscode.workspace.getConfiguration('go-on');
        const configuredConfigPath = config.get('configPath', './config.toml');
        const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
        if (!workspaceFolder) {
            vscode.window.showErrorMessage('No workspace folder open.');
            return;
        }
        try {
            const runtime = await ensureGoOnBinary(workspaceFolder.uri.fsPath, config, context);
            const fullConfigPath = await resolveConfigPath(workspaceFolder.uri.fsPath, configuredConfigPath, runtime.runtimeDir);
            await goOnManager.start(fullConfigPath, runtime.executablePath, workspaceFolder.uri.fsPath);
            vscode.window.showInformationMessage('Go-On proxy started.');
        }
        catch (error) {
            vscode.window.showErrorMessage(`Failed to start Go-On: ${error.message}`);
        }
    });
    // Command to stop Go-On proxy
    let stopCommand = vscode.commands.registerCommand('go-on.stop', () => {
        goOnManager.stop();
        vscode.window.showInformationMessage('Go-On proxy stopped.');
    });
    // Command to send chat request
    let sendRequestCommand = vscode.commands.registerCommand('go-on.sendRequest', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        const message = await vscode.window.showInputBox({
            prompt: 'Enter your message',
            placeHolder: 'Type your chat message here...'
        });
        if (!message)
            return;
        try {
            const result = await goOnManager.sendRequest('chat', {
                messages: [{ role: 'user', content: message }]
            });
            vscode.window.showInformationMessage(`Response: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`Request failed: ${error.message}`);
        }
    });
    // Health check command
    let healthCheckCommand = vscode.commands.registerCommand('go-on.healthCheck', async () => {
        try {
            const result = await goOnManager.sendRequest('runtime.health');
            vscode.window.showInformationMessage(`Health: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`Health check failed: ${error.message}`);
        }
    });
    // Breaker status command
    let breakerStatusCommand = vscode.commands.registerCommand('go-on.breakerStatus', async () => {
        try {
            const result = await goOnManager.sendRequest('breaker.status');
            vscode.window.showInformationMessage(`Breaker Status: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`Breaker status check failed: ${error.message}`);
        }
    });
    // Cache clear command
    let cacheClearCommand = vscode.commands.registerCommand('go-on.cacheClear', async () => {
        try {
            await goOnManager.sendRequest('cache.clear');
            vscode.window.showInformationMessage('Cache cleared.');
        }
        catch (error) {
            vscode.window.showErrorMessage(`Cache clear failed: ${error.message}`);
        }
    });
    // Vector clear command
    let vectorClearCommand = vscode.commands.registerCommand('go-on.vectorClear', async () => {
        try {
            await goOnManager.sendRequest('vector.clear');
            vscode.window.showInformationMessage('Vector memory cleared.');
        }
        catch (error) {
            vscode.window.showErrorMessage(`Vector clear failed: ${error.message}`);
        }
    });
    // Config reload command
    let configReloadCommand = vscode.commands.registerCommand('go-on.configReload', async () => {
        try {
            await goOnManager.sendRequest('config.reload');
            vscode.window.showInformationMessage('Configuration reloaded.');
        }
        catch (error) {
            vscode.window.showErrorMessage(`Config reload failed: ${error.message}`);
        }
    });
    // Shutdown command
    let shutdownCommand = vscode.commands.registerCommand('go-on.shutdown', async () => {
        try {
            await goOnManager.sendRequest('shutdown');
            vscode.window.showInformationMessage('Shutdown initiated.');
        }
        catch (error) {
            vscode.window.showErrorMessage(`Shutdown failed: ${error.message}`);
        }
    });
    // Open chat command
    let openChatCommand = vscode.commands.registerCommand('go-on.openChat', () => {
        vscode.commands.executeCommand('workbench.view.extension.go-on');
    });
    // Open settings command
    let openSettingsCommand = vscode.commands.registerCommand('go-on.openSettings', () => {
        vscode.commands.executeCommand('workbench.view.extension.go-on');
    });
    // Clear chat command
    let clearChatCommand = vscode.commands.registerCommand('go-on.clearChat', () => {
        // This will be handled by the chat view
        vscode.window.showInformationMessage('Clear chat command executed');
    });
    // Export chat command
    let exportChatCommand = vscode.commands.registerCommand('go-on.exportChat', () => {
        // This will be handled by the chat view
        vscode.window.showInformationMessage('Export chat command executed');
    });
    // Create workflow command
    let createWorkflowCommand = vscode.commands.registerCommand('go-on.createWorkflow', () => {
        vscode.commands.executeCommand('workbench.view.extension.go-on');
        // The workflow creation will be handled by the workflow view
    });
    // Run workflow command
    let runWorkflowCommand = vscode.commands.registerCommand('go-on.runWorkflow', () => {
        vscode.window.showInformationMessage('Select a workflow to run from the Workflow panel');
    });
    // Show process flow command
    let showProcessFlowCommand = vscode.commands.registerCommand('go-on.showProcessFlow', () => {
        vscode.commands.executeCommand('workbench.view.extension.go-on');
    });
    // New session command
    let newSessionCommand = vscode.commands.registerCommand('go-on.newSession', () => {
        vscode.window.showInputBox({
            prompt: 'Enter a name for the new chat session',
            placeHolder: 'My Session'
        }).then(sessionName => {
            if (sessionName) {
                chatProvider.createNewSession(sessionName);
            }
        });
    });
    // Switch session command
    let switchSessionCommand = vscode.commands.registerCommand('go-on.switchSession', () => {
        // For now, show available sessions (this would be enhanced with a quick pick)
        vscode.window.showQuickPick(['default', 'session1', 'session2'], {
            placeHolder: 'Select a chat session to switch to'
        }).then(session => {
            if (session) {
                chatProvider.switchSession(session);
            }
        });
    });
    // Refresh status monitor command
    let refreshStatusMonitorCommand = vscode.commands.registerCommand('go-on.refreshStatusMonitor', () => {
        statusMonitor.refresh();
    });
    let keyringSetCommand = vscode.commands.registerCommand('go-on.keyringSet', async (payload) => {
        const name = payload?.name;
        const value = payload?.value;
        if (!name || value === undefined) {
            throw new Error('keyring set requires name and value');
        }
        await runGoOnSecretCommand(context, 'set', name, value);
    });
    let keyringGetCommand = vscode.commands.registerCommand('go-on.keyringGet', async (payload) => {
        const name = payload?.name;
        if (!name) {
            throw new Error('keyring get requires name');
        }
        return await runGoOnSecretCommand(context, 'get', name);
    });
    let keyringDeleteCommand = vscode.commands.registerCommand('go-on.keyringDelete', async (payload) => {
        const name = payload?.name;
        if (!name) {
            throw new Error('keyring delete requires name');
        }
        await runGoOnSecretCommand(context, 'delete', name);
    });
    let keyringListCommand = vscode.commands.registerCommand('go-on.keyringList', async () => {
        return await runGoOnSecretCommand(context, 'list');
    });
    let applyDefaultConfigCommand = vscode.commands.registerCommand('go-on.applyDefaultConfigTemplate', async (payload) => {
        const template = payload?.template;
        if (!template) {
            throw new Error('template is required');
        }
        const configPath = await applyDefaultConfigTemplate(context, template);
        return configPath;
    });
    let updateWorkflowMappingCommand = vscode.commands.registerCommand('go-on.updateWorkflowMapping', async (payload) => {
        if (!payload) {
            throw new Error('workflow mapping payload is required');
        }
        return await updateWorkflowMappingConfig(context, payload);
    });
    let updateRulesCommand = vscode.commands.registerCommand('go-on.updateRules', async (payload) => {
        if (!payload) {
            throw new Error('rules payload is required');
        }
        return await updateRulesMarkdownFiles(context, payload);
    });
    // Auto-start if configured
    ensureGoOnBinary(vscode.workspace.workspaceFolders?.[0]?.uri.fsPath, config, context)
        .then(() => {
        console.log('Go-On runtime is ready.');
    })
        .catch((error) => {
        console.warn(`Go-On runtime check failed: ${error.message}`);
    });
    if (config.get('autoStart', false)) {
        vscode.commands.executeCommand('go-on.start');
    }
    context.subscriptions.push(startCommand, stopCommand, sendRequestCommand, healthCheckCommand, breakerStatusCommand, cacheClearCommand, vectorClearCommand, configReloadCommand, shutdownCommand, openChatCommand, openSettingsCommand, clearChatCommand, exportChatCommand, newSessionCommand, switchSessionCommand, createWorkflowCommand, runWorkflowCommand, showProcessFlowCommand, refreshStatusMonitorCommand, keyringSetCommand, keyringGetCommand, keyringDeleteCommand, keyringListCommand, applyDefaultConfigCommand, updateWorkflowMappingCommand, updateRulesCommand);
}
exports.activate = activate;
function deactivate() {
    goOnManager.stop();
}
exports.deactivate = deactivate;
/**
 * Sync VS Code language with app configuration
 * This ensures the app uses the same language as VS Code
 */
async function syncLanguageToApp(context, language) {
    try {
        const config = vscode.workspace.getConfiguration('go-on');
        const configuredConfigPath = config.get('configPath', './config.toml');
        const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
        if (!workspaceFolder) {
            return;
        }
        // Create a language configuration object for the app
        const languageConfig = {
            ui_language: language,
            sync_vscode_language: true,
        };
        // Store language preference in app settings
        await config.update('language', language, vscode.ConfigurationTarget.Global);
        // Log successful sync
        console.log(`Language synchronized: VS Code ${language} -> App ${language}`);
    }
    catch (error) {
        console.warn('Failed to sync language:', error);
    }
}
//# sourceMappingURL=extension.js.map