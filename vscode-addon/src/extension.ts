import * as vscode from 'vscode';
import { spawn, ChildProcess } from 'child_process';
import * as path from 'path';
import * as fs from 'fs';
import * as fsPromises from 'fs/promises';
import * as https from 'https';
import * as os from 'os';
import * as tar from 'tar';
import AdmZip = require('adm-zip');
import { GoOnChatViewProvider } from './chatView';
import { GoOnSettingsViewProvider } from './settingsView';
import { StatusMonitor } from './statusMonitor';
import { GoOnWorkflowViewProvider } from './workflowView';
import { GoOnProcessFlowViewProvider } from './processFlowView';
import { GoOnAdvancedEditProvider } from './advancedEdit';
import { i18n } from './i18n';
import { configManager } from './configManager';

interface JsonRpcRequest {
    jsonrpc: '2.0';
    id: number;
    method: string;
    params?: any;
}

interface JsonRpcResponse {
    jsonrpc: '2.0';
    id: number;
    result?: any;
    error?: {
        code: number;
        message: string;
        data?: any;
    };
}

class GoOnManager {
    private process: ChildProcess | null = null;
    private requestId = 0;
    private pendingRequests = new Map<number, { resolve: Function; reject: Function }>();
    private statusItems: vscode.TreeItem[] = [];
    private runtimeEnvOverrides: Record<string, string> = {};

    constructor() {
        this.updateStatus();
    }

    async start(configPath: string, executablePath: string, cwd: string): Promise<void> {
        if (this.process) {
            throw new Error('Go-On is already running');
        }

        return new Promise((resolve, reject) => {
            let resolved = false;
            let stderrBuffer = '';

            this.process = spawn(executablePath, ['--config', configPath, '--verbose'], {
                cwd,
                env: {
                    ...process.env,
                    ...this.runtimeEnvOverrides
                },
                stdio: ['pipe', 'pipe', 'pipe']
            });

            let startupTimeout: NodeJS.Timeout | undefined = setTimeout(() => {
                this.process?.kill();
                reject(new Error('Go-On startup timeout'));
            }, 10000);

            this.process.stdout?.on('data', (data: Buffer) => {
                const output = data.toString();
                console.log(`Go-On stdout: ${output}`);

                // Try to parse JSON-RPC responses
                try {
                    const lines = output.trim().split('\n');
                    for (const line of lines) {
                        if (line.trim()) {
                            const response: JsonRpcResponse = JSON.parse(line);
                            const pending = this.pendingRequests.get(response.id);
                            if (pending) {
                                this.pendingRequests.delete(response.id);
                                if (response.error) {
                                    pending.reject(new Error(response.error.message));
                                } else {
                                    pending.resolve(response.result);
                                }
                            }
                        }
                    }
                } catch (e) {
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

            this.process.stderr?.on('data', (data: Buffer) => {
                const text = data.toString();
                stderrBuffer += text;
                if (stderrBuffer.length > 4000) {
                    stderrBuffer = stderrBuffer.slice(-4000);
                }
                console.error(`Go-On stderr: ${text}`);
            });

            this.process.on('close', (code: number) => {
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

    stop(): void {
        if (this.process) {
            this.process.kill();
            this.process = null;
        }
        this.updateStatus();
    }

    isRunning(): boolean {
        return this.process !== null;
    }

    setRuntimeEnvOverrides(overrides: Record<string, string>): void {
        this.runtimeEnvOverrides = {
            ...this.runtimeEnvOverrides,
            ...overrides
        };
    }

    async sendRequest(method: string, params?: any): Promise<any> {
        if (!this.process) {
            throw new Error('Go-On is not running');
        }

        const id = ++this.requestId;
        const request: JsonRpcRequest = {
            jsonrpc: '2.0',
            id,
            method,
            params
        };

        return new Promise((resolve, reject) => {
            this.pendingRequests.set(id, { resolve, reject });

            const requestStr = JSON.stringify(request) + '\n';
            this.process!.stdin!.write(requestStr);

            // Timeout after 30 seconds
            setTimeout(() => {
                if (this.pendingRequests.has(id)) {
                    this.pendingRequests.delete(id);
                    reject(new Error('Request timeout'));
                }
            }, 30000);
        });
    }

    private updateStatus(): void {
        this.statusItems = [
            new vscode.TreeItem(`Status: ${this.isRunning() ? 'Running' : 'Stopped'}`, vscode.TreeItemCollapsibleState.None)
        ];
        // Refresh the tree view
        vscode.commands.executeCommand('go-on-status.refresh');

        // Notify status monitor
        vscode.commands.executeCommand('go-on.refreshStatusMonitor');
    }

    getStatusItems(): vscode.TreeItem[] {
        return this.statusItems;
    }
}

class GoOnStatusProvider implements vscode.TreeDataProvider<vscode.TreeItem> {
    private _onDidChangeTreeData: vscode.EventEmitter<vscode.TreeItem | undefined | null | void> = new vscode.EventEmitter<vscode.TreeItem | undefined | null | void>();
    readonly onDidChangeTreeData: vscode.Event<vscode.TreeItem | undefined | null | void> = this._onDidChangeTreeData.event;

    constructor(private manager: GoOnManager) { }

    refresh(): void {
        this._onDidChangeTreeData.fire();
    }

    getTreeItem(element: vscode.TreeItem): vscode.TreeItem {
        return element;
    }

    getChildren(element?: vscode.TreeItem): Thenable<vscode.TreeItem[]> {
        if (!element) {
            return Promise.resolve(this.manager.getStatusItems());
        }
        return Promise.resolve([]);
    }
}

interface RuntimeResolution {
    executablePath: string;
    runtimeDir: string;
}

async function promptForManualBinaryPath(
    config: vscode.WorkspaceConfiguration,
    workspaceRoot: string | undefined,
    reason: string
): Promise<RuntimeResolution> {
    const selectOption = 'Select Local Binary';
    const openSettingsOption = 'Open Go-On Settings';
    const cancelOption = 'Cancel';
    const choice = await vscode.window.showErrorMessage(
        `Failed to download Go-On runtime: ${reason}`,
        selectOption,
        openSettingsOption,
        cancelOption
    );

    if (choice === openSettingsOption) {
        await vscode.commands.executeCommand('workbench.action.openSettings', '@ext:go-on-vscode go-on.executablePath');
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
        await vscode.commands.executeCommand('workbench.action.openSettings', '@ext:go-on-vscode go-on.executablePath');
        throw new Error('No local binary selected. Set go-on.executablePath in settings and try again.');
    }

    const selectedPath = fileSelection[0].fsPath;
    if (!(await pathExists(selectedPath))) {
        throw new Error(`Selected executable does not exist: ${selectedPath}`);
    }

    if (os.platform() !== 'win32') {
        try {
            await fsPromises.chmod(selectedPath, 0o755);
        } catch {
            // Ignore chmod failures for user-managed binaries.
        }
    }

    await config.update(
        'executablePath',
        selectedPath,
        workspaceRoot ? vscode.ConfigurationTarget.Workspace : vscode.ConfigurationTarget.Global
    );

    vscode.window.showInformationMessage(`Using local Go-On binary: ${selectedPath}`);
    return {
        executablePath: selectedPath,
        runtimeDir: path.dirname(selectedPath)
    };
}

function platformAssetInfo(): { assetName: string; executableName: string } {
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

async function pathExists(filePath: string): Promise<boolean> {
    try {
        await fsPromises.access(filePath, fs.constants.F_OK);
        return true;
    } catch {
        return false;
    }
}

function buildReleaseAssetUrl(repository: string, tag: string, assetName: string): string {
    if (tag === 'latest') {
        return `https://github.com/${repository}/releases/latest/download/${assetName}`;
    }
    return `https://github.com/${repository}/releases/download/${tag}/${assetName}`;
}

async function downloadFile(url: string, destinationPath: string, maxRedirects: number = 5): Promise<void> {
    if (maxRedirects <= 0) {
        throw new Error('Too many redirects while downloading file');
    }
    
    await fsPromises.mkdir(path.dirname(destinationPath), { recursive: true });

    await new Promise<void>((resolve, reject) => {
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

async function extractArchive(archivePath: string, destinationDir: string): Promise<void> {
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

async function resolveConfigPath(
    workspaceRoot: string,
    configuredConfigPath: string,
    runtimeDir: string
): Promise<string> {
    const workspaceConfigPath = path.resolve(workspaceRoot, configuredConfigPath);
    if (await pathExists(workspaceConfigPath)) {
        return workspaceConfigPath;
    }

    const bundledConfigPath = path.join(runtimeDir, 'config.toml');
    if (await pathExists(bundledConfigPath)) {
        return bundledConfigPath;
    }

    const workspaceConfigExamplePath = path.join(workspaceRoot, 'config.toml.example');
    if (await pathExists(workspaceConfigExamplePath)) {
        await fsPromises.mkdir(path.dirname(workspaceConfigPath), { recursive: true });
        await fsPromises.copyFile(workspaceConfigExamplePath, workspaceConfigPath);
        vscode.window.showInformationMessage(`Go-On config created from workspace template: ${workspaceConfigPath}`);
        return workspaceConfigPath;
    }

    const bundledConfigExamplePath = path.join(runtimeDir, 'config.toml.example');
    if (await pathExists(bundledConfigExamplePath)) {
        await fsPromises.mkdir(path.dirname(workspaceConfigPath), { recursive: true });
        await fsPromises.copyFile(bundledConfigExamplePath, workspaceConfigPath);
        vscode.window.showInformationMessage(`Go-On config created from runtime template: ${workspaceConfigPath}`);
        return workspaceConfigPath;
    }

    throw new Error(
        `Config not found. Checked workspace path '${workspaceConfigPath}' and bundled path '${bundledConfigPath}'.`
    );
}

async function ensureGoOnBinary(
    workspaceRoot: string | undefined,
    config: vscode.WorkspaceConfiguration,
    context: vscode.ExtensionContext
): Promise<RuntimeResolution> {
    const configuredExecutablePath = config.get<string>('executablePath', './target/release/go-on');

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
    } else if (path.isAbsolute(configuredExecutablePath) && await pathExists(configuredExecutablePath)) {
        return {
            executablePath: configuredExecutablePath,
            runtimeDir: path.dirname(configuredExecutablePath)
        };
    }

    const autoDownloadEnabled = config.get<boolean>('autoDownloadBinary', true);
    if (!autoDownloadEnabled) {
        throw new Error(
            `Configured executable does not exist: ${configuredExecutablePath}. Enable go-on.autoDownloadBinary or set go-on.executablePath.`
        );
    }

    const { assetName, executableName } = platformAssetInfo();
    const releaseRepository = config.get<string>('releaseRepository', 'mikewolfli/go-on');
    const releaseTag = config.get<string>('releaseTag', 'latest');
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
    } catch (error: any) {
        return await promptForManualBinaryPath(
            config,
            workspaceRoot,
            error?.message || String(error)
        );
    }

    if (os.platform() !== 'win32') {
        await fsPromises.chmod(executablePath, 0o755);
    }

    if (!(await pathExists(executablePath))) {
        throw new Error(`Downloaded archive did not contain executable: ${executableName}`);
    }

    vscode.window.showInformationMessage('Go-On runtime download complete. Chat is ready to use.');

    return { executablePath, runtimeDir };
}

async function runGoOnSecretCommand(
    context: vscode.ExtensionContext,
    action: 'set' | 'get' | 'delete' | 'list',
    secretName?: string,
    secretValue?: string
): Promise<string> {
    const config = vscode.workspace.getConfiguration('go-on');
    const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    const runtime = await ensureGoOnBinary(workspaceRoot, config, context);

    const args: string[] = ['--secret', action];
    if (secretName) {
        args.push('--secret-name', secretName);
    }
    if (secretValue !== undefined) {
        args.push('--secret-value', secretValue);
    }

    return new Promise<string>((resolve, reject) => {
        const proc = spawn(runtime.executablePath, args, {
            cwd: workspaceRoot || runtime.runtimeDir,
            stdio: ['ignore', 'pipe', 'pipe']
        });

        let stdout = '';
        let stderr = '';

        proc.stdout?.on('data', (chunk: Buffer) => {
            stdout += chunk.toString();
        });

        proc.stderr?.on('data', (chunk: Buffer) => {
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

function escapeRegex(value: string): string {
    return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function formatTomlStringList(items: string[]): string {
    return `[${items.map((item) => `"${item}"`).join(', ')}]`;
}

function formatTomlMultilineStringList(items: string[]): string {
    const lines = items.map((item) => `    "${item.replace(/"/g, '\\"')}"`);
    return `[
${lines.join(',\n')}
]`;
}

function upsertSectionLine(section: string, lineRegex: RegExp, line: string): string {
    if (lineRegex.test(section)) {
        return section.replace(lineRegex, line);
    }
    const lines = section.split('\n');
    lines.splice(1, 0, line);
    return lines.join('\n');
}

function upsertTopLevelString(content: string, key: string, value: string): string {
    const regex = new RegExp(`^${escapeRegex(key)}\\s*=\\s*".*"\\s*$`, 'm');
    const replacement = `${key} = "${value}"`;
    if (regex.test(content)) {
        return content.replace(regex, replacement);
    }
    return `${replacement}\n${content}`;
}

function upsertFlowPhases(content: string, phases: string[]): string {
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

function upsertPhaseAgents(content: string, phase: string, agents: string[]): string {
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

function upsertPhaseFallback(content: string, phase: string, fallback: boolean): string {
    const header = `[phases.${phase}]`;
    const escapedHeader = escapeRegex(header);
    const sectionRegex = new RegExp(`^${escapedHeader}[\\s\\S]*?(?=^\\[[^\\]]+\\]|\\Z)`, 'm');
    const fallbackLine = `fallback = ${fallback ? 'true' : 'false'}`;

    if (sectionRegex.test(content)) {
        return content.replace(sectionRegex, (section) =>
            upsertSectionLine(section, /^fallback\s*=\s*(true|false)\s*$/m, fallbackLine)
        );
    }

    return `${content.trimEnd()}\n\n${header}\ndescription = "${phase} phase"\nagents = ["copilot"]\n${fallbackLine}\n`;
}

function upsertPhasePrinciples(content: string, phase: string, principles: string[]): string {
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

function upsertPhaseOptionNumber(content: string, phase: string, optionKey: string, value: number): string {
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

async function resolveConfigFilePath(
    context: vscode.ExtensionContext,
    configuredConfigPath?: string
): Promise<{ workspaceRoot: string; configPath: string; runtimeDir: string }> {
    const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    if (!workspaceRoot) {
        throw new Error('No workspace folder open.');
    }

    const config = vscode.workspace.getConfiguration('go-on');
    const runtime = await ensureGoOnBinary(workspaceRoot, config, context);
    const settingPath = configuredConfigPath || config.get<string>('configPath', './config.toml');
    const configPath = await resolveConfigPath(workspaceRoot, settingPath, runtime.runtimeDir);
    return { workspaceRoot, configPath, runtimeDir: runtime.runtimeDir };
}

async function applyDefaultConfigTemplate(
    context: vscode.ExtensionContext,
    templateFile: string
): Promise<string> {
    const { workspaceRoot, configPath, runtimeDir } = await resolveConfigFilePath(context);
    const candidates = [
        path.resolve(workspaceRoot, templateFile),
        path.join(runtimeDir, templateFile)
    ];

    let sourcePath: string | undefined;
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

async function updateWorkflowMappingConfig(
    context: vscode.ExtensionContext,
    mapping: {
        defaultPhase?: string;
        phases?: Record<
            string,
            {
                agents: string[];
                fallback?: boolean;
                principles?: string[];
                switchRules?: {
                    circuitBreakerFailures?: number;
                    circuitBreakerOpenSeconds?: number;
                };
            }
        >;
    }
): Promise<string> {
    const { configPath } = await resolveConfigFilePath(context);
    let content = await fsPromises.readFile(configPath, 'utf8');

    const phaseEntries = Object.entries(mapping.phases || {})
        .map(([phase, config]) => {
            const phaseName = phase.trim();
            const agents = (config?.agents || []).map((a) => a.trim()).filter(Boolean);
            const principles = (config?.principles || []).map((p) => p.trim()).filter(Boolean);
            return [phaseName, { ...config, agents, principles }] as const;
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

async function updateRulesMarkdownFiles(
    context: vscode.ExtensionContext,
    payload: {
        globalRules?: string[];
        commonRules?: string[];
        phaseRules?: Record<string, string[]>;
    }
): Promise<string> {
    const { configPath } = await resolveConfigFilePath(context);
    const configDir = path.dirname(configPath);
    const rulesDir = path.join(configDir, 'RULES');
    await fsPromises.mkdir(rulesDir, { recursive: true });

    const writeRulesFile = async (filePath: string, rules: string[]) => {
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

let goOnManager: GoOnManager;
let statusProvider: GoOnStatusProvider;
let runtimeReadyPromise: Promise<void> | null = null;

async function executeFirstAvailableCommand(commandIds: string[]): Promise<boolean> {
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

async function revealGoOnView(target: 'chat' | 'settings' | 'workflow' | 'process-flow'): Promise<boolean> {
    const openedContainer = await executeFirstAvailableCommand([
        'workbench.view.extension.go-on',
        'workbench.view.extension.go_on',
        'workbench.view.extension.goon'
    ]);

    const focusCommands: Record<typeof target, string[]> = {
        chat: ['go-on-chat.focus', 'go_on_chat.focus'],
        settings: ['go-on-settings.focus', 'go_on_settings.focus'],
        workflow: ['go-on-workflow.focus', 'go_on_workflow.focus'],
        'process-flow': ['go-on-process-flow.focus', 'go_on_process_flow.focus']
    };

    const focused = await executeFirstAvailableCommand(focusCommands[target]);

    if (openedContainer || focused) {
        return true;
    }

    const viewIds: Record<typeof target, string> = {
        chat: 'go-on-chat',
        settings: 'go-on-settings',
        workflow: 'go-on-workflow',
        'process-flow': 'go-on-process-flow'
    };

    try {
        await vscode.commands.executeCommand('workbench.action.openView', viewIds[target]);
        return true;
    } catch {
        return false;
    }
}

async function ensureRuntimeReadyAfterChatOpen(context: vscode.ExtensionContext): Promise<void> {
    if (runtimeReadyPromise) {
        await runtimeReadyPromise;
        return;
    }

    runtimeReadyPromise = (async () => {
        const config = vscode.workspace.getConfiguration('go-on');
        const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;

        await vscode.window.withProgress(
            {
                location: vscode.ProgressLocation.Notification,
                title: 'Preparing Go-On runtime',
                cancellable: false
            },
            async () => {
                await ensureGoOnBinary(workspaceRoot, config, context);
            }
        );

        if (config.get<boolean>('autoStart', false) && !goOnManager.isRunning()) {
            await vscode.commands.executeCommand('go-on.start');
        }
    })();

    try {
        await runtimeReadyPromise;
    } catch (error) {
        runtimeReadyPromise = null;
        throw error;
    }
}

async function ensureGoOnStarted(): Promise<void> {
    if (goOnManager.isRunning()) {
        return;
    }

    await vscode.commands.executeCommand('go-on.start');
    if (!goOnManager.isRunning()) {
        throw new Error('Go-On backend is still stopped after startup attempt. Check executablePath/configPath settings.');
    }
}

async function prepareRuntimeAndStartFromChat(context: vscode.ExtensionContext): Promise<void> {
    try {
        await ensureRuntimeReadyAfterChatOpen(context);
        await ensureGoOnStarted();
    } catch (error: any) {
        vscode.window.showWarningMessage(
            `Chat is open. Backend is not ready yet: ${error?.message || error}. Configure Go-On settings in the Chat/Settings view and retry.`
        );
    }
}

function parseMissingEnvVariableNames(errorMessage: string): string[] {
    const matches = errorMessage.match(/missing required environment variables[^:]*:\s*([^\n]+)/i);
    if (!matches || matches.length < 2) {
        return [];
    }

    return matches[1]
        .split(',')
        .map((name) => name.trim())
        .filter((name) => /^[A-Z0-9_]+$/.test(name));
}

function buildPlaceholderEnvValues(envNames: string[]): Record<string, string> {
    const values: Record<string, string> = {};
    for (const envName of envNames) {
        values[envName] = '__GO_ON_PLACEHOLDER__';
    }
    return values;
}

export function activate(context: vscode.ExtensionContext) {
    console.log('Go-On extension is now active!');

    // Initialize i18n system
    const currentLanguage = i18n.getCurrentLanguage();
    console.log(`Go-On UI Language: ${currentLanguage}`);

    // Initialize config manager
    const config = vscode.workspace.getConfiguration('go-on');
    const configPath = config.get<string>('configPath', './config.toml');
    configManager.initialize(configPath).catch(err => {
        console.warn('Failed to initialize config manager:', err);
    });

    // Sync VS Code language to app configuration
    syncLanguageToApp(context, currentLanguage);

    goOnManager = new GoOnManager();
    statusProvider = new GoOnStatusProvider(goOnManager);

    // Initialize status monitor
    const statusMonitor = new StatusMonitor(goOnManager);
    context.subscriptions.push(statusMonitor);

    // Initialize advanced edit provider
    const advancedEditProvider = new GoOnAdvancedEditProvider(goOnManager, context);

    // Register webview providers
    const chatProvider = new GoOnChatViewProvider(
        context.extensionUri,
        goOnManager,
        context,
        async () => {
            await prepareRuntimeAndStartFromChat(context);
        }
    );
    const settingsProvider = new GoOnSettingsViewProvider(context.extensionUri, goOnManager, context);
    const workflowProvider = new GoOnWorkflowViewProvider(context.extensionUri, goOnManager, context);
    const processFlowProvider = new GoOnProcessFlowViewProvider(context.extensionUri, goOnManager, context);

    context.subscriptions.push(
        vscode.window.registerWebviewViewProvider(GoOnChatViewProvider.viewType, chatProvider),
        vscode.window.registerWebviewViewProvider(GoOnSettingsViewProvider.viewType, settingsProvider),
        vscode.window.registerWebviewViewProvider(GoOnWorkflowViewProvider.viewType, workflowProvider),
        vscode.window.registerWebviewViewProvider(GoOnProcessFlowViewProvider.viewType, processFlowProvider)
    );

    vscode.window.registerTreeDataProvider('go-on-status', statusProvider);

    // Command to start Go-On proxy
    let startCommand = vscode.commands.registerCommand('go-on.start', async () => {
        const config = vscode.workspace.getConfiguration('go-on');
        const configuredConfigPath = config.get<string>('configPath', './config.toml');
        const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
        if (!workspaceFolder) {
            vscode.window.showErrorMessage('No workspace folder open.');
            return;
        }

        const tryStart = async () => {
            const runtime = await ensureGoOnBinary(workspaceFolder.uri.fsPath, config, context);
            const fullConfigPath = await resolveConfigPath(
                workspaceFolder.uri.fsPath,
                configuredConfigPath,
                runtime.runtimeDir
            );

            await goOnManager.start(fullConfigPath, runtime.executablePath, workspaceFolder.uri.fsPath);
        };

        try {
            await tryStart();
            vscode.window.showInformationMessage('Go-On proxy started.');
        } catch (error: any) {
            const errorMessage = String(error?.message || error);
            const missingEnvVars = parseMissingEnvVariableNames(errorMessage);
            if (missingEnvVars.length > 0) {
                try {
                    const envValues = buildPlaceholderEnvValues(missingEnvVars);
                    goOnManager.setRuntimeEnvOverrides(envValues);
                    await tryStart();
                    vscode.window.showWarningMessage('Go-On proxy started without API keys. Configure provider keys in Settings before using cloud agents.');
                    return;
                } catch (retryError: any) {
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

        if (!message) return;

        try {
            const result = await goOnManager.sendRequest('chat', {
                messages: [{ role: 'user', content: message }]
            });
            vscode.window.showInformationMessage(`Response: ${JSON.stringify(result)}`);
        } catch (error: any) {
            vscode.window.showErrorMessage(`Request failed: ${error.message}`);
        }
    });

    // Health check command
    let healthCheckCommand = vscode.commands.registerCommand('go-on.healthCheck', async () => {
        try {
            const result = await goOnManager.sendRequest('runtime.health');
            vscode.window.showInformationMessage(`Health: ${JSON.stringify(result)}`);
        } catch (error: any) {
            vscode.window.showErrorMessage(`Health check failed: ${error.message}`);
        }
    });

    // Breaker status command
    let breakerStatusCommand = vscode.commands.registerCommand('go-on.breakerStatus', async () => {
        try {
            const result = await goOnManager.sendRequest('breaker.status');
            vscode.window.showInformationMessage(`Breaker Status: ${JSON.stringify(result)}`);
        } catch (error: any) {
            vscode.window.showErrorMessage(`Breaker status check failed: ${error.message}`);
        }
    });

    // Cache clear command
    let cacheClearCommand = vscode.commands.registerCommand('go-on.cacheClear', async () => {
        try {
            await goOnManager.sendRequest('cache.clear');
            vscode.window.showInformationMessage('Cache cleared.');
        } catch (error: any) {
            vscode.window.showErrorMessage(`Cache clear failed: ${error.message}`);
        }
    });

    // Vector clear command
    let vectorClearCommand = vscode.commands.registerCommand('go-on.vectorClear', async () => {
        try {
            await goOnManager.sendRequest('vector.clear');
            vscode.window.showInformationMessage('Vector memory cleared.');
        } catch (error: any) {
            vscode.window.showErrorMessage(`Vector clear failed: ${error.message}`);
        }
    });

    // Config reload command
    let configReloadCommand = vscode.commands.registerCommand('go-on.configReload', async () => {
        try {
            await goOnManager.sendRequest('config.reload');
            vscode.window.showInformationMessage('Configuration reloaded.');
        } catch (error: any) {
            vscode.window.showErrorMessage(`Config reload failed: ${error.message}`);
        }
    });

    // Shutdown command
    let shutdownCommand = vscode.commands.registerCommand('go-on.shutdown', async () => {
        try {
            await goOnManager.sendRequest('shutdown');
            vscode.window.showInformationMessage('Shutdown initiated.');
        } catch (error: any) {
            vscode.window.showErrorMessage(`Shutdown failed: ${error.message}`);
        }
    });

    // Open chat command
    let openChatCommand = vscode.commands.registerCommand('go-on.openChat', async () => {
        // Always show chat first; backend preparation runs in background.
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
        } else {
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

    let keyringSetCommand = vscode.commands.registerCommand('go-on.keyringSet', async (payload?: { name?: string; value?: string }) => {
        const name = payload?.name;
        const value = payload?.value;
        if (!name || value === undefined) {
            throw new Error('keyring set requires name and value');
        }
        await runGoOnSecretCommand(context, 'set', name, value);
    });

    let keyringGetCommand = vscode.commands.registerCommand('go-on.keyringGet', async (payload?: { name?: string }) => {
        const name = payload?.name;
        if (!name) {
            throw new Error('keyring get requires name');
        }
        return await runGoOnSecretCommand(context, 'get', name);
    });

    let keyringDeleteCommand = vscode.commands.registerCommand('go-on.keyringDelete', async (payload?: { name?: string }) => {
        const name = payload?.name;
        if (!name) {
            throw new Error('keyring delete requires name');
        }
        await runGoOnSecretCommand(context, 'delete', name);
    });

    let keyringListCommand = vscode.commands.registerCommand('go-on.keyringList', async () => {
        return await runGoOnSecretCommand(context, 'list');
    });

    let applyDefaultConfigCommand = vscode.commands.registerCommand('go-on.applyDefaultConfigTemplate', async (payload?: { template?: string }) => {
        const template = payload?.template;
        if (!template) {
            throw new Error('template is required');
        }
        const configPath = await applyDefaultConfigTemplate(context, template);
        return configPath;
    });

    let updateWorkflowMappingCommand = vscode.commands.registerCommand('go-on.updateWorkflowMapping', async (payload?: {
        defaultPhase?: string;
        phases?: Record<string, {
            agents: string[];
            fallback?: boolean;
            principles?: string[];
            switchRules?: {
                circuitBreakerFailures?: number;
                circuitBreakerOpenSeconds?: number;
            };
        }>;
    }) => {
        if (!payload) {
            throw new Error('workflow mapping payload is required');
        }
        return await updateWorkflowMappingConfig(context, payload);
    });

    let updateRulesCommand = vscode.commands.registerCommand('go-on.updateRules', async (payload?: {
        globalRules?: string[];
        commonRules?: string[];
        phaseRules?: Record<string, string[]>;
    }) => {
        if (!payload) {
            throw new Error('rules payload is required');
        }
        return await updateRulesMarkdownFiles(context, payload);
    });

    // Runtime download/start is intentionally deferred until the Chat view is opened.

    context.subscriptions.push(
        startCommand,
        stopCommand,
        sendRequestCommand,
        healthCheckCommand,
        breakerStatusCommand,
        cacheClearCommand,
        vectorClearCommand,
        configReloadCommand,
        shutdownCommand,
        openChatCommand,
        closeChatCommand,
        openSettingsCommand,
        clearChatCommand,
        exportChatCommand,
        newSessionCommand,
        switchSessionCommand,
        createWorkflowCommand,
        runWorkflowCommand,
        showProcessFlowCommand,
        refreshStatusMonitorCommand,
        keyringSetCommand,
        keyringGetCommand,
        keyringDeleteCommand,
        keyringListCommand,
        applyDefaultConfigCommand,
        updateWorkflowMappingCommand,
        updateRulesCommand
    );

    // Guarantee chat visibility even when the activity bar icon is hidden by layout settings.
    setTimeout(() => {
        void vscode.commands.executeCommand('go-on.openChat');
    }, 300);
}

export function deactivate() {
    if (goOnManager) goOnManager.stop();
}

/**
 * Sync VS Code language with app configuration
 * This ensures the app uses the same language as VS Code
 */
async function syncLanguageToApp(context: vscode.ExtensionContext, language: string): Promise<void> {
    try {
        const config = vscode.workspace.getConfiguration('go-on');
        const configuredConfigPath = config.get<string>('configPath', './config.toml');
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
    } catch (error) {
        console.warn('Failed to sync language:', error);
    }
}