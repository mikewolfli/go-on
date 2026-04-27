"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.registerViewCommands = void 0;
const vscode = require("vscode");
function registerViewCommands(deps) {
    const openChatCommand = vscode.commands.registerCommand("go-on.openChat", async () => {
        try {
            await deps.ensureBinaryReady();
        }
        catch (error) {
            await deps.revealGoOnView("settings");
            const message = error instanceof Error ? error.message : String(error);
            vscode.window.showWarningMessage(`Go-On executable is not ready: ${message}. Please set a valid .exe, .bat, or .sh path in Settings.`);
            return;
        }
        const opened = await deps.revealGoOnView("chat");
        if (!opened) {
            vscode.window.showWarningMessage("Go-On Chat view is not available yet. Reload Window after installing/updating the extension.");
        }
        void deps.prepareRuntimeAfterChatOpen();
    });
    const closeChatCommand = vscode.commands.registerCommand("go-on.closeChat", async () => {
        await vscode.commands.executeCommand("workbench.view.explorer");
        if (deps.isRunning()) {
            deps.stop();
            vscode.window.showInformationMessage("Go-On chat closed. Running backend was stopped.");
        }
        else {
            vscode.window.showInformationMessage("Go-On chat closed. Backend was already stopped.");
        }
    });
    const openSettingsCommand = vscode.commands.registerCommand("go-on.openSettings", async () => {
        const opened = await deps.revealGoOnView("settings");
        if (!opened) {
            vscode.window.showWarningMessage("Go-On Settings view is not available yet. Reload Window after installing/updating the extension.");
        }
    });
    const createWorkflowCommand = vscode.commands.registerCommand("go-on.createWorkflow", async () => {
        const opened = await deps.revealGoOnView("workflow");
        if (!opened) {
            vscode.window.showWarningMessage("Go-On Workflow view is not available yet. Reload Window after installing/updating the extension.");
        }
    });
    const runWorkflowCommand = vscode.commands.registerCommand("go-on.runWorkflow", () => {
        vscode.window.showInformationMessage("Select a workflow to run from the Workflow panel");
    });
    const showProcessFlowCommand = vscode.commands.registerCommand("go-on.showProcessFlow", async () => {
        const opened = await deps.revealGoOnView("process-flow");
        if (!opened) {
            vscode.window.showWarningMessage("Go-On Process Flow view is not available yet. Reload Window after installing/updating the extension.");
        }
    });
    const clearChatCommand = vscode.commands.registerCommand("go-on.clearChat", () => {
        deps.clearChat();
    });
    const exportChatCommand = vscode.commands.registerCommand("go-on.exportChat", () => {
        deps.exportChat();
    });
    const newSessionCommand = vscode.commands.registerCommand("go-on.newSession", () => {
        vscode.window
            .showInputBox({
            prompt: "Enter a name for the new chat session",
            placeHolder: "My Session",
        })
            .then((sessionName) => {
            if (sessionName) {
                deps.createSession(sessionName);
            }
        });
    });
    const switchSessionCommand = vscode.commands.registerCommand("go-on.switchSession", () => {
        vscode.window
            .showQuickPick(["default", "session1", "session2"], {
            placeHolder: "Select a chat session to switch to",
        })
            .then((session) => {
            if (session) {
                deps.switchSession(session);
            }
        });
    });
    return [
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
    ];
}
exports.registerViewCommands = registerViewCommands;
//# sourceMappingURL=commandRegistry.js.map