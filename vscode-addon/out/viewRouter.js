"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.revealGoOnView = void 0;
const vscode = require("vscode");
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
exports.revealGoOnView = revealGoOnView;
//# sourceMappingURL=viewRouter.js.map