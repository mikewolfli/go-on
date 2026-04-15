"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.GoOnAdvancedEditProvider = void 0;
const vscode = require("vscode");
class GoOnAdvancedEditProvider {
    constructor(_manager, _context) {
        this.manager = _manager;
        this.context = _context;
        // Register code actions
        this.registerCodeActions();
        // Register commands
        this.registerCommands();
    }
    registerCodeActions() {
        // Provide code actions for refactoring
        this.context.subscriptions.push(vscode.languages.registerCodeActionsProvider(['javascript', 'typescript', 'python', 'java', 'cpp', 'c', 'go', 'rust'], {
            provideCodeActions: (document, range, _context, _token) => {
                const actions = [];
                // Add Go-On specific code actions
                if (range.isEmpty) {
                    actions.push(this.createExplainCodeAction(document, range));
                    actions.push(this.createRefactorCodeAction(document, range));
                    actions.push(this.createOptimizeCodeAction(document, range));
                }
                return actions;
            }
        }));
    }
    extractResponseText(result) {
        if (!result || typeof result !== 'object') {
            return undefined;
        }
        const candidate = result.response;
        return typeof candidate === 'string' ? candidate : undefined;
    }
    getErrorMessage(error) {
        return error instanceof Error ? error.message : String(error);
    }
    registerCommands() {
        // Register advanced editing commands
        this.context.subscriptions.push(vscode.commands.registerCommand('go-on.editCode', async () => {
            await this.handleAdvancedEdit();
        }), vscode.commands.registerCommand('go-on.refactorCode', async () => {
            await this.handleRefactorCode();
        }));
    }
    createExplainCodeAction(document, range) {
        const action = new vscode.CodeAction('Go-On: Explain Code', vscode.CodeActionKind.QuickFix);
        action.command = {
            command: 'go-on.editCode',
            title: 'Explain Code',
            arguments: [{ action: 'explain', document, range }]
        };
        return action;
    }
    createRefactorCodeAction(document, range) {
        const action = new vscode.CodeAction('Go-On: Refactor Code', vscode.CodeActionKind.Refactor);
        action.command = {
            command: 'go-on.refactorCode',
            title: 'Refactor Code',
            arguments: [{ action: 'refactor', document, range }]
        };
        return action;
    }
    createOptimizeCodeAction(document, range) {
        const action = new vscode.CodeAction('Go-On: Optimize Code', vscode.CodeActionKind.Refactor);
        action.command = {
            command: 'go-on.editCode',
            title: 'Optimize Code',
            arguments: [{ action: 'optimize', document, range }]
        };
        return action;
    }
    async handleAdvancedEdit(args) {
        const editor = vscode.window.activeTextEditor;
        if (!editor) {
            vscode.window.showErrorMessage('No active editor');
            return;
        }
        const document = editor.document;
        const selection = editor.selection;
        const selectedText = document.getText(selection);
        if (!selectedText) {
            vscode.window.showInformationMessage('Please select some code to edit');
            return;
        }
        let action = args?.action;
        if (!action) {
            const picked = await vscode.window.showQuickPick([
                { label: 'Explain Code', value: 'explain' },
                { label: 'Refactor Code', value: 'refactor' },
                { label: 'Optimize Code', value: 'optimize' },
                { label: 'Add Comments', value: 'comment' },
                { label: 'Convert to Async', value: 'async' },
                { label: 'Add Error Handling', value: 'error-handling' },
                { label: 'Generate Unit Tests', value: 'test' },
                { label: 'Security Audit', value: 'security-audit' }
            ], { placeHolder: 'Choose an action' });
            if (!picked)
                return;
            action = picked.value;
        }
        try {
            const prompt = this.buildPrompt(action, selectedText, document.languageId, args?.details);
            const result = await this.manager.sendRequest('chat', {
                messages: [{ role: 'user', content: prompt }]
            });
            const responseText = this.extractResponseText(result);
            if (!responseText) {
                vscode.window.showErrorMessage('No response from AI service');
                return;
            }
            await this.handleResultOutput(responseText, editor, selection, document.languageId);
        }
        catch (error) {
            vscode.window.showErrorMessage(`Edit failed: ${this.getErrorMessage(error)}`);
        }
    }
    async handleResultOutput(resultText, editor, selection, language) {
        const config = vscode.workspace.getConfiguration('go-on');
        const showDiffByDefault = config.get('advancedAI.showDiffByDefault', true);
        let showResult;
        if (showDiffByDefault) {
            showResult = { label: 'Show Diff', value: 'diff' };
        }
        else {
            showResult = await vscode.window.showQuickPick([
                { label: 'Replace Selection', value: 'replace' },
                { label: 'Show in New Document', value: 'new-doc' },
                { label: 'Show Diff', value: 'diff' },
                { label: 'Copy to Clipboard', value: 'clipboard' }
            ], { placeHolder: 'How to show the result?' });
        }
        if (!showResult)
            return;
        switch (showResult.value) {
            case 'replace':
                await editor.edit(editBuilder => {
                    editBuilder.replace(selection, resultText);
                });
                vscode.window.showInformationMessage('Result applied to document');
                break;
            case 'new-doc': {
                const doc = await vscode.workspace.openTextDocument({ content: resultText, language });
                await vscode.window.showTextDocument(doc, { preview: false });
                break;
            }
            case 'diff': {
                const originalDoc = await vscode.workspace.openTextDocument({ content: editor.document.getText(selection), language });
                const refactoredDoc = await vscode.workspace.openTextDocument({ content: resultText, language });
                await vscode.commands.executeCommand('vscode.diff', originalDoc.uri, refactoredDoc.uri, 'Original ↔ Refactored');
                break;
            }
            case 'clipboard':
                await vscode.env.clipboard.writeText(resultText);
                vscode.window.showInformationMessage('Result copied to clipboard');
                break;
        }
    }
    async handleRefactorCode(args) {
        const editor = vscode.window.activeTextEditor;
        if (!editor) {
            vscode.window.showErrorMessage('No active editor');
            return;
        }
        const document = editor.document;
        const selection = editor.selection;
        const selectedText = document.getText(selection);
        if (!selectedText) {
            vscode.window.showInformationMessage('Please select some code to refactor');
            return;
        }
        let refactorType = args?.action;
        if (!refactorType) {
            const picked = await vscode.window.showQuickPick([
                { label: 'Extract Function', value: 'extract-function' },
                { label: 'Rename Variables', value: 'rename-variables' },
                { label: 'Simplify Logic', value: 'simplify-logic' },
                { label: 'Improve Performance', value: 'performance' },
                { label: 'Add Type Hints', value: 'type-hints' },
                { label: 'Custom Refactoring', value: 'custom' }
            ], { placeHolder: 'Choose refactoring type' });
            if (!picked)
                return;
            refactorType = picked.value;
        }
        let prompt;
        if (refactorType === 'custom') {
            const customPrompt = await vscode.window.showInputBox({
                prompt: 'Describe the refactoring you want',
                placeHolder: 'e.g., make this function more readable'
            });
            if (!customPrompt)
                return;
            prompt = `Please refactor this code to ${customPrompt}:\n\n${selectedText}`;
        }
        else {
            prompt = this.buildRefactorPrompt(refactorType, selectedText, document.languageId);
        }
        try {
            const result = await this.manager.sendRequest('chat', {
                messages: [{ role: 'user', content: prompt }]
            });
            const responseText = this.extractResponseText(result);
            if (!responseText) {
                vscode.window.showErrorMessage('No response from AI service');
                return;
            }
            await this.handleResultOutput(responseText, editor, selection, document.languageId);
        }
        catch (error) {
            vscode.window.showErrorMessage(`Refactoring failed: ${this.getErrorMessage(error)}`);
        }
    }
    buildPrompt(action, code, language, details) {
        const config = vscode.workspace.getConfiguration('go-on');
        const advancedContext = config.get('advancedAI.context', 'You are a senior engineer assistant with expertise across languages, security, testing, and performance.');
        const modelHint = config.get('chat.model', 'auto');
        const temperature = config.get('chat.temperature', 0.7);
        const base = `${advancedContext} Use ${modelHint} model settings (temperature ${temperature}). Preserve behavior while improving code quality and readability.`;
        const detail = `\n\nInput Language: ${language}\nInput Code:\n${code}\n${details ? `Additional Instructions: ${details}\n` : ''}`;
        const prompts = {
            explain: `${base}\n\nTask: Explain what this code does, including intent, edge cases, and potential pitfalls.${detail}`,
            refactor: `${base}\n\nTask: Refactor this code to be clean, modular, and maintainable. Keep logic equivalent.${detail}`,
            optimize: `${base}\n\nTask: Optimize this code for performance and readability. Mention tradeoffs.${detail}`,
            comment: `${base}\n\nTask: Add comprehensive inline comments and a top-level summary.${detail}`,
            async: `${base}\n\nTask: Convert this code to async/await style (or equivalent async pattern) with error handling.${detail}`,
            'error-handling': `${base}\n\nTask: Add robust error handling, validation, and edge case handling.${detail}`,
            test: `${base}\n\nTask: Generate unit tests for this code snippet with assertions and edge cases.${detail}`,
            'security-audit': `${base}\n\nTask: Perform a security audit of this code and suggest fixes for vulnerabilities.${detail}`
        };
        return prompts[action] || `${base}\n\nTask: ${action} this code.${detail}`;
    }
    buildRefactorPrompt(refactorType, code, language) {
        const prompts = {
            'extract-function': `Please extract functions from this ${language} code to improve readability:\n\n${code}`,
            'rename-variables': `Please rename variables in this ${language} code to be more descriptive:\n\n${code}`,
            'simplify-logic': `Please simplify the logic in this ${language} code:\n\n${code}`,
            performance: `Please optimize this ${language} code for better performance:\n\n${code}`,
            'type-hints': `Please add type hints to this ${language} code:\n\n${code}`
        };
        return prompts[refactorType] || `Please refactor this ${language} code:\n\n${code}`;
    }
}
exports.GoOnAdvancedEditProvider = GoOnAdvancedEditProvider;
//# sourceMappingURL=advancedEdit.js.map