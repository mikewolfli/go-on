// Chat functionality with enhanced features
(function () {
  // eslint-disable-next-line no-undef
  const vscode =
    typeof acquireVsCodeApi === "function" ? acquireVsCodeApi() : null;
  const messagesContainer = document.getElementById("messages");
  const messageInput = document.getElementById("messageInput");
  const sendButton = document.getElementById("sendButton");
  const sessionSelect = document.getElementById("sessionSelect");
  const newSessionBtn = document.getElementById("newSessionBtn");
  const clearSessionBtn = document.getElementById("clearSessionBtn");
  const exportSessionBtn = document.getElementById("exportSessionBtn");
  const fileInput = document.getElementById("fileInput");
  const attachBtn = document.getElementById("attachBtn");
  const attachmentPreview = document.getElementById("attachmentPreview");
  const attachmentList = document.getElementById("attachmentList");
  const clearAttachmentsBtn = document.getElementById("clearAttachmentsBtn");

  if (!vscode) {
    console.error("Go-On: VS Code API not available");
    return;
  }

  const MAX_FILE_SIZE = 10 * 1024 * 1024; // 10 MB
  let attachments = [];
  let messageHistory = [];
  let currentSession = "default";
  let availableSessions = ["default"];

  // Load existing chat history
  const state = vscode.getState();
  if (state && state.messages) {
    messageHistory = state.messages;
    renderMessages();
  }

  // Enhanced message rendering with markdown support
  function renderMarkdown(text) {
    // Limit input length to prevent ReDoS attacks
    const MAX_RENDER_INPUT_LENGTH = 100000; // 100KB
    if (text.length > MAX_RENDER_INPUT_LENGTH) {
      text =
        text.substring(0, MAX_RENDER_INPUT_LENGTH) + "\n\n[Content truncated]";
    }

    // First escape HTML to prevent XSS
    text = escapeHtml(text);

    // Handle &lt;thinking&gt;...&lt;/thinking&gt; blocks - collapsed by default
    text = text.replace(
      /&lt;thinking&gt;([\s\S]*?)&lt;\/thinking&gt;/gi,
      (_match, content) => {
        const safeContent = content.trim().replace(/\n/g, "<br>");
        return `<details class="thinking-block"><summary class="thinking-toggle">💭 Thinking</summary><div class="thinking-content">${safeContent}</div></details>`;
      },
    );

    // Then handle code blocks - collapsible via <details>
    text = text
      .replace(/```(\w+)?\n?([\s\S]*?)```/g, (_match, lang, code) => {
        const language = lang || "plaintext";
        const canRun =
          language === "javascript" ||
          language === "python" ||
          language === "bash";
        // Content is already escaped, preserve newlines as <br>
        const escapedCode = code.trim().replace(/\n/g, "<br>");
        return `<details class="code-block" open data-language="${language}"><summary class="code-header"><span class="code-lang">${escapeHtml(language)}</span><span class="code-actions">${canRun ? '<button class="run-btn" data-action="run">▶️</button>' : ""}<button class="copy-btn" data-action="copy">📋</button></span></summary><div class="code-content"><code class="language-${language}">${escapedCode}</code></div></details>`;
      })
      .replace(/`([^`]+)`/g, '<code class="inline-code">$1</code>')
      .replace(/\*\*(.*?)\*\*/g, "<strong>$1</strong>")
      .replace(/\*(.*?)\*/g, "<em>$1</em>")
      .replace(/\n/g, "<br>");

    return text;
  }

  function escapeHtml(text) {
    const div = document.createElement("div");
    div.textContent = text;
    return div.innerHTML;
  }

  // Event delegation for code block copy/run buttons (avoids CSP-blocked inline onclick)
  messagesContainer.addEventListener("click", (e) => {
    const button = e.target.closest("button[data-action]");
    if (!button) return;

    const action = button.getAttribute("data-action");
    const codeBlock = button.closest(".code-block");
    const codeElement = codeBlock && codeBlock.querySelector("code");

    if (!codeElement) return;

    if (action === "copy") {
      vscode.postMessage({
        type: "copyCode",
        code: codeElement.textContent || "",
      });
      const orig = button.textContent;
      button.textContent = "✅";
      setTimeout(() => (button.textContent = orig), 1000);
    } else if (action === "run") {
      const language = codeBlock.getAttribute("data-language");
      vscode.postMessage({
        type: "runCode",
        code: codeElement.textContent || "",
        language: language,
      });
    }
  });

  function addMessage(
    role,
    content,
    timestamp = new Date().toISOString(),
    session = currentSession,
  ) {
    const message = { role, content, timestamp, session };
    messageHistory.push(message);
    renderMessages();
    saveState();

    // Auto-scroll to bottom
    messagesContainer.scrollTop = messagesContainer.scrollHeight;
  }

  function renderMessages() {
    messagesContainer.innerHTML = "";

    const sessionMessages = messageHistory.filter(
      (msg) => msg.session === currentSession,
    );

    sessionMessages.forEach((msg) => {
      const messageDiv = document.createElement("div");
      messageDiv.className = `message ${msg.role}`;

      const header = document.createElement("div");
      header.className = "message-header";
      header.textContent = `${msg.role === "user" ? "You" : msg.role === "assistant" ? "Go-On" : "System"} (${new Date(msg.timestamp).toLocaleTimeString()})`;

      const content = document.createElement("div");
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
    messageHistory = messageHistory.filter(
      (msg) => msg.session !== currentSession,
    );
    renderMessages();
    saveState();
  }

  function exportChat() {
    const sessionMessages = messageHistory.filter(
      (msg) => msg.session === currentSession,
    );
    const exportData = {
      session: currentSession,
      timestamp: new Date().toISOString(),
      messages: sessionMessages,
    };

    const blob = new Blob([JSON.stringify(exportData, null, 2)], {
      type: "application/json",
    });
    const url = URL.createObjectURL(blob);

    const a = document.createElement("a");
    a.href = url;
    a.download = `go-on-chat-${currentSession}-${new Date().toISOString().split("T")[0]}.json`;
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
      type: "newSession",
      sessionName,
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
      type: "switchSession",
      sessionName,
    });
  }

  function updateSessionSelect() {
    sessionSelect.innerHTML = "";
    availableSessions.forEach((session) => {
      const option = document.createElement("option");
      option.value = session;
      option.textContent = session;
      if (session === currentSession) {
        option.selected = true;
      }
      sessionSelect.appendChild(option);
    });
  }

  function showError(message) {
    const errorDiv = document.createElement("div");
    errorDiv.className = "message error";
    errorDiv.innerHTML = `<div class="message-header">Error</div><div>${message}</div>`;
    messagesContainer.appendChild(errorDiv);
    messagesContainer.scrollTop = messagesContainer.scrollHeight;

    setTimeout(() => {
      errorDiv.remove();
    }, 3000);
  }

  function updateSessionIndicator() {
    const statusBar = document.getElementById("status");
    const baseText = statusBar.textContent.split(" | ")[0];
    statusBar.textContent = `${baseText} | Session: ${currentSession}`;
  }

  // Enhanced message handling
  // NOTE: Webview JS cannot access vscode i18n/localization APIs.
  // User-facing strings below (typing indicator, session prompts) are hardcoded in English.
  // Future improvement: pass localized strings from the extension via postMessage.
  function showTypingIndicator() {
    const indicator = document.createElement("div");
    indicator.className = "message assistant typing";
    indicator.id = "typing-indicator";
    indicator.innerHTML =
      '<div class="message-header">Go-On is typing...</div><div class="typing-dots"><span></span><span></span><span></span></div>';
    messagesContainer.appendChild(indicator);
    messagesContainer.scrollTop = messagesContainer.scrollHeight;
  }

  function hideTypingIndicator() {
    const indicator = document.getElementById("typing-indicator");
    if (indicator) {
      indicator.remove();
    }
  }

  // --- Attachment handling ---

  function handleFileSelection(files) {
    for (const file of files) {
      if (file.size > MAX_FILE_SIZE) {
        showError(
          `File "${file.name}" exceeds the 10 MB size limit and was skipped.`,
        );
        continue;
      }

      const reader = new FileReader();
      reader.onload = function (e) {
        const dataUrl = e.target.result;
        attachments.push({
          name: file.name,
          type: file.type,
          dataUrl: dataUrl,
        });
        renderAttachments();
      };
      reader.onerror = function () {
        showError(`Failed to read file "${file.name}".`);
      };
      reader.readAsDataURL(file);
    }
  }

  function renderAttachments() {
    if (attachments.length === 0) {
      attachmentPreview.style.display = "none";
      return;
    }

    attachmentPreview.style.display = "flex";
    attachmentList.innerHTML = "";

    attachments.forEach((att, index) => {
      const item = document.createElement("div");
      item.className = "attachment-item";

      const isImage = att.type.startsWith("image/");

      if (isImage) {
        const img = document.createElement("img");
        img.className = "attachment-thumb";
        img.src = att.dataUrl;
        img.alt = att.name;
        item.appendChild(img);
      } else {
        const iconSpan = document.createElement("span");
        iconSpan.className = "attachment-icon";
        iconSpan.textContent = "📄";
        item.appendChild(iconSpan);
      }

      const nameSpan = document.createElement("span");
      nameSpan.className = "attachment-name";
      nameSpan.textContent = att.name;
      item.appendChild(nameSpan);

      const removeBtn = document.createElement("button");
      removeBtn.className = "attachment-clear";
      removeBtn.textContent = "✕";
      removeBtn.style.padding = "0 2px";
      removeBtn.style.fontSize = "0.7em";
      removeBtn.style.border = "none";
      removeBtn.title = `Remove ${att.name}`;
      removeBtn.addEventListener("click", function (e) {
        e.stopPropagation();
        attachments.splice(index, 1);
        renderAttachments();
      });
      item.appendChild(removeBtn);

      attachmentList.appendChild(item);
    });
  }

  function clearAttachments() {
    attachments = [];
    renderAttachments();
  }

  function sendMessage(text) {
    if (attachments.length > 0) {
      vscode.postMessage({
        type: "sendMessageWithAttachments",
        text,
        attachments: attachments.slice(),
      });
      clearAttachments();
    } else {
      vscode.postMessage({ type: "sendMessage", text });
    }
  }

  // Event listeners
  sendButton.addEventListener("click", () => {
    const text = messageInput.value.trim();
    if (text || attachments.length > 0) {
      addMessage("user", text || "[Attachments]");
      sendMessage(text || "");
      messageInput.value = "";
      showTypingIndicator();
    }
  });

  attachBtn.addEventListener("click", () => {
    fileInput.click();
  });

  fileInput.addEventListener("change", () => {
    if (fileInput.files) {
      handleFileSelection(fileInput.files);
      fileInput.value = "";
    }
  });

  clearAttachmentsBtn.addEventListener("click", () => {
    clearAttachments();
  });

  messageInput.addEventListener("keypress", (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      sendButton.click();
    }
  });

  // Session management event listeners
  // NOTE: prompt() is not native VS Code UX. Delegate to extension host.
  newSessionBtn.addEventListener("click", () => {
    vscode.postMessage({
      type: "showInputBox",
      prompt: "Enter a name for the new chat session",
      id: "newSessionName",
    });
  });

  sessionSelect.addEventListener("change", (e) => {
    switchSession(e.target.value);
  });

  // NOTE: confirm() is not native VS Code UX. Delegate to extension host.
  clearSessionBtn.addEventListener("click", () => {
    vscode.postMessage({
      type: "showConfirm",
      message: `Clear all messages in session "${currentSession}"?`,
      id: "clearChat",
    });
  });

  exportSessionBtn.addEventListener("click", () => {
    vscode.postMessage({ type: "exportChat" });
  });

  // Handle messages from extension
  window.addEventListener("message", (event) => {
    const message = event.data;

    switch (message.type) {
      case "addMessage":
        hideTypingIndicator();
        addMessage(
          message.role,
          message.content,
          message.timestamp,
          message.session,
        );
        break;
      case "clearChat":
        clearChat();
        break;
      case "exportChat":
        exportChat();
        break;
      case "codeResult":
        hideTypingIndicator();
        addMessage(
          "system",
          `Code execution result:\n\`\`\`\n${message.result}\n\`\`\``,
        );
        break;
      case "updateStatus": {
        const statusBar = document.getElementById("status");
        statusBar.textContent = message.status;
        updateSessionIndicator();
        break;
      }
      case "showInputBoxResult":
        if (message.id === "newSessionName" && message.value) {
          createNewSession(message.value.trim());
        }
        break;
      case "showConfirmResult":
        if (message.id === "clearChat" && message.confirmed) {
          clearChat();
        }
        break;
      case "newSession":
        createNewSession(message.sessionName);
        break;
      case "switchSession":
        switchSession(message.sessionName);
        break;
      case "sessionsList":
        availableSessions = message.sessions;
        currentSession = message.currentSession;
        updateSessionSelect();
        updateSessionIndicator();
        break;
      case "error":
        showError(message.message);
        break;
    }
  });

  // Initialize
  updateSessionIndicator();
  messageInput.focus();

  // Request sessions list from extension
  vscode.postMessage({ type: "getSessions" });
})();
