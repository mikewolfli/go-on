"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.buildPlaceholderEnvValues = exports.parseMissingEnvVariableNames = exports.prepareRuntimeAndStartFromChat = exports.ensureGoOnStarted = exports.ensureRuntimeReadyAfterChatOpen = void 0;
const vscode = require("vscode");
let runtimeReadyPromise = null;
async function ensureRuntimeReadyAfterChatOpen(context, deps) {
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
            await deps.ensureBinary(workspaceRoot, config, context);
        });
        if (config.get('autoStart', false) && !deps.isRunning()) {
            await vscode.commands.executeCommand(deps.startCommandId);
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
exports.ensureRuntimeReadyAfterChatOpen = ensureRuntimeReadyAfterChatOpen;
async function ensureGoOnStarted(isRunning, startCommandId) {
    if (isRunning()) {
        return;
    }
    await vscode.commands.executeCommand(startCommandId);
    if (!isRunning()) {
        throw new Error('Go-On backend is still stopped after startup attempt. Check executablePath/configPath settings.');
    }
}
exports.ensureGoOnStarted = ensureGoOnStarted;
async function prepareRuntimeAndStartFromChat(context, deps) {
    try {
        await ensureRuntimeReadyAfterChatOpen(context, deps);
        await ensureGoOnStarted(deps.isRunning, deps.startCommandId);
    }
    catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        vscode.window.showWarningMessage(`Chat is open. Backend is not ready yet: ${message}. Configure Go-On settings in the Chat/Settings view and retry.`);
    }
}
exports.prepareRuntimeAndStartFromChat = prepareRuntimeAndStartFromChat;
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
exports.parseMissingEnvVariableNames = parseMissingEnvVariableNames;
function buildPlaceholderEnvValues(envNames) {
    const values = {};
    for (const envName of envNames) {
        values[envName] = '__GO_ON_PLACEHOLDER__';
    }
    return values;
}
exports.buildPlaceholderEnvValues = buildPlaceholderEnvValues;
//# sourceMappingURL=runtimeBootstrap.js.map