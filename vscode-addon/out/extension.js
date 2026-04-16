"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.deactivate = exports.activate = void 0;
const vscode = require("vscode");
const child_process_1 = require("child_process");
const path = require("path");
const fsPromises = require("fs/promises");
const chatView_1 = require("./chatView");
const settingsView_1 = require("./settingsView");
const statusMonitor_1 = require("./statusMonitor");
const workflowView_1 = require("./workflowView");
const processFlowView_1 = require("./processFlowView");
const advancedEdit_1 = require("./advancedEdit");
const i18n_1 = require("./i18n");
const configManager_1 = require("./configManager");
const viewRouter_1 = require("./viewRouter");
const commandRegistry_1 = require("./commandRegistry");
const rpcCommandRegistry_1 = require("./rpcCommandRegistry");
const coreCommandRegistry_1 = require("./coreCommandRegistry");
const runtimeManager_1 = require("./runtimeManager");
const runtimeBinaryService_1 = require("./runtimeBinaryService");
const runtimeBootstrap_1 = require("./runtimeBootstrap");
async function runGoOnSecretCommand(context, action, secretName, secretValue) {
    const config = vscode.workspace.getConfiguration('go-on');
    const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    const runtime = await (0, runtimeBinaryService_1.ensureGoOnBinary)(workspaceRoot, config, context);
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
    const flowSectionRegex = /\[flow\][\s\S]*?(?=\n\[[^\]]+\]|$)/;
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
    const runtime = await (0, runtimeBinaryService_1.ensureGoOnBinary)(workspaceRoot, config, context);
    const settingPath = configuredConfigPath || config.get('configPath', './config.toml');
    const configPath = await (0, runtimeBinaryService_1.resolveConfigPath)(workspaceRoot, settingPath, runtime.runtimeDir);
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
        if (await (0, runtimeBinaryService_1.pathExists)(candidate)) {
            sourcePath = candidate;
            break;
        }
    }
    if (!sourcePath) {
        throw new Error(`Template not found: ${templateFile}`);
    }
    await fsPromises.copyFile(sourcePath, configPath);
    await (0, runtimeBinaryService_1.ensureProvidersTomlForConfig)(workspaceRoot, runtimeDir, configPath);
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
let goOnOutput;
function activate(context) {
    goOnOutput = vscode.window.createOutputChannel('Go-On');
    context.subscriptions.push(goOnOutput);
    goOnOutput.appendLine('Go-On extension activated');
    // Initialize i18n system
    const currentLanguage = i18n_1.i18n.getCurrentLanguage();
    goOnOutput.appendLine(`UI Language: ${currentLanguage}`);
    // Initialize config manager
    const config = vscode.workspace.getConfiguration('go-on');
    const configPath = config.get('configPath', './config.toml');
    configManager_1.configManager.initialize(configPath).catch(err => {
        goOnOutput.appendLine(`warn: config manager init failed: ${err}`);
    });
    // Sync VS Code language to app configuration
    syncLanguageToApp(currentLanguage);
    goOnManager = new runtimeManager_1.GoOnManager();
    goOnManager.setOutputChannel(goOnOutput);
    statusProvider = new runtimeManager_1.GoOnStatusProvider(goOnManager);
    const runtimeBootstrapDeps = {
        ensureBinary: runtimeBinaryService_1.ensureGoOnBinary,
        isRunning: () => goOnManager.isRunning(),
        startCommandId: 'go-on.start'
    };
    // Initialize status monitor
    const statusMonitor = new statusMonitor_1.StatusMonitor(goOnManager);
    context.subscriptions.push(statusMonitor);
    // Initialize advanced edit provider
    const _advancedEditProvider = new advancedEdit_1.GoOnAdvancedEditProvider(goOnManager, context);
    // Register webview providers
    const chatProvider = new chatView_1.GoOnChatViewProvider(context.extensionUri, goOnManager, context, async () => {
        await (0, runtimeBootstrap_1.prepareRuntimeAndStartFromChat)(context, runtimeBootstrapDeps);
    });
    const settingsProvider = new settingsView_1.GoOnSettingsViewProvider(context.extensionUri, goOnManager, context);
    const workflowProvider = new workflowView_1.GoOnWorkflowViewProvider(context.extensionUri, goOnManager, context);
    const processFlowProvider = new processFlowView_1.GoOnProcessFlowViewProvider(context.extensionUri, goOnManager, context);
    context.subscriptions.push(vscode.window.registerWebviewViewProvider(chatView_1.GoOnChatViewProvider.viewType, chatProvider), vscode.window.registerWebviewViewProvider(settingsView_1.GoOnSettingsViewProvider.viewType, settingsProvider), vscode.window.registerWebviewViewProvider(workflowView_1.GoOnWorkflowViewProvider.viewType, workflowProvider), vscode.window.registerWebviewViewProvider(processFlowView_1.GoOnProcessFlowViewProvider.viewType, processFlowProvider));
    vscode.window.registerTreeDataProvider('go-on-status', statusProvider);
    const coreCommands = (0, coreCommandRegistry_1.registerCoreCommands)({
        context,
        ensureBinary: runtimeBinaryService_1.ensureGoOnBinary,
        resolveConfigPath: runtimeBinaryService_1.resolveConfigPath,
        parseMissingEnvVariableNames: runtimeBootstrap_1.parseMissingEnvVariableNames,
        buildPlaceholderEnvValues: runtimeBootstrap_1.buildPlaceholderEnvValues,
        start: (configPath, executablePath, cwd, protocolMode) => goOnManager.start(configPath, executablePath, cwd, protocolMode),
        stop: () => goOnManager.stop(),
        isRunning: () => goOnManager.isRunning(),
        sendRequest: (method, params) => goOnManager.sendRequest(method, params),
        setRuntimeEnvOverrides: (overrides) => goOnManager.setRuntimeEnvOverrides(overrides),
    });
    const viewCommands = (0, commandRegistry_1.registerViewCommands)({
        revealGoOnView: viewRouter_1.revealGoOnView,
        ensureBinaryReady: async () => {
            const config = vscode.workspace.getConfiguration('go-on');
            const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
            await (0, runtimeBinaryService_1.ensureGoOnBinary)(workspaceRoot, config, context);
        },
        prepareRuntimeAfterChatOpen: async () => (0, runtimeBootstrap_1.prepareRuntimeAndStartFromChat)(context, runtimeBootstrapDeps),
        isRunning: () => goOnManager.isRunning(),
        stop: () => goOnManager.stop(),
        createSession: (sessionName) => chatProvider.createNewSession(sessionName),
        switchSession: (sessionName) => chatProvider.switchSession(sessionName),
    });
    const rpcCommands = (0, rpcCommandRegistry_1.registerRpcCommands)({
        isRunning: () => goOnManager.isRunning(),
        sendRequest: (method, params) => goOnManager.sendRequest(method, params),
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
    // Runtime download/start is intentionally deferred until the Chat view is opened.
    context.subscriptions.push(...coreCommands, ...viewCommands, ...rpcCommands, refreshStatusMonitorCommand, keyringSetCommand, keyringGetCommand, keyringDeleteCommand, keyringListCommand, applyDefaultConfigCommand, updateWorkflowMappingCommand, updateRulesCommand);
    // Open chat automatically only once to avoid intrusive focus switching on every startup.
    const hasOpenedChat = context.globalState.get('go-on.hasOpenedChatOnce', false);
    if (!hasOpenedChat) {
        setTimeout(() => {
            void vscode.commands.executeCommand('go-on.openChat');
        }, 300);
        void context.globalState.update('go-on.hasOpenedChatOnce', true);
    }
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
async function syncLanguageToApp(language) {
    try {
        const config = vscode.workspace.getConfiguration('go-on');
        const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
        if (!workspaceFolder) {
            return;
        }
        // Create a language configuration object for the app
        // Store language preference in app settings
        await config.update('language', language, vscode.ConfigurationTarget.Global);
        // Log successful sync
        goOnOutput.appendLine(`Language synchronized: VS Code ${language} -> App ${language}`);
    }
    catch (error) {
        goOnOutput.appendLine(`warn: language sync failed: ${error}`);
    }
}
//# sourceMappingURL=extension.js.map