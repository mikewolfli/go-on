import * as vscode from "vscode";
import { t, MessageKeys } from "./i18n";
import { getNonce } from "./utils";

/**
 * Build the full HTML for the chat webview panel.
 */
export function getChatHtml(
  webview: vscode.Webview,
  extensionUri: vscode.Uri,
  isManagerRunning: boolean,
): string {
  const styleResetUri = webview.asWebviewUri(
    vscode.Uri.joinPath(extensionUri, "media", "reset.css"),
  );
  const styleVSCodeUri = webview.asWebviewUri(
    vscode.Uri.joinPath(extensionUri, "media", "vscode.css"),
  );
  const scriptUri = webview.asWebviewUri(
    vscode.Uri.joinPath(extensionUri, "media", "chat.js"),
  );

  const nonce = getNonce();

  return `<!DOCTYPE html>
            <html lang="en">
            <head>
                <meta charset="UTF-8">
                <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource} 'unsafe-inline'; img-src ${webview.cspSource} data:; script-src 'nonce-${nonce}';">
                <meta name="viewport" content="width=device-width, initial-scale=1.0">
                <link href="${styleResetUri}" rel="stylesheet">
                <link href="${styleVSCodeUri}" rel="stylesheet">
                <title>Go-On Chat</title>
                <style>
                    .chat-container {
                        height: 100%;
                        display: flex;
                        flex-direction: column;
                        padding: 10px;
                    }
                    .chat-messages {
                        flex: 1;
                        overflow-y: auto;
                        margin-bottom: 10px;
                        border: 1px solid var(--vscode-panel-border);
                        border-radius: 3px;
                        padding: 10px;
                        background: var(--vscode-input-background);
                    }
                    .message {
                        margin-bottom: 10px;
                        padding: 8px;
                        border-radius: 4px;
                    }
                    .message.user {
                        background: var(--vscode-textLink-foreground);
                        color: white;
                        margin-left: 20px;
                    }
                    .message.assistant {
                        background: var(--vscode-editor-background);
                        border: 1px solid var(--vscode-panel-border);
                        margin-right: 20px;
                    }
                    .message.streaming {
                        background: var(--vscode-editor-background);
                        border: 1px solid var(--vscode-panel-border);
                        margin-right: 20px;
                        border-left: 3px solid var(--vscode-progressBar-background);
                    }
                    .message.error {
                        background: var(--vscode-notificationsErrorIcon-foreground);
                        color: white;
                    }
                    .message.system {
                        background: var(--vscode-notificationsInfoIcon-foreground);
                        color: white;
                        font-style: italic;
                    }
                    .message.typing {
                        background: var(--vscode-editor-background);
                        border: 1px solid var(--vscode-panel-border);
                        margin-right: 20px;
                        opacity: 0.7;
                    }
                    .message-header {
                        font-weight: bold;
                        margin-bottom: 4px;
                        font-size: 0.9em;
                    }
                    .code-block {
                        background: var(--vscode-textCodeBlock-background);
                        border: 1px solid var(--vscode-textCodeBlock-border);
                        border-radius: 3px;
                        margin: 4px 0;
                        overflow: hidden;
                    }
                    .code-block summary {
                        cursor: pointer;
                        padding: 8px 12px;
                        background: var(--vscode-editor-background);
                        user-select: none;
                        list-style: none;
                    }
                    .code-block summary::-webkit-details-marker {
                        display: none;
                    }
                    .code-block summary::before {
                        content: "▸ ";
                        margin-right: 4px;
                    }
                    .code-block[open] summary::before {
                        content: "▾ ";
                    }
                    .code-header {
                        display: flex;
                        justify-content: space-between;
                        align-items: center;
                        width: 100%;
                    }
                    .code-lang {
                        color: var(--vscode-textCodeBlock-foreground);
                        font-family: var(--vscode-editor-font-family);
                        font-size: 0.85em;
                    }
                    .code-actions {
                        display: flex;
                        gap: 6px;
                    }
                    .code-content {
                        margin: 0;
                        padding: 12px;
                        background: var(--vscode-textCodeBlock-background);
                        color: var(--vscode-editor-foreground);
                        overflow-x: auto;
                        font-family: var(--vscode-editor-font-family);
                        font-size: 0.9em;
                        white-space: pre-wrap;
                        word-break: break-word;
                        line-height: 1.5;
                    }
                    .code-block code {
                        font-family: inherit;
                        color: inherit;
                    }
                    .copy-btn, .run-btn {
                        background: var(--vscode-button-background);
                        color: var(--vscode-button-foreground);
                        border: none;
                        border-radius: 3px;
                        padding: 2px 6px;
                        cursor: pointer;
                        font-size: 0.8em;
                    }
                    .copy-btn:hover, .run-btn:hover {
                        background: var(--vscode-button-hoverBackground);
                    }
                    .thinking-block {
                        background: var(--vscode-editor-background);
                        border: 1px solid var(--vscode-input-border);
                        border-radius: 3px;
                        margin: 4px 0;
                        overflow: hidden;
                    }
                    .thinking-block summary {
                        cursor: pointer;
                        padding: 8px 12px;
                        user-select: none;
                        list-style: none;
                        color: var(--vscode-textPreformat-foreground);
                    }
                    .thinking-block summary::-webkit-details-marker {
                        display: none;
                    }
                    .thinking-block summary::before {
                        content: "▸ ";
                        margin-right: 4px;
                    }
                    .thinking-block[open] summary::before {
                        content: "▾ ";
                    }
                    .thinking-toggle {
                        font-weight: 500;
                        font-style: italic;
                    }
                    .thinking-content {
                        margin: 0;
                        padding: 12px;
                        background: var(--vscode-input-background);
                        border-top: 1px solid var(--vscode-input-border);
                        color: var(--vscode-editor-foreground);
                        font-family: var(--vscode-editor-font-family);
                        font-size: 0.85em;
                        white-space: pre-wrap;
                        word-break: break-word;
                        line-height: 1.5;
                    }
                    .inline-code {
                        background: var(--vscode-textCodeBlock-background);
                        border: 1px solid var(--vscode-textCodeBlock-border);
                        border-radius: 2px;
                        padding: 1px 3px;
                        font-family: var(--vscode-editor-font-family);
                        font-size: 0.9em;
                    }
                    .typing-dots {
                        display: flex;
                        gap: 2px;
                    }
                    .typing-dots span {
                        width: 4px;
                        height: 4px;
                        background: var(--vscode-progressBar-background);
                        border-radius: 50%;
                        animation: typing 1.4s infinite;
                    }
                    .typing-dots span:nth-child(2) { animation-delay: 0.2s; }
                    .typing-dots span:nth-child(3) { animation-delay: 0.4s; }
                    @keyframes typing {
                        0%, 60%, 100% { opacity: 0.3; }
                        30% { opacity: 1; }
                    }
                    .chat-input-container {
                        display: flex;
                        gap: 5px;
                    }
                    .stop-generating-container {
                        display: flex;
                        align-items: center;
                        justify-content: center;
                        gap: 8px;
                        padding: 6px 0;
                    }
                    .stop-generating-btn {
                        padding: 6px 16px;
                        background: var(--vscode-errorForeground);
                        color: white;
                        border: none;
                        border-radius: 3px;
                        cursor: pointer;
                        font-size: 0.9em;
                        display: flex;
                        align-items: center;
                        gap: 4px;
                    }
                    .stop-generating-btn:hover {
                        opacity: 0.85;
                    }
                    .token-counter {
                        font-size: 0.8em;
                        color: var(--vscode-descriptionForeground);
                        font-family: var(--vscode-editor-font-family);
                    }
                    .stop-generating-container.hidden {
                        display: none;
                    }
                    .chat-attach {
                        padding: 8px 10px;
                        background: var(--vscode-button-secondaryBackground);
                        color: var(--vscode-button-secondaryForeground);
                        border: 1px solid var(--vscode-button-border);
                        border-radius: 3px;
                        cursor: pointer;
                        font-size: 1em;
                        line-height: 1;
                    }
                    .chat-attach:hover {
                        background: var(--vscode-button-secondaryHoverBackground);
                    }
                    .chat-input {
                        flex: 1;
                        padding: 8px;
                        border: 1px solid var(--vscode-input-border);
                        border-radius: 3px;
                        background: var(--vscode-input-background);
                        color: var(--vscode-input-foreground);
                        resize: vertical;
                        min-height: 20px;
                        max-height: 100px;
                    }
                    .chat-send {
                        padding: 8px 12px;
                        background: var(--vscode-button-background);
                        color: var(--vscode-button-foreground);
                        border: none;
                        border-radius: 3px;
                        cursor: pointer;
                    }
                    .chat-send:hover {
                        background: var(--vscode-button-hoverBackground);
                    }
                    .attachment-preview {
                        display: flex;
                        align-items: flex-start;
                        gap: 6px;
                        padding: 6px 8px;
                        margin-bottom: 6px;
                        background: var(--vscode-editor-background);
                        border: 1px solid var(--vscode-panel-border);
                        border-radius: 3px;
                    }
                    .attachment-list {
                        display: flex;
                        flex-wrap: wrap;
                        gap: 6px;
                        flex: 1;
                    }
                    .attachment-item {
                        display: flex;
                        align-items: center;
                        gap: 4px;
                        padding: 3px 6px;
                        background: var(--vscode-badge-background);
                        color: var(--vscode-badge-foreground);
                        border-radius: 3px;
                        font-size: 0.8em;
                        max-width: 180px;
                        overflow: hidden;
                        text-overflow: ellipsis;
                        white-space: nowrap;
                    }
                    .attachment-item img.attachment-thumb {
                        width: 24px;
                        height: 24px;
                        object-fit: cover;
                        border-radius: 2px;
                    }
                    .attachment-item .attachment-icon {
                        font-size: 1.1em;
                    }
                    .attachment-item .attachment-name {
                        overflow: hidden;
                        text-overflow: ellipsis;
                        white-space: nowrap;
                    }
                    .attachment-clear {
                        padding: 2px 6px;
                        background: transparent;
                        color: var(--vscode-input-foreground);
                        border: 1px solid var(--vscode-panel-border);
                        border-radius: 3px;
                        cursor: pointer;
                        font-size: 0.9em;
                        line-height: 1;
                        opacity: 0.7;
                    }
                    .attachment-clear:hover {
                        opacity: 1;
                        background: var(--vscode-list-hoverBackground);
                    }
                    .status-bar {
                        padding: 5px;
                        background: var(--vscode-statusBar-background);
                        color: var(--vscode-statusBar-foreground);
                        font-size: 0.8em;
                        text-align: center;
                        border-radius: 3px;
                        margin-bottom: 10px;
                    }
                    .session-controls {
                        display: flex;
                        gap: 5px;
                        margin-bottom: 10px;
                        align-items: center;
                    }
                    .session-select {
                        flex: 1;
                        padding: 4px 8px;
                        border: 1px solid var(--vscode-input-border);
                        border-radius: 3px;
                        background: var(--vscode-dropdown-background);
                        color: var(--vscode-dropdown-foreground);
                    }
                    .session-btn {
                        padding: 4px 8px;
                        background: var(--vscode-button-secondaryBackground);
                        color: var(--vscode-button-secondaryForeground);
                        border: 1px solid var(--vscode-button-border);
                        border-radius: 3px;
                        cursor: pointer;
                        font-size: 0.8em;
                    }
                    .session-btn:hover {
                        background: var(--vscode-button-secondaryHoverBackground);
                    }
                </style>
            </head>
            <body>
                <div class="chat-container">
                    <div class="status-bar" id="status">
                        ${isManagerRunning ? "🟢 " + t(MessageKeys.goOnStarted) : "🔴 " + t(MessageKeys.goOnStopped)}
                    </div>
                    <div class="session-controls">
                        <select class="session-select" id="sessionSelect">
                            <option value="default">default</option>
                        </select>
                        <button class="session-btn" id="newSessionBtn" title="${t(MessageKeys.newSession)}">➕</button>
                        <button class="session-btn" id="clearSessionBtn" title="${t(MessageKeys.clearChat)}">🗑️</button>
                        <button class="session-btn" id="exportSessionBtn" title="${t(MessageKeys.export)}">📤</button>
                    </div>
                    <div class="chat-messages" id="messages"></div>
                        <div class="stop-generating-container hidden" id="stopGenerationContainer">
                            <button class="stop-generating-btn" id="stopGenerationBtn">■ Stop Generating</button>
                            <span class="token-counter" id="tokenCounter">0 tokens</span>
                        </div>
                        <div class="attachment-preview" id="attachmentPreview" style="display:none">
                            <div class="attachment-list" id="attachmentList"></div>
                            <button class="attachment-clear" id="clearAttachmentsBtn">✕</button>
                        </div>
                        <div class="chat-input-container">
                            <input type="file" id="fileInput" accept="image/*,.pdf,.txt,.md" multiple style="display:none" />
                            <button class="chat-attach" id="attachBtn" title="${t(MessageKeys.attachFiles)}">📎</button>
                            <input type="text" class="chat-input" id="messageInput" placeholder="${t(MessageKeys.inputPlaceholder)}" />
                            <button class="chat-send" id="sendButton">${t(MessageKeys.sendMessage)}</button>
                        </div>
                </div>
                <script nonce="${nonce}" src="${scriptUri}"></script>
            </body>
            </html>`;
}
