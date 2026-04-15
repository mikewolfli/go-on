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
const protocolContract_1 = require("./protocolContract");
class GoOnManager {
    classifyRpcErrorKind(message, data) {
        const explicit = typeof data?.kind === 'string' ? data.kind : undefined;
        if (explicit && explicit.trim().length > 0) {
            return explicit;
        }
        const lower = String(message || '').toLowerCase();
        if (lower.includes('pua'))
            return 'PuaViolation';
        if (lower.includes('budget'))
            return 'BudgetExceeded';
        if (lower.includes('hardening policy denied') || lower.includes('sandbox')) {
            return 'SandboxBlocked';
        }
        return 'GeneralError';
    }
    formatRpcError(error) {
        const kind = this.classifyRpcErrorKind(error.message, error.data);
        const context = typeof error.data?.detail === 'string' &&
            error.data.detail.includes(protocolContract_1.protocolContract.errors.requestErrorContextPrefix)
            ? protocolContract_1.protocolContract.errors.requestErrorContextPrefix
            : 'none';
        return `rpc_error:${error.code}:${kind}:${error.message} (context=${context})`;
    }
    constructor() {
        this.process = null;
        this.requestId = 0;
        this.pendingRequests = new Map();
        this.statusItems = [];
        this.runtimeEnvOverrides = {};
        this.lastWizardPromptAt = 0;
        this.updateStatus();
    }
    async start(configPath, executablePath, cwd, protocolMode) {
        if (this.process) {
            throw new Error('Go-On is already running');
        }
        return new Promise((resolve, reject) => {
            let resolved = false;
            let stderrBuffer = '';
            const args = ['--config', configPath, '--verbose'];
            if (protocolMode && protocolMode !== 'from_config') {
                args.push('--protocol-mode', protocolMode);
            }
            this.process = (0, child_process_1.spawn)(executablePath, args, {
                cwd,
                env: {
                    ...process.env,
                    ...this.runtimeEnvOverrides
                },
                stdio: ['pipe', 'pipe', 'pipe']
            });
            let startupTimeout = setTimeout(() => {
                this.process?.kill();
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
                                    pending.reject(new Error(this.formatRpcError(response.error)));
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
                    resolved = true;
                    resolve();
                }
            });
            this.process.stderr?.on('data', (data) => {
                const text = data.toString();
                stderrBuffer += text;
                if (stderrBuffer.length > 4000) {
                    stderrBuffer = stderrBuffer.slice(-4000);
                }
                console.error(`Go-On stderr: ${text}`);
            });
            this.process.on('close', (code) => {
                console.log(`Go-On process exited with code ${code}`);
                const failedBeforeStartup = !resolved;
                this.process = null;
                this.updateStatus();
                if (startupTimeout) {
                    clearTimeout(startupTimeout);
                    startupTimeout = undefined;
                }
                if (failedBeforeStartup) {
                    const details = stderrBuffer.trim();
                    reject(new Error(`Go-On exited before startup (code ${code}). ${details || 'No stderr output.'}`));
                }
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
    setRuntimeEnvOverrides(overrides) {
        this.runtimeEnvOverrides = {
            ...this.runtimeEnvOverrides,
            ...overrides
        };
    }
    async sendRequest(method, params, options) {
        if (!this.process) {
            throw new Error('Go-On is not running');
        }
        if (!options?.skipProviderGuard && this.requiresAiProvider(method)) {
            const ready = await this.isAnyAiProviderReady();
            if (!ready) {
                await this.notifyAndOpenSetupWizard();
                throw new Error(`${protocolContract_1.protocolContract.errors.providerNotReady} ${protocolContract_1.protocolContract.errors.setupWizardOpened}`);
            }
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
    requiresAiProvider(method) {
        return new Set([
            'chat',
            'workflow.execute',
            'task.plan',
            'task.execute',
            'learning.summary',
            'primary_secondary.summary'
        ]).has(method);
    }
    async isAnyAiProviderReady() {
        const now = Date.now();
        if (this.providerReadyCache && now - this.providerReadyCache.checkedAt < 5000) {
            return this.providerReadyCache.ready;
        }
        try {
            const report = await this.sendRequest('runtime.health', undefined, {
                skipProviderGuard: true,
            });
            const components = Array.isArray(report?.components)
                ? report.components
                : Array.isArray(report?.report?.components)
                    ? report.report.components
                    : [];
            const providerComponent = components.find((component) => component?.name === 'provider_dependencies');
            if (!providerComponent) {
                this.providerReadyCache = { checkedAt: now, ready: true };
                return true;
            }
            const ready = Number(providerComponent?.details?.ready ?? 0);
            const total = Number(providerComponent?.details?.total ?? 0);
            const isReady = total > 0 && ready > 0;
            this.providerReadyCache = { checkedAt: now, ready: isReady };
            return isReady;
        }
        catch {
            // Do not hard-block on guard probe failures.
            this.providerReadyCache = { checkedAt: now, ready: true };
            return true;
        }
    }
    async notifyAndOpenSetupWizard() {
        const now = Date.now();
        if (now - this.lastWizardPromptAt < 5000) {
            return;
        }
        this.lastWizardPromptAt = now;
        await vscode.window.showWarningMessage(protocolContract_1.protocolContract.errors.setupWizardPrompt);
        await vscode.commands.executeCommand('go-on.openSettings');
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
function isSupportedExecutablePath(filePath) {
    const ext = path.extname(filePath).toLowerCase();
    return ext === '.exe' || ext === '.bat' || ext === '.sh';
}
async function openExecutablePathSettings() {
    await vscode.commands.executeCommand('go-on.openSettings');
    await vscode.commands.executeCommand('workbench.action.openSettings', '@ext:go-on-vscode go-on.executablePath');
}
async function promptForManualBinaryPath(config, workspaceRoot, reason) {
    const selectOption = 'Select Local Binary';
    const openSettingsOption = 'Open Go-On Settings';
    const cancelOption = 'Cancel';
    const choice = await vscode.window.showErrorMessage(`Failed to download Go-On runtime: ${reason}`, selectOption, openSettingsOption, cancelOption);
    if (choice === openSettingsOption) {
        await openExecutablePathSettings();
        throw new Error('Runtime download failed. Set go-on.executablePath and try again.');
    }
    if (choice !== selectOption) {
        throw new Error('Runtime download was canceled. You can set go-on.executablePath in settings.');
    }
    const fileSelection = await vscode.window.showOpenDialog({
        canSelectFiles: true,
        canSelectFolders: false,
        canSelectMany: false,
        title: 'Select Go-On executable',
        openLabel: 'Use This Binary'
    });
    if (!fileSelection || fileSelection.length === 0) {
        await openExecutablePathSettings();
        throw new Error('No local binary selected. Set go-on.executablePath in settings and try again.');
    }
    const selectedPath = fileSelection[0].fsPath;
    if (!(await pathExists(selectedPath))) {
        await openExecutablePathSettings();
        throw new Error(`Selected executable does not exist: ${selectedPath}`);
    }
    if (!isSupportedExecutablePath(selectedPath)) {
        await openExecutablePathSettings();
        throw new Error(`Selected file is not supported: ${selectedPath}. Please select an .exe, .bat, or .sh file.`);
    }
    if (os.platform() !== 'win32') {
        try {
            await fsPromises.chmod(selectedPath, 0o755);
        }
        catch {
            // Ignore chmod failures for user-managed binaries.
        }
    }
    await config.update('executablePath', selectedPath, workspaceRoot ? vscode.ConfigurationTarget.Workspace : vscode.ConfigurationTarget.Global);
    vscode.window.showInformationMessage(`Using local Go-On binary: ${selectedPath}`);
    return {
        executablePath: selectedPath,
        runtimeDir: path.dirname(selectedPath)
    };
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
async function downloadFile(url, destinationPath, maxRedirects = 5) {
    if (maxRedirects <= 0) {
        throw new Error('Too many redirects while downloading file');
    }
    await fsPromises.mkdir(path.dirname(destinationPath), { recursive: true });
    await new Promise((resolve, reject) => {
        const request = https.get(url, (response) => {
            const statusCode = response.statusCode ?? 0;
            if (statusCode >= 300 && statusCode < 400 && response.headers.location) {
                response.resume();
                downloadFile(response.headers.location, destinationPath, maxRedirects - 1).then(resolve).catch(reject);
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
        await ensureProvidersTomlForConfig(workspaceRoot, runtimeDir, workspaceConfigPath);
        return workspaceConfigPath;
    }
    const bundledConfigPath = path.join(runtimeDir, 'config.toml');
    if (await pathExists(bundledConfigPath)) {
        return bundledConfigPath;
    }
    const workspaceConfigTemplatePath = path.join(workspaceRoot, 'config.toml.autopilot-adaptive');
    if (await pathExists(workspaceConfigTemplatePath)) {
        await fsPromises.mkdir(path.dirname(workspaceConfigPath), { recursive: true });
        await fsPromises.copyFile(workspaceConfigTemplatePath, workspaceConfigPath);
        await ensureProvidersTomlForConfig(workspaceRoot, runtimeDir, workspaceConfigPath);
        vscode.window.showInformationMessage(`Go-On config created from workspace template: ${workspaceConfigPath}`);
        return workspaceConfigPath;
    }
    const bundledConfigTemplatePath = path.join(runtimeDir, 'config.toml.autopilot-adaptive');
    if (await pathExists(bundledConfigTemplatePath)) {
        await fsPromises.mkdir(path.dirname(workspaceConfigPath), { recursive: true });
        await fsPromises.copyFile(bundledConfigTemplatePath, workspaceConfigPath);
        await ensureProvidersTomlForConfig(workspaceRoot, runtimeDir, workspaceConfigPath);
        vscode.window.showInformationMessage(`Go-On config created from runtime template: ${workspaceConfigPath}`);
        return workspaceConfigPath;
    }
    throw new Error(`Config not found. Checked workspace path '${workspaceConfigPath}' and bundled path '${bundledConfigPath}'.`);
}
async function ensureProvidersTomlForConfig(workspaceRoot, runtimeDir, configPath) {
    const targetDir = path.dirname(configPath);
    const targetProvidersPath = path.join(targetDir, 'providers.toml');
    if (await pathExists(targetProvidersPath)) {
        return;
    }
    const sourceCandidates = [
        path.join(workspaceRoot, 'providers.toml'),
        path.join(runtimeDir, 'providers.toml')
    ];
    for (const candidate of sourceCandidates) {
        if (!(await pathExists(candidate))) {
            continue;
        }
        await fsPromises.mkdir(targetDir, { recursive: true });
        await fsPromises.copyFile(candidate, targetProvidersPath);
        vscode.window.showInformationMessage(`Go-On providers catalog synced: ${targetProvidersPath}`);
        return;
    }
}
async function ensureGoOnBinary(workspaceRoot, config, context) {
    const configuredExecutablePath = config.get('executablePath', './target/release/go-on');
    const ensureSupportedPath = async (resolvedPath) => {
        if (!isSupportedExecutablePath(resolvedPath)) {
            await openExecutablePathSettings();
            throw new Error(`Configured executable must be an .exe, .bat, or .sh file: ${resolvedPath}`);
        }
        return {
            executablePath: resolvedPath,
            runtimeDir: path.dirname(resolvedPath)
        };
    };
    if (workspaceRoot) {
        const resolvedWorkspaceExecutable = path.isAbsolute(configuredExecutablePath)
            ? configuredExecutablePath
            : path.resolve(workspaceRoot, configuredExecutablePath);
        if (await pathExists(resolvedWorkspaceExecutable)) {
            return await ensureSupportedPath(resolvedWorkspaceExecutable);
        }
    }
    else if (path.isAbsolute(configuredExecutablePath) && await pathExists(configuredExecutablePath)) {
        return await ensureSupportedPath(configuredExecutablePath);
    }
    const autoDownloadEnabled = config.get('autoDownloadBinary', false);
    if (!autoDownloadEnabled) {
        await openExecutablePathSettings();
        throw new Error(`Configured executable does not exist: ${configuredExecutablePath}. Set go-on.executablePath to a valid local runtime path.`);
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
    vscode.window.showInformationMessage(`Go-On runtime not found. Downloading ${assetName} from ${releaseRepository} (${releaseTag})...`);
    try {
        await downloadFile(downloadUrl, archivePath);
        await extractArchive(archivePath, runtimeDir);
    }
    catch (error) {
        return await promptForManualBinaryPath(config, workspaceRoot, error?.message || String(error));
    }
    if (os.platform() !== 'win32') {
        await fsPromises.chmod(executablePath, 0o755);
    }
    if (!(await pathExists(executablePath))) {
        throw new Error(`Downloaded archive did not contain executable: ${executableName}`);
    }
    if (!isSupportedExecutablePath(executablePath)) {
        await openExecutablePathSettings();
        throw new Error(`Resolved runtime is not supported: ${executablePath}. Expected .exe, .bat, or .sh.`);
    }
    vscode.window.showInformationMessage('Go-On runtime download complete. Chat is ready to use.');
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
    await ensureProvidersTomlForConfig(workspaceRoot, runtimeDir, configPath);
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
let runtimeReadyPromise = null;
async function executeFirstAvailableCommand(commandIds) {
    const availableCommands = await vscode.commands.getCommands(true);
    for (const commandId of commandIds) {
        if (!availableCommands.includes(commandId)) {
            continue;
        }
        await vscode.commands.executeCommand(commandId);
        return true;
    }
    return false;
}
async function revealGoOnView(target) {
    const openedContainer = await executeFirstAvailableCommand([
        'workbench.view.extension.go-on',
        'workbench.view.extension.go_on',
        'workbench.view.extension.goon'
    ]);
    const focusCommands = {
        chat: ['go-on-chat.focus', 'go_on_chat.focus'],
        settings: ['go-on-settings.focus', 'go_on_settings.focus'],
        workflow: ['go-on-workflow.focus', 'go_on_workflow.focus'],
        'process-flow': ['go-on-process-flow.focus', 'go_on_process_flow.focus']
    };
    const focused = await executeFirstAvailableCommand(focusCommands[target]);
    if (openedContainer || focused) {
        return true;
    }
    const viewIds = {
        chat: 'go-on-chat',
        settings: 'go-on-settings',
        workflow: 'go-on-workflow',
        'process-flow': 'go-on-process-flow'
    };
    try {
        await vscode.commands.executeCommand('workbench.action.openView', viewIds[target]);
        return true;
    }
    catch {
        return false;
    }
}
async function ensureRuntimeReadyAfterChatOpen(context) {
    if (runtimeReadyPromise) {
        await runtimeReadyPromise;
        return;
    }
    runtimeReadyPromise = (async () => {
        const config = vscode.workspace.getConfiguration('go-on');
        const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
        await vscode.window.withProgress({
            location: vscode.ProgressLocation.Notification,
            title: 'Preparing Go-On runtime',
            cancellable: false
        }, async () => {
            await ensureGoOnBinary(workspaceRoot, config, context);
        });
        if (config.get('autoStart', false) && !goOnManager.isRunning()) {
            await vscode.commands.executeCommand('go-on.start');
        }
    })();
    try {
        await runtimeReadyPromise;
    }
    catch (error) {
        runtimeReadyPromise = null;
        throw error;
    }
}
async function ensureGoOnStarted() {
    if (goOnManager.isRunning()) {
        return;
    }
    await vscode.commands.executeCommand('go-on.start');
    if (!goOnManager.isRunning()) {
        throw new Error('Go-On backend is still stopped after startup attempt. Check executablePath/configPath settings.');
    }
}
async function prepareRuntimeAndStartFromChat(context) {
    try {
        await ensureRuntimeReadyAfterChatOpen(context);
        await ensureGoOnStarted();
    }
    catch (error) {
        vscode.window.showWarningMessage(`Chat is open. Backend is not ready yet: ${error?.message || error}. Configure Go-On settings in the Chat/Settings view and retry.`);
    }
}
function parseMissingEnvVariableNames(errorMessage) {
    const matches = errorMessage.match(/missing required environment variables[^:]*:\s*([^\n]+)/i);
    if (!matches || matches.length < 2) {
        return [];
    }
    return matches[1]
        .split(',')
        .map((name) => name.trim())
        .filter((name) => /^[A-Z0-9_]+$/.test(name));
}
function buildPlaceholderEnvValues(envNames) {
    const values = {};
    for (const envName of envNames) {
        values[envName] = '__GO_ON_PLACEHOLDER__';
    }
    return values;
}
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
    // Register webview providers
    const chatProvider = new chatView_1.GoOnChatViewProvider(context.extensionUri, goOnManager, context, async () => {
        await prepareRuntimeAndStartFromChat(context);
    });
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
        const tryStart = async () => {
            const runtime = await ensureGoOnBinary(workspaceFolder.uri.fsPath, config, context);
            const fullConfigPath = await resolveConfigPath(workspaceFolder.uri.fsPath, configuredConfigPath, runtime.runtimeDir);
            const protocolMode = config.get('runtime.protocolMode', 'from_config');
            await goOnManager.start(fullConfigPath, runtime.executablePath, workspaceFolder.uri.fsPath, protocolMode);
        };
        try {
            await tryStart();
            vscode.window.showInformationMessage('Go-On proxy started.');
        }
        catch (error) {
            const errorMessage = String(error?.message || error);
            const missingEnvVars = parseMissingEnvVariableNames(errorMessage);
            if (missingEnvVars.length > 0) {
                try {
                    const envValues = buildPlaceholderEnvValues(missingEnvVars);
                    goOnManager.setRuntimeEnvOverrides(envValues);
                    await tryStart();
                    vscode.window.showWarningMessage('Go-On proxy started without API keys. Configure provider keys in Settings before using cloud agents.');
                    return;
                }
                catch (retryError) {
                    const retryMessage = String(retryError?.message || retryError);
                    vscode.window.showErrorMessage(`Failed to start Go-On: ${retryMessage}`);
                    throw retryError;
                }
            }
            vscode.window.showErrorMessage(`Failed to start Go-On: ${errorMessage}`);
            throw error;
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
    let healthProbesCommand = vscode.commands.registerCommand('go-on.healthProbes', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            const result = await goOnManager.sendRequest('health.probes');
            const probes = result?.probes ?? {};
            const liveness = probes?.liveness?.status ?? 'unknown';
            const readiness = probes?.readiness?.status ?? 'unknown';
            const summary = probes?.summary ?? {};
            const locks = probes?.locks ?? {};
            const timeouts = probes?.timeouts ?? {};
            vscode.window.showInformationMessage(`health.probes: liveness=${liveness}, readiness=${readiness}, lock=${String(locks.status ?? 'unknown')}, poisoned=${Number(locks.poisoned_total ?? 0)}, slow=${Number(locks.slow_wait_total ?? 0)}, timeout=${String(timeouts.status ?? 'unknown')}, agent_timeout=${Number(timeouts.agent_request_total ?? 0)}, review_timeout=${Number(timeouts.review_gate_total ?? 0)}, probe_timeout=${Number(timeouts.runtime_probe_total ?? 0)}, error=${Number(summary.error ?? 0)}, warn=${Number(summary.warn ?? 0)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`health.probes failed: ${error.message}`);
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
    let openChatCommand = vscode.commands.registerCommand('go-on.openChat', async () => {
        const config = vscode.workspace.getConfiguration('go-on');
        const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
        try {
            await ensureGoOnBinary(workspaceRoot, config, context);
        }
        catch (error) {
            await revealGoOnView('settings');
            vscode.window.showWarningMessage(`Go-On executable is not ready: ${error?.message || error}. Please set a valid .exe, .bat, or .sh path in Settings.`);
            return;
        }
        const opened = await revealGoOnView('chat');
        if (!opened) {
            vscode.window.showWarningMessage('Go-On Chat view is not available yet. Reload Window after installing/updating the extension.');
        }
        void prepareRuntimeAndStartFromChat(context);
    });
    // Close chat command and stop backend if currently running
    let closeChatCommand = vscode.commands.registerCommand('go-on.closeChat', async () => {
        // Switch away from Go-On chat view to effectively close/hide it.
        await vscode.commands.executeCommand('workbench.view.explorer');
        if (goOnManager.isRunning()) {
            goOnManager.stop();
            vscode.window.showInformationMessage('Go-On chat closed. Running backend was stopped.');
        }
        else {
            vscode.window.showInformationMessage('Go-On chat closed. Backend was already stopped.');
        }
    });
    // Open settings command
    let openSettingsCommand = vscode.commands.registerCommand('go-on.openSettings', async () => {
        const opened = await revealGoOnView('settings');
        if (!opened) {
            vscode.window.showWarningMessage('Go-On Settings view is not available yet. Reload Window after installing/updating the extension.');
        }
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
    let createWorkflowCommand = vscode.commands.registerCommand('go-on.createWorkflow', async () => {
        const opened = await revealGoOnView('workflow');
        if (!opened) {
            vscode.window.showWarningMessage('Go-On Workflow view is not available yet. Reload Window after installing/updating the extension.');
        }
    });
    // Run workflow command
    let runWorkflowCommand = vscode.commands.registerCommand('go-on.runWorkflow', () => {
        vscode.window.showInformationMessage('Select a workflow to run from the Workflow panel');
    });
    // Show process flow command
    let showProcessFlowCommand = vscode.commands.registerCommand('go-on.showProcessFlow', async () => {
        const opened = await revealGoOnView('process-flow');
        if (!opened) {
            vscode.window.showWarningMessage('Go-On Process Flow view is not available yet. Reload Window after installing/updating the extension.');
        }
    });
    let workflowExecuteRpcCommand = vscode.commands.registerCommand('go-on.workflowExecute', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        const objective = await vscode.window.showInputBox({
            prompt: 'Workflow objective',
            placeHolder: 'Describe the task objective for workflow.execute'
        });
        if (!objective) {
            return;
        }
        try {
            const result = await goOnManager.sendRequest('workflow.execute', {
                task: objective,
            });
            vscode.window.showInformationMessage(`workflow.execute completed: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`workflow.execute failed: ${error.message}`);
        }
    });
    let taskPlanRpcCommand = vscode.commands.registerCommand('go-on.taskPlan', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        const task = await vscode.window.showInputBox({
            prompt: 'Task to plan',
            placeHolder: 'Describe the task for task.plan'
        });
        if (!task) {
            return;
        }
        try {
            const result = await goOnManager.sendRequest('task.plan', { task });
            vscode.window.showInformationMessage(`task.plan completed: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`task.plan failed: ${error.message}`);
        }
    });
    let taskExecuteRpcCommand = vscode.commands.registerCommand('go-on.taskExecute', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        const task = await vscode.window.showInputBox({
            prompt: 'Task to execute',
            placeHolder: 'Describe the task for task.execute'
        });
        if (!task) {
            return;
        }
        try {
            const result = await goOnManager.sendRequest('task.execute', {
                task,
                requirement_confirmed: true,
            });
            vscode.window.showInformationMessage(`task.execute completed: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`task.execute failed: ${error.message}`);
        }
    });
    let learningSummaryRpcCommand = vscode.commands.registerCommand('go-on.learningSummary', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            const result = await goOnManager.sendRequest('learning.summary');
            vscode.window.showInformationMessage(`learning.summary: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`learning.summary failed: ${error.message}`);
        }
    });
    let learningGuardrailRpcCommand = vscode.commands.registerCommand('go-on.learningGuardrail', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            const result = await goOnManager.sendRequest('learning.guardrail', { limit: 50 });
            const guardrail = result?.guardrail ?? {};
            const stats = guardrail?.stats ?? {};
            const warnings = Array.isArray(guardrail?.warnings) ? guardrail.warnings.length : 0;
            vscode.window.showInformationMessage(`learning.guardrail: status=${String(guardrail?.status ?? 'unknown')}, samples=${Number(stats?.records_total ?? 0)}, parseable=${(Number(stats?.parseable_ratio ?? 0) * 100).toFixed(1)}%, quality=${(Number(stats?.quality_ratio ?? 0) * 100).toFixed(1)}%, high_risk=${Number(stats?.high_risk_records ?? 0)}, warnings=${warnings}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`learning.guardrail failed: ${error.message}`);
        }
    });
    let learningReplayRpcCommand = vscode.commands.registerCommand('go-on.learningReplay', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            const result = await goOnManager.sendRequest('learning.replay', { limit: 20 });
            const replay = result?.replay ?? {};
            const records = Array.isArray(replay?.records) ? replay.records.length : 0;
            const workflow = Number(replay?.workflow_events ?? 0);
            const pua = Number(replay?.pua_events ?? 0);
            const hasBus = replay?.latest_learning_bus ? 'yes' : 'no';
            vscode.window.showInformationMessage(`learning.replay: records=${records}, workflow=${workflow}, pua=${pua}, latest_bus=${hasBus}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`learning.replay failed: ${error.message}`);
        }
    });
    let knowledgeDistillRpcCommand = vscode.commands.registerCommand('go-on.knowledgeDistill', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            const result = await goOnManager.sendRequest('knowledge.distill', {
                limit: 20,
                strategy_limit: 8,
                apply_tombstone: true,
            });
            const distillation = result?.distillation ?? {};
            const layers = distillation?.layers ?? {};
            const evidence = layers?.evidence ?? {};
            const summary = layers?.summary ?? {};
            const strategy = layers?.strategy ?? {};
            const conflicts = layers?.conflicts ?? {};
            const tombstones = layers?.tombstones ?? {};
            vscode.window.showInformationMessage(`knowledge.distill: evidence=${Number(evidence?.records_total ?? 0)}, summary=${Number(summary?.sampled_events ?? 0)}, strategy=${Number(strategy?.rules_total ?? 0)}, conflicts=${Number(conflicts?.count ?? 0)}, tombstones_added=${Number(tombstones?.added_count ?? 0)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`knowledge.distill failed: ${error.message}`);
        }
    });
    let rlAlignmentEvalRpcCommand = vscode.commands.registerCommand('go-on.rlAlignmentEval', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            const result = await goOnManager.sendRequest('rl.alignment.offline_eval', { window: 120 });
            const offlineEval = result?.offline_eval ?? {};
            const decision = offlineEval?.decision ?? {};
            const comparison = offlineEval?.comparison ?? {};
            const drift = offlineEval?.drift ?? {};
            vscode.window.showInformationMessage(`rl.alignment.offline_eval: samples=${Number(offlineEval?.samples_total ?? 0)}, uplift=${Number(comparison?.reward_uplift ?? 0).toFixed(4)}, pass=${Boolean(comparison?.passes)}, drift=${Number(drift?.absolute_diff ?? 0).toFixed(4)}, alert=${Boolean(drift?.alert)}, mode=${String(decision?.recommended_mode ?? 'conservative')}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`rl.alignment.offline_eval failed: ${error.message}`);
        }
    });
    let hardnessStatusRpcCommand = vscode.commands.registerCommand('go-on.hardnessStatus', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        const task = await vscode.window.showInputBox({
            prompt: 'Task text for hardness.status',
            placeHolder: 'Describe the task to evaluate routing hardness',
            value: 'Assess multi-file routing and budget orchestration update'
        });
        if (task === undefined) {
            return;
        }
        try {
            const result = await goOnManager.sendRequest('hardness.status', {
                task,
                changed_files: ['src/acp/impl/request.rs', 'tests/acp_runtime_rpc_integration.rs'],
                tool_dependencies: ['search_files', 'read_file', 'write_file']
            });
            const hardness = result?.hardness ?? {};
            const budget = hardness?.budget ?? {};
            vscode.window.showInformationMessage(`hardness.status: level=${String(hardness?.level ?? 'unknown')}, score=${Number(hardness?.score ?? 0).toFixed(1)}, timeout=${Number(budget?.timeout_seconds ?? 0)}s, parallelism_cap=${Number(budget?.parallelism_cap ?? 1)}, mode=${String(budget?.recommended_mode ?? 'agent')}, reviews=${Number(budget?.required_reviews ?? 1)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`hardness.status failed: ${error.message}`);
        }
    });
    let costStatusRpcCommand = vscode.commands.registerCommand('go-on.costStatus', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        const task = await vscode.window.showInputBox({
            prompt: 'Task text for cost.status',
            placeHolder: 'Describe the task to evaluate token/cost governance',
            value: 'Optimize token budget and model cost routing for multi-step task'
        });
        if (task === undefined) {
            return;
        }
        try {
            const result = await goOnManager.sendRequest('cost.status', {
                task,
                changed_files: ['src/acp/impl/request.rs', 'vscode-addon/src/extension.ts'],
                tool_dependencies: ['search_files', 'read_file', 'write_file'],
                max_output_tokens: 1800
            });
            const cost = result?.cost ?? {};
            const budget = cost?.budget ?? {};
            const compression = cost?.compression ?? {};
            const routing = cost?.routing ?? {};
            const telemetry = cost?.telemetry ?? {};
            vscode.window.showInformationMessage(`cost.status: class=${String(budget?.budget_class ?? 'unknown')}, input=${Number(budget?.input_tokens_estimate ?? 0)}, output=${Number(budget?.output_tokens_budget ?? 0)}, total=${Number(budget?.total_tokens_budget ?? 0)}, compress=${Boolean(compression?.triggered)}, tier=${String(routing?.preferred_model_tier ?? 'economy')}, est_cost=${Number(telemetry?.estimated_total_cost ?? 0).toFixed(4)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`cost.status failed: ${error.message}`);
        }
    });
    let configBaselineRpcCommand = vscode.commands.registerCommand('go-on.configBaseline', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            const result = await goOnManager.sendRequest('config.baseline');
            const baseline = result?.baseline ?? {};
            const status = String(baseline?.status ?? 'unknown');
            const protocolMode = String(baseline?.effective?.protocol_mode ?? 'auto');
            const strictEnabled = baseline?.effective?.production_strict === true;
            const migration = baseline?.migration ?? {};
            const legacyCount = Number(migration?.legacy_key_count ?? 0);
            const explicitCount = Number(baseline?.file?.runtime_explicit_field_count ?? 0);
            vscode.window.showInformationMessage(`config.baseline: status=${status}, protocol=${protocolMode}, strict=${strictEnabled ? 'on' : 'off'}, runtime_explicit=${explicitCount}, legacy_keys=${legacyCount}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`config.baseline failed: ${error.message}`);
        }
    });
    let errorContractRpcCommand = vscode.commands.registerCommand('go-on.errorContract', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            const result = await goOnManager.sendRequest('error.contract');
            const contract = result?.contract ?? {};
            const version = String(contract?.version ?? 'unknown');
            const kinds = Array.isArray(contract?.kinds) ? contract.kinds : [];
            const retryableKinds = kinds.filter((item) => item?.retry?.retryable === true).length;
            vscode.window.showInformationMessage(`error.contract: version=${version}, kinds=${kinds.length}, retryable_kinds=${retryableKinds}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`error.contract failed: ${error.message}`);
        }
    });
    let buildReproRpcCommand = vscode.commands.registerCommand('go-on.buildRepro', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            const result = await goOnManager.sendRequest('build.repro');
            const build = result?.build ?? {};
            const repro = build?.reproducibility ?? {};
            const requiredPresent = Number(repro?.required_present ?? 0);
            const requiredTotal = Number(repro?.required_total ?? 0);
            const status = String(build?.status ?? 'unknown');
            const commit = String(build?.build?.git_commit_short ?? '-');
            const releaseItems = Array.isArray(build?.release_manifest?.items)
                ? build.release_manifest.items.length
                : 0;
            vscode.window.showInformationMessage(`build.repro: status=${status}, required=${requiredPresent}/${requiredTotal}, commit=${commit}, release_items=${releaseItems}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`build.repro failed: ${error.message}`);
        }
    });
    let dataLifecycleRpcCommand = vscode.commands.registerCommand('go-on.dataLifecycle', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            const result = await goOnManager.sendRequest('data.lifecycle', { execute_gc: false });
            const lifecycle = result?.lifecycle ?? {};
            const storage = lifecycle?.storage ?? {};
            const waterline = storage?.waterline ?? {};
            const status = String(waterline?.status ?? 'unknown');
            const totalBytes = Number(storage?.total_bytes ?? 0);
            const targetCount = Array.isArray(storage?.targets) ? storage.targets.length : 0;
            const alerts = Array.isArray(waterline?.alerts) ? waterline.alerts.length : 0;
            vscode.window.showInformationMessage(`data.lifecycle: status=${status}, total_bytes=${totalBytes}, targets=${targetCount}, alerts=${alerts}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`data.lifecycle failed: ${error.message}`);
        }
    });
    let optimizationPeakRpcCommand = vscode.commands.registerCommand('go-on.optimizationPeak', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            const result = await goOnManager.sendRequest('optimization.peak', {
                task: 'BLUE15 one-shot optimization peak',
                freeze_mode: 'strict'
            });
            const peak = result?.peak ?? {};
            const overallPass = peak?.overall_pass === true;
            const gates = Array.isArray(peak?.gates) ? peak.gates : [];
            const passed = gates.filter((item) => item?.passed === true).length;
            const status = String(peak?.status ?? 'unknown');
            const version = String(peak?.version ?? '-');
            vscode.window.showInformationMessage(`optimization.peak: status=${status}, overall_pass=${overallPass}, gates=${passed}/${gates.length}, version=${version}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`optimization.peak failed: ${error.message}`);
        }
    });
    let autotuneStatusRpcCommand = vscode.commands.registerCommand('go-on.autotuneStatus', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            const result = await goOnManager.sendRequest('autotune.status');
            vscode.window.showInformationMessage(`autotune.status: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`autotune.status failed: ${error.message}`);
        }
    });
    let selectorStatusRpcCommand = vscode.commands.registerCommand('go-on.selectorStatus', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            const result = await goOnManager.sendRequest('selector.status');
            const mode = result?.mode ?? 'unknown';
            const selector = result?.selector ?? {};
            const topModel = Array.isArray(selector?.models) && selector.models.length > 0
                ? selector.models[0]
                : null;
            vscode.window.showInformationMessage(`selector.status: mode=${mode}, exploration_bias=${Number(selector?.exploration_bias ?? 0).toFixed(2)}, tracked_models=${Number(selector?.tracked_models ?? 0)}, total_observations=${Number(selector?.total_observations ?? 0)}, top_model=${String(topModel?.model_id ?? '-')}, top_score=${Number(topModel?.ucb_score ?? 0).toFixed(3)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`selector.status failed: ${error.message}`);
        }
    });
    let governanceStatusRpcCommand = vscode.commands.registerCommand('go-on.governanceStatus', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            const result = await goOnManager.sendRequest('governance.status');
            const governance = result?.governance ?? {};
            const strictEnabled = governance?.config?.production_strict === true;
            const strictViolations = Number(governance?.config?.strict_violation_count ?? 0);
            const entryAuthEnabled = governance?.config?.entry_auth_enabled === true;
            const entryAuthKeyConfigured = governance?.config?.entry_auth_key_configured === true;
            const entryRateLimit = Number(governance?.config?.entry_rate_limit_rpm ?? 0);
            vscode.window.showInformationMessage(`governance=${governance?.status ?? 'unknown'}, strict=${strictEnabled ? 'on' : 'off'}, strict_violations=${strictViolations}, entry_auth=${entryAuthEnabled ? 'on' : 'off'}, entry_key=${entryAuthKeyConfigured ? 'set' : 'missing'}, entry_rpm=${entryRateLimit}, rules=${governance?.rules?.version ?? '-'}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`governance.status failed: ${error.message}`);
        }
    });
    let governancePlanGetRpcCommand = vscode.commands.registerCommand('go-on.governancePlanGet', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            const result = await goOnManager.sendRequest('governance.plan.get');
            const plan = result?.plan ?? {};
            const escalationLevel = String(plan?.escalation_level ?? 'L1');
            const redLines = Array.isArray(plan?.red_lines) ? plan.red_lines.length : 0;
            const stageReq = Array.isArray(plan?.stage_requirements) ? plan.stage_requirements.length : 0;
            const safeguards = Array.isArray(plan?.mandatory_safeguards) ? plan.mandatory_safeguards.length : 0;
            vscode.window.showInformationMessage(`governance.plan.get: escalation=${escalationLevel}, red_lines=${redLines}, stage_requirements=${stageReq}, safeguards=${safeguards}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`governance.plan.get failed: ${error.message}`);
        }
    });
    let governanceAuditRecentRpcCommand = vscode.commands.registerCommand('go-on.governanceAuditRecent', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        const limitText = await vscode.window.showInputBox({
            prompt: 'Limit for governance.audit.recent',
            placeHolder: '20',
            value: '20'
        });
        if (limitText === undefined) {
            return;
        }
        const limit = Number.parseInt(limitText, 10);
        const safeLimit = Number.isFinite(limit) && limit > 0 ? Math.min(limit, 200) : 20;
        try {
            const result = await goOnManager.sendRequest('governance.audit.recent', { limit: safeLimit });
            const events = Array.isArray(result?.audit?.events) ? result.audit.events : [];
            const latestAction = events.length > 0 ? String(events[events.length - 1]?.action ?? '-') : '-';
            vscode.window.showInformationMessage(`governance.audit.recent: events=${events.length}, latest_action=${latestAction}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`governance.audit.recent failed: ${error.message}`);
        }
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
    // autotune.get — retrieve current autotune parameters
    let autotuneGetRpcCommand = vscode.commands.registerCommand('go-on.autotuneGet', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            const result = await goOnManager.sendRequest('autotune.get');
            vscode.window.showInformationMessage(`autotune.get: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`autotune.get failed: ${error.message}`);
        }
    });
    // autotune.reset — reset autotune learning state
    let autotuneResetRpcCommand = vscode.commands.registerCommand('go-on.autotuneReset', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        const confirm = await vscode.window.showWarningMessage('Reset autotune state? This will clear learned parameters.', 'Reset', 'Cancel');
        if (confirm !== 'Reset') {
            return;
        }
        try {
            const result = await goOnManager.sendRequest('autotune.reset', {});
            vscode.window.showInformationMessage(`autotune.reset: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`autotune.reset failed: ${error.message}`);
        }
    });
    // metrics.get — retrieve runtime metrics snapshot
    let metricsGetRpcCommand = vscode.commands.registerCommand('go-on.metricsGet', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            const result = await goOnManager.sendRequest('metrics.get');
            const metrics = result;
            vscode.window.showInformationMessage(`metrics: chat=${Number(metrics?.chat_requests_total ?? 0)}, failed=${Number(metrics?.failed_requests ?? 0)}, agent_timeout=${Number(metrics?.agent_timeout_failures_total ?? 0)}, review_timeout=${Number(metrics?.review_gate_timeout_total ?? 0)}, probe_timeout=${Number(metrics?.runtime_probe_timeout_total ?? 0)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`metrics.get failed: ${error.message}`);
        }
    });
    // metrics.reset — reset runtime metric counters
    let metricsResetRpcCommand = vscode.commands.registerCommand('go-on.metricsReset', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        const confirm = await vscode.window.showWarningMessage('Reset all runtime metric counters?', 'Reset', 'Cancel');
        if (confirm !== 'Reset') {
            return;
        }
        try {
            await goOnManager.sendRequest('metrics.reset');
            vscode.window.showInformationMessage('Metrics reset.');
        }
        catch (error) {
            vscode.window.showErrorMessage(`metrics.reset failed: ${error.message}`);
        }
    });
    // trace.metrics — aggregated trace timing metrics
    let traceMetricsRpcCommand = vscode.commands.registerCommand('go-on.traceMetrics', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            const result = await goOnManager.sendRequest('trace.metrics');
            const trace = result;
            const timeouts = trace?.timeouts ?? {};
            vscode.window.showInformationMessage(`trace.metrics: buffered=${Number(trace?.buffered_events ?? 0)}, slow_top_n=${Array.isArray(trace?.slow_requests_top_n) ? trace.slow_requests_top_n.length : 0}, agent_timeout=${Number(timeouts?.agent_request_total ?? 0)}, review_timeout=${Number(timeouts?.review_gate_total ?? 0)}, probe_timeout=${Number(timeouts?.runtime_probe_total ?? 0)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`trace.metrics failed: ${error.message}`);
        }
    });
    let qualityBaselineRpcCommand = vscode.commands.registerCommand('go-on.qualityBaseline', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            const healthResult = await goOnManager.sendRequest('runtime.health');
            const metricsResult = await goOnManager.sendRequest('metrics.get');
            const traceResult = await goOnManager.sendRequest('trace.metrics');
            const lifecycle = healthResult?.lifecycle ?? {};
            const metrics = metricsResult;
            const trace = traceResult;
            const timeouts = trace?.timeouts ?? {};
            const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
            let scenarioCount = 0;
            if (workspaceRoot) {
                const requestsDir = path.join(workspaceRoot, 'requests');
                if (fs.existsSync(requestsDir)) {
                    scenarioCount = fs
                        .readdirSync(requestsDir)
                        .filter((name) => name.toLowerCase().endsWith('.ndjson')).length;
                }
            }
            vscode.window.showInformationMessage(`quality.baseline: healthy=${Boolean(lifecycle?.is_healthy)}, total=${Number(metrics?.total_requests ?? 0)}, success=${Number(metrics?.successful_requests ?? 0)}, failed=${Number(metrics?.failed_requests ?? 0)}, avg_ms=${Number(metrics?.avg_request_duration_ms ?? 0).toFixed(1)}, buffered=${Number(trace?.buffered_events ?? 0)}, scenarios=${scenarioCount}, agent_timeout=${Number(timeouts?.agent_request_total ?? 0)}, review_timeout=${Number(timeouts?.review_gate_total ?? 0)}, probe_timeout=${Number(timeouts?.runtime_probe_total ?? 0)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`quality.baseline failed: ${error.message}`);
        }
    });
    // runtime.stability — check runtime stability baseline
    let runtimeStabilityRpcCommand = vscode.commands.registerCommand('go-on.runtimeStability', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            const result = await goOnManager.sendRequest('runtime.stability');
            const stability = result?.stability ?? {};
            const checks = stability?.checks ?? [];
            const summary = stability?.summary ?? {};
            const checkSummary = checks.map((check) => `${check.name}=${check.status}`).join(', ');
            vscode.window.showInformationMessage(`runtime.stability: score=${Number(stability?.score ?? 0)}, level=${stability?.level ?? 'unknown'}, safe_restart=${Boolean(stability?.safe_restart_ready)}, health_errors=${Number(summary?.health_errors ?? 0)}, health_warnings=${Number(summary?.health_warnings ?? 0)}, config_warnings=${Number(summary?.config_warnings ?? 0)}, strict_violations=${Number(summary?.strict_violations ?? 0)}, checks=[${checkSummary}]`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`runtime.stability failed: ${error.message}`);
        }
    });
    let harnessStatusRpcCommand = vscode.commands.registerCommand('go-on.harnessStatus', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            const result = await goOnManager.sendRequest('harness.status', { seed: 20260415 });
            const harness = result?.harness ?? {};
            const suites = harness?.suites ?? {};
            vscode.window.showInformationMessage(`harness.status: total=${Number(harness?.scenario_total ?? 0)}, smoke=${Number(suites?.smoke?.count ?? 0)}, regression=${Number(suites?.regression?.count ?? 0)}, adversarial=${Number(suites?.adversarial?.count ?? 0)}, long_chain=${Number(suites?.long_chain?.count ?? 0)}, seed=${Number(harness?.fixed_seed ?? 0)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`harness.status failed: ${error.message}`);
        }
    });
    // trace.get — fetch recent trace events
    let traceGetRpcCommand = vscode.commands.registerCommand('go-on.traceGet', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            const result = await goOnManager.sendRequest('trace.get', {});
            vscode.window.showInformationMessage(`trace.get: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`trace.get failed: ${error.message}`);
        }
    });
    // observability.alerts — aggregated runtime alerts
    let observabilityAlertsRpcCommand = vscode.commands.registerCommand('go-on.observabilityAlerts', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            const result = await goOnManager.sendRequest('observability.alerts', { limit: 20 });
            const alerts = result?.alerts ?? {};
            const critical = Number(alerts?.critical ?? 0);
            const warn = Number(alerts?.warn ?? 0);
            const info = Number(alerts?.info ?? 0);
            const topCode = Array.isArray(alerts?.items) && alerts.items.length > 0
                ? String(alerts.items[0]?.code ?? '-')
                : '-';
            vscode.window.showInformationMessage(`observability.alerts: critical=${critical}, warn=${warn}, info=${info}, top=${topCode}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`observability.alerts failed: ${error.message}`);
        }
    });
    // security.baseline — production security readiness summary
    let securityBaselineRpcCommand = vscode.commands.registerCommand('go-on.securityBaseline', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            const result = await goOnManager.sendRequest('security.baseline', {});
            const baseline = result?.baseline ?? {};
            const level = String(baseline?.level ?? 'unknown');
            const ingress = String(baseline?.ingress_status ?? 'unknown');
            const riskCount = Number(baseline?.risk_count ?? 0);
            const strict = Boolean(baseline?.production_strict?.enabled ?? false);
            vscode.window.showInformationMessage(`security.baseline: level=${level}, ingress=${ingress}, strict=${strict}, risks=${riskCount}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`security.baseline failed: ${error.message}`);
        }
    });
    // breaker.reset — reset circuit breaker for a specific agent
    let breakerResetRpcCommand = vscode.commands.registerCommand('go-on.breakerReset', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        const agent = await vscode.window.showInputBox({
            prompt: 'Agent name to reset circuit breaker for',
            placeHolder: 'e.g. copilot, deepseek, gemini'
        });
        if (!agent) {
            return;
        }
        try {
            const result = await goOnManager.sendRequest('breaker.reset', { agent });
            vscode.window.showInformationMessage(`breaker.reset: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`breaker.reset failed: ${error.message}`);
        }
    });
    // breaker.recovery — recover degraded services from failure prevention and circuit breakers
    let breakerRecoveryRpcCommand = vscode.commands.registerCommand('go-on.breakerRecovery', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        const target = await vscode.window.showInputBox({
            prompt: 'Optional agent name to recover (leave empty for all degraded services)',
            placeHolder: 'e.g. copilot, deepseek, gemini'
        });
        if (target === undefined) {
            return;
        }
        try {
            const params = target.trim().length > 0 ? { agent: target.trim() } : {};
            const result = await goOnManager.sendRequest('breaker.recovery', params);
            const recoveredCount = Number(result?.recovered_count ?? 0);
            const remaining = Number(result?.remaining_degraded_count ?? 0);
            vscode.window.showInformationMessage(`breaker.recovery: recovered=${recoveredCount}, remaining_degraded=${remaining}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`breaker.recovery failed: ${error.message}`);
        }
    });
    // maintenance.gc — trigger in-process garbage collection
    let maintenanceGcRpcCommand = vscode.commands.registerCommand('go-on.maintenanceGc', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            await goOnManager.sendRequest('maintenance.gc');
            vscode.window.showInformationMessage('Maintenance GC completed.');
        }
        catch (error) {
            vscode.window.showErrorMessage(`maintenance.gc failed: ${error.message}`);
        }
    });
    // conversation.checkpoint.create — create a conversation checkpoint
    let checkpointCreateRpcCommand = vscode.commands.registerCommand('go-on.checkpointCreate', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        const conversationId = await vscode.window.showInputBox({
            prompt: 'Conversation ID',
            placeHolder: 'e.g. default-session'
        });
        if (!conversationId) {
            return;
        }
        const message = await vscode.window.showInputBox({
            prompt: 'Checkpoint message',
            placeHolder: 'Describe current conversation state'
        });
        if (!message) {
            return;
        }
        try {
            const result = await goOnManager.sendRequest('conversation.checkpoint.create', {
                conversation_id: conversationId,
                messages: [{ role: 'user', content: message }],
            });
            vscode.window.showInformationMessage(`Checkpoint created: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`checkpoint.create failed: ${error.message}`);
        }
    });
    // conversation.checkpoint.list — list conversation checkpoints
    let checkpointListRpcCommand = vscode.commands.registerCommand('go-on.checkpointList', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        const conversationId = await vscode.window.showInputBox({
            prompt: 'Conversation ID',
            placeHolder: 'e.g. default-session'
        });
        if (!conversationId) {
            return;
        }
        try {
            const result = await goOnManager.sendRequest('conversation.checkpoint.list', {
                conversation_id: conversationId,
            });
            vscode.window.showInformationMessage(`Checkpoints: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`checkpoint.list failed: ${error.message}`);
        }
    });
    // conversation.rollback — roll back conversation to a checkpoint
    let conversationRollbackRpcCommand = vscode.commands.registerCommand('go-on.conversationRollback', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        const checkpointId = await vscode.window.showInputBox({
            prompt: 'Checkpoint ID to roll back to',
            placeHolder: 'e.g. ckpt-001'
        });
        if (!checkpointId) {
            return;
        }
        const conversationId = await vscode.window.showInputBox({
            prompt: 'Conversation ID',
            placeHolder: 'e.g. default-session'
        });
        if (!conversationId) {
            return;
        }
        try {
            const result = await goOnManager.sendRequest('conversation.rollback', {
                conversation_id: conversationId,
                checkpoint_id: checkpointId,
            });
            vscode.window.showInformationMessage(`Rolled back: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`conversation.rollback failed: ${error.message}`);
        }
    });
    // primary_secondary.summary — get primary/secondary agent execution summary
    let primarySecondarySummaryRpcCommand = vscode.commands.registerCommand('go-on.primarySecondarySummary', async () => {
        if (!goOnManager.isRunning()) {
            vscode.window.showErrorMessage('Go-On is not running. Start it first.');
            return;
        }
        try {
            const result = await goOnManager.sendRequest('primary_secondary.summary', {});
            vscode.window.showInformationMessage(`primary_secondary.summary: ${JSON.stringify(result)}`);
        }
        catch (error) {
            vscode.window.showErrorMessage(`primary_secondary.summary failed: ${error.message}`);
        }
    });
    // Runtime download/start is intentionally deferred until the Chat view is opened.
    context.subscriptions.push(startCommand, stopCommand, sendRequestCommand, healthCheckCommand, healthProbesCommand, breakerStatusCommand, cacheClearCommand, vectorClearCommand, configReloadCommand, shutdownCommand, openChatCommand, closeChatCommand, openSettingsCommand, clearChatCommand, exportChatCommand, newSessionCommand, switchSessionCommand, createWorkflowCommand, runWorkflowCommand, showProcessFlowCommand, workflowExecuteRpcCommand, taskPlanRpcCommand, taskExecuteRpcCommand, learningSummaryRpcCommand, learningGuardrailRpcCommand, learningReplayRpcCommand, knowledgeDistillRpcCommand, rlAlignmentEvalRpcCommand, hardnessStatusRpcCommand, costStatusRpcCommand, configBaselineRpcCommand, errorContractRpcCommand, buildReproRpcCommand, dataLifecycleRpcCommand, optimizationPeakRpcCommand, autotuneStatusRpcCommand, selectorStatusRpcCommand, governanceStatusRpcCommand, governancePlanGetRpcCommand, governanceAuditRecentRpcCommand, refreshStatusMonitorCommand, keyringSetCommand, keyringGetCommand, keyringDeleteCommand, keyringListCommand, applyDefaultConfigCommand, updateWorkflowMappingCommand, updateRulesCommand, autotuneGetRpcCommand, autotuneResetRpcCommand, metricsGetRpcCommand, metricsResetRpcCommand, traceMetricsRpcCommand, qualityBaselineRpcCommand, runtimeStabilityRpcCommand, harnessStatusRpcCommand, traceGetRpcCommand, observabilityAlertsRpcCommand, securityBaselineRpcCommand, breakerResetRpcCommand, breakerRecoveryRpcCommand, maintenanceGcRpcCommand, checkpointCreateRpcCommand, checkpointListRpcCommand, conversationRollbackRpcCommand, primarySecondarySummaryRpcCommand);
    // Guarantee chat visibility even when the activity bar icon is hidden by layout settings.
    setTimeout(() => {
        void vscode.commands.executeCommand('go-on.openChat');
    }, 300);
}
exports.activate = activate;
function deactivate() {
    if (goOnManager)
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