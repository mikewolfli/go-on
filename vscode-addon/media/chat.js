// Chat functionality with enhanced features
(function() {
    const vscode = acquireVsCodeApi();
    const messagesContainer = document.getElementById('messages');
    const messageInput = document.getElementById('messageInput');
    const sendButton = document.getElementById('sendButton');
    const sessionSelect = document.getElementById('sessionSelect');
    const newSessionBtn = document.getElementById('newSessionBtn');
    const clearSessionBtn = document.getElementById('clearSessionBtn');
    const exportSessionBtn = document.getElementById('exportSessionBtn');

    let messageHistory = [];
    let currentSession = 'default';
    let availableSessions = ['default'];

    // Load existing chat history
    const state = vscode.getState();
    if (state && state.messages) {
        messageHistory = state.messages;
        renderMessages();
    }

    // Enhanced message rendering with markdown support
    function renderMarkdown(text) {
        // Basic markdown parsing for code blocks and formatting
        return text
            .replace(/```(\w+)?\n?([\s\S]*?)```/g, (match, lang, code) => {
                const language = lang || 'plaintext';
                return `<pre class="code-block" data-language="${language}"><code class="language-${language}">${escapeHtml(code.trim())}</code><button class="copy-btn" onclick="copyCode(this)">📋</button><button class="run-btn" onclick="runCode(this)" style="display: ${language === 'javascript' || language === 'python' || language === 'bash' ? 'inline' : 'none'};">▶️</button></pre>`;
            })
            .replace(/`([^`]+)`/g, '<code class="inline-code">$1</code>')
            .replace(/\*\*(.*?)\*\*/g, '<strong>$1</strong>')
            .replace(/\*(.*?)\*/g, '<em>$1</em>')
            .replace(/\n/g, '<br>');
    }

    function escapeHtml(text) {
        const div = document.createElement('div');
        div.textContent = text;
        return div.innerHTML;
    }

    // Copy code to clipboard
    window.copyCode = function(button) {
        const codeElement = button.previousElementSibling;
        const code = codeElement.textContent || codeElement.innerText;
        navigator.clipboard.writeText(code).then(() => {
            const originalText = button.textContent;
            button.textContent = '✅';
            setTimeout(() => button.textContent = originalText, 1000);
        });
    };

    // Run code functionality
    window.runCode = function(button) {
        const codeElement = button.previousElementSibling;
        const code = codeElement.textContent || codeElement.innerText;
        const language = button.parentElement.getAttribute('data-language');

        vscode.postMessage({
            type: 'runCode',
            code: code,
            language: language
        });
    };

    function addMessage(role, content, timestamp = new Date().toISOString(), session = currentSession) {
        const message = { role, content, timestamp, session };
        messageHistory.push(message);
        renderMessages();
        saveState();

        // Auto-scroll to bottom
        messagesContainer.scrollTop = messagesContainer.scrollHeight;
    }

    function renderMessages() {
        messagesContainer.innerHTML = '';

        const sessionMessages = messageHistory.filter(msg => msg.session === currentSession);

        sessionMessages.forEach(msg => {
            const messageDiv = document.createElement('div');
            messageDiv.className = `message ${msg.role}`;

            const header = document.createElement('div');
            header.className = 'message-header';
            header.textContent = `${msg.role === 'user' ? 'You' : msg.role === 'assistant' ? 'Go-On' : 'System'} (${new Date(msg.timestamp).toLocaleTimeString()})`;

            const content = document.createElement('div');
            content.innerHTML = renderMarkdown(msg.content);

            messageDiv.appendChild(header);
            messageDiv.appendChild(content);
            messagesContainer.appendChild(messageDiv);
        });
    }

    function saveState() {
        vscode.setState({ messages: messageHistory, currentSession });
    }

    function clearChat() {
        messageHistory = messageHistory.filter(msg => msg.session !== currentSession);
        renderMessages();
        saveState();
    }

    function exportChat() {
        const sessionMessages = messageHistory.filter(msg => msg.session === currentSession);
        const exportData = {
            session: currentSession,
            timestamp: new Date().toISOString(),
            messages: sessionMessages
        };

        const blob = new Blob([JSON.stringify(exportData, null, 2)], { type: 'application/json' });
        const url = URL.createObjectURL(blob);

        const a = document.createElement('a');
        a.href = url;
        a.download = `go-on-chat-${currentSession}-${new Date().toISOString().split('T')[0]}.json`;
        document.body.appendChild(a);
        a.click();
        document.body.removeChild(a);
        URL.revokeObjectURL(url);
    }

    // Session management
    function createNewSession(sessionName) {
        if (!sessionName) return;

        if (availableSessions.includes(sessionName)) {
            showError(`Session "${sessionName}" already exists`);
            return;
        }

        currentSession = sessionName;
        availableSessions.push(sessionName);
        updateSessionSelect();
        renderMessages();
        saveState();
        updateSessionIndicator();

        vscode.postMessage({
            type: 'newSession',
            sessionName
        });
    }

    function switchSession(sessionName) {
        if (!availableSessions.includes(sessionName)) {
            showError(`Session "${sessionName}" does not exist`);
            return;
        }

        currentSession = sessionName;
        updateSessionSelect();
        renderMessages();
        saveState();
        updateSessionIndicator();

        vscode.postMessage({
            type: 'switchSession',
            sessionName
        });
    }

    function updateSessionSelect() {
        sessionSelect.innerHTML = '';
        availableSessions.forEach(session => {
            const option = document.createElement('option');
            option.value = session;
            option.textContent = session;
            if (session === currentSession) {
                option.selected = true;
            }
            sessionSelect.appendChild(option);
        });
    }

    function showError(message) {
        const errorDiv = document.createElement('div');
        errorDiv.className = 'message error';
        errorDiv.innerHTML = `<div class="message-header">Error</div><div>${message}</div>`;
        messagesContainer.appendChild(errorDiv);
        messagesContainer.scrollTop = messagesContainer.scrollHeight;

        setTimeout(() => {
            errorDiv.remove();
        }, 3000);
    }

    function updateSessionIndicator() {
        const statusBar = document.getElementById('status');
        const baseText = statusBar.textContent.split(' | ')[0];
        statusBar.textContent = `${baseText} | Session: ${currentSession}`;
    }

    // Enhanced message handling
    function showTypingIndicator() {
        const indicator = document.createElement('div');
        indicator.className = 'message assistant typing';
        indicator.id = 'typing-indicator';
        indicator.innerHTML = '<div class="message-header">Go-On is typing...</div><div class="typing-dots"><span></span><span></span><span></span></div>';
        messagesContainer.appendChild(indicator);
        messagesContainer.scrollTop = messagesContainer.scrollHeight;
    }

    function hideTypingIndicator() {
        const indicator = document.getElementById('typing-indicator');
        if (indicator) {
            indicator.remove();
        }
    }

    // Event listeners
    sendButton.addEventListener('click', () => {
        const text = messageInput.value.trim();
        if (text) {
            addMessage('user', text);
            vscode.postMessage({ type: 'sendMessage', text });
            messageInput.value = '';
            showTypingIndicator();
        }
    });

    messageInput.addEventListener('keypress', (e) => {
        if (e.key === 'Enter' && !e.shiftKey) {
            e.preventDefault();
            sendButton.click();
        }
    });

    // Session management event listeners
    newSessionBtn.addEventListener('click', () => {
        const sessionName = prompt('Enter a name for the new session:');
        if (sessionName && sessionName.trim()) {
            createNewSession(sessionName.trim());
        }
    });

    sessionSelect.addEventListener('change', (e) => {
        switchSession(e.target.value);
    });

    clearSessionBtn.addEventListener('click', () => {
        if (confirm(`Clear all messages in session "${currentSession}"?`)) {
            vscode.postMessage({ type: 'clearChat' });
        }
    });

    exportSessionBtn.addEventListener('click', () => {
        vscode.postMessage({ type: 'exportChat' });
    });

    // Handle messages from extension
    window.addEventListener('message', event => {
        const message = event.data;

        switch (message.type) {
            case 'addMessage':
                hideTypingIndicator();
                addMessage(message.role, message.content, message.timestamp);
                break;
            case 'clearChat':
                clearChat();
                break;
            case 'exportChat':
                exportChat();
                break;
            case 'codeResult':
                hideTypingIndicator();
                addMessage('system', `Code execution result:\n\`\`\`\n${message.result}\n\`\`\``);
                break;
            case 'updateStatus':
                const statusBar = document.getElementById('status');
                statusBar.textContent = message.status;
                updateSessionIndicator();
                break;
            case 'newSession':
                createNewSession(message.sessionName);
                break;
            case 'switchSession':
                switchSession(message.sessionName);
                break;
            case 'sessionsList':
                availableSessions = message.sessions;
                currentSession = message.currentSession;
                updateSessionSelect();
                updateSessionIndicator();
                break;
            case 'error':
                showError(message.message);
                break;
        }
    });

    // Initialize
    updateSessionIndicator();
    messageInput.focus();

    // Request sessions list from extension
    vscode.postMessage({ type: 'getSessions' });
})();