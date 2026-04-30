<template>
  <div class="chat-view">
    <!-- Top Agents Bar -->
    <div class="agents-bar">
      <div class="agents-scroll">
        <el-tag
          v-for="agent in activeAgents"
          :key="agent.id"
          :type="selectedAgent === agent.id ? 'primary' : 'info'"
          class="agent-chip"
          @click="selectedAgent = agent.id"
          style="cursor:pointer"
        >
          {{ agent.name }}
        </el-tag>
        <el-tag v-if="!activeAgents.length" type="warning">
          {{ t('chat.noAgents') }}
        </el-tag>
      </div>
    </div>

    <!-- Main Chat Area -->
    <div class="chat-main">
      <!-- Left: Session List -->
      <div class="session-panel">
        <div class="session-panel-header">
          <span>{{ t('chat.sessions') }}</span>
          <el-button size="small" type="primary" plain @click="newSession">
            +
          </el-button>
        </div>
        <div class="session-list">
          <div
            v-for="session in sessions"
            :key="session.id"
            :class="['session-item', session.id === activeSessionId ? 'active' : '']"
            @click="selectSession(session.id)"
          >
            <div class="session-name">{{ session.name }}</div>
            <div v-if="session.id === activeSessionId || expandedSession === session.id" class="session-steps">
              <div
                v-for="step in session.steps"
                :key="step.name"
                :class="['step-item', `step-${step.status}`]"
              >
                <span class="step-dot" />
                <span class="step-name">{{ step.name }}</span>
              </div>
              <div v-if="!session.steps.length" class="step-empty">
                {{ t('chat.noSteps') }}
              </div>
            </div>
          </div>
          <div v-if="!sessions.length" class="session-empty">
            {{ t('chat.noSessions') }}
          </div>
        </div>
      </div>

      <!-- Right: Chat Context + Input -->
      <div class="chat-content">
        <!-- Message History (4/5 height) -->
        <div class="message-area" ref="messageAreaRef">
          <div v-if="!messages.length" class="message-empty">
            {{ t('chat.emptyHistory') }}
          </div>
          <div
            v-for="msg in messages"
            :key="msg.id"
            :class="['message', `message-${msg.role}`]"
          >
            <div class="message-role">{{ msg.role === 'user' ? t('chat.you') : t('chat.assistant') }}</div>
            <div class="message-content" v-html="renderMarkdown(msg.content)" />
          </div>
          <div v-if="loading" class="message message-assistant">
            <div class="message-role">{{ t('chat.assistant') }}</div>
            <div class="message-content message-loading">
              <span class="dot" />
              <span class="dot" />
              <span class="dot" />
            </div>
          </div>
        </div>

        <!-- Image / File Previews -->
        <div v-if="attachments.length" class="attachment-previews">
          <div class="attachment-previews-header">
            <span class="attachment-count">{{ t('chat.imageAttached', { n: attachments.length }) }}</span>
            <el-button size="small" text @click="removeAllAttachments">{{ t('chat.removeAll') }}</el-button>
          </div>
          <div class="attachment-previews-scroll">
            <div
              v-for="(att, idx) in attachments"
              :key="idx"
              class="attachment-thumb"
            >
              <img
                v-if="att.type.startsWith('image/')"
                :src="att.dataUrl"
                class="attachment-thumb-img"
                :alt="att.name"
              />
              <div v-else class="attachment-thumb-file">
                <el-icon><Document /></el-icon>
                <span class="attachment-thumb-name">{{ att.name }}</span>
              </div>
              <el-button
                class="attachment-remove"
                size="small"
                circle
                @click="removeAttachment(idx)"
              >
                ✕
              </el-button>
            </div>
          </div>
        </div>

        <!-- Input Area (1/5 height) -->
        <div class="input-area">
          <div class="input-area-row">
            <el-upload
              :show-file-list="false"
              :multiple="true"
              :accept="acceptTypes"
              :before-upload="handleFileSelect"
              :auto-upload="false"
              :disabled="loading"
            >
              <el-button
                size="small"
                :disabled="loading"
                :title="t('chat.attachImage')"
                :aria-label="t('chat.attachImage')"
                class="upload-button"
              >
                <svg t="1743266093421" class="icon" viewBox="0 0 1024 1024" version="1.1" xmlns="http://www.w3.org/2000/svg" p-id="2549" width="16" height="16"><path d="M928 64H96C42.98 64 0 106.98 0 160v704c0 53.02 42.98 96 96 96h832c53.02 0 96-42.98 96-96V160c0-53.02-42.98-96-96-96z m32 800c0 17.673-14.327 32-32 32H96c-17.673 0-32-14.327-32-32V160c0-17.673 14.327-32 32-32h832c17.673 0 32 14.327 32 32v704z" fill="currentColor" p-id="2550"></path><path d="M384 480l-128 192h512l-192-256-128 160zM256 320c-35.346 0-64 28.654-64 64s28.654 64 64 64 64-28.654 64-64-28.654-64-64-64z" fill="currentColor" p-id="2551"></path></svg>
              </el-button>
            </el-upload>
            <el-input
              v-model="inputText"
              type="textarea"
              :rows="3"
              :placeholder="t('chat.inputPlaceholder')"
              :disabled="loading || !isBackendRunning"
              @keydown.ctrl.enter="sendMessage"
              @keydown.meta.enter="sendMessage"
              resize="none"
            />
            <el-button
              type="primary"
              :loading="loading"
              :disabled="(!inputText.trim() && !attachments.length) || !isBackendRunning"
              :aria-label="t('chat.send')"
              @click="sendMessage"
              class="send-button"
            >
              {{ t('chat.send') }}
            </el-button>
          </div>
        </div>

        <!-- Mode Bar (bottom) -->
        <div class="mode-bar">
          <div class="mode-group">
            <span class="mode-label">{{ t('chat.mode') }}:</span>
            <el-radio-group v-model="chatMode" size="small">
              <el-radio-button value="plan">{{ t('chat.modePlan') }}</el-radio-button>
              <el-radio-button value="agent">{{ t('chat.modeAgent') }}</el-radio-button>
              <el-radio-button value="edit">{{ t('chat.modeEdit') }}</el-radio-button>
              <el-radio-button value="auto">{{ t('chat.modeAuto') }}</el-radio-button>
            </el-radio-group>
          </div>
          <div class="mode-group">
            <span class="mode-label">{{ t('chat.link') }}:</span>
            <el-radio-group v-model="chatLink" size="small">
              <el-radio-button value="local">{{ t('chat.linkLocal') }}</el-radio-button>
              <el-radio-button value="server">{{ t('chat.linkServer') }}</el-radio-button>
              <el-radio-button value="multi">{{ t('chat.linkMulti') }}</el-radio-button>
            </el-radio-group>
          </div>
          <div class="mode-group">
            <span class="mode-label">{{ t('chat.workflow') }}:</span>
            <el-radio-group v-model="chatWorkflow" size="small">
              <el-radio-button value="auto">{{ t('chat.workflowAuto') }}</el-radio-button>
              <el-radio-button value="dev">{{ t('chat.workflowDev') }}</el-radio-button>
              <el-radio-button value="general">{{ t('chat.workflowGeneral') }}</el-radio-button>
              <el-radio-button value="custom">{{ t('chat.workflowCustom') }}</el-radio-button>
            </el-radio-group>
          </div>
          <div class="backend-status">
            <el-tag :type="isBackendRunning ? 'success' : 'danger'" size="small">
              {{ isBackendRunning ? t('chat.backendOnline') : t('chat.backendOffline') }}
            </el-tag>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onActivated, onMounted, onUnmounted, nextTick } from "vue";
import { useI18n } from "vue-i18n";
import { ElMessage } from "element-plus";
import { Document } from "@element-plus/icons-vue";
import { useRuntimeStore } from "../stores/runtime";
import { defaultRuntimeBaseUrl } from "../services/protocolContract";

const { t } = useI18n();
const runtime = useRuntimeStore();

// ── State ─────────────────────────────────────────────────────────────────────
// ⚠ DESIGN NOTE: `activeMessages` (derived from `sessions`) is component-local state.
//   Messages are lost on navigation because ChatView unmounts.
//   To preserve messages across routes, either:
//   - Wrap <ChatView /> with <keep-alive> in the parent router-view/tabs, or
//   - Migrate session/message state to a Pinia store (e.g. `useChatStore`).
// Track current AbortController for cleanup on unmount
const currentAbortController = ref<AbortController | null>(null);

const inputText = ref("");
const loading = ref(false);
const messageAreaRef = ref<HTMLElement | null>(null);
const selectedAgent = ref<string | null>(null);
const chatMode = ref("agent");
const chatLink = ref("local");
const chatWorkflow = ref("auto");
const expandedSession = ref<string | null>(null);
const activeSessionId = ref<string | null>(null);

// ── File Attachments ─────────────────────────────────────────────────────────
interface Attachment {
  name: string;
  type: string;
  dataUrl: string;
  file: File;
}
const attachments = ref<Attachment[]>([]);
const acceptTypes = "image/*,.pdf,.txt,.md,.csv,.json";

function handleFileSelect(file: File): boolean {
  const allowedExtensions = ["jpg", "jpeg", "png", "gif", "webp", "bmp", "svg", "pdf", "txt", "md", "csv", "json"];
  const ext = file.name.split(".").pop()?.toLowerCase() || "";
  const isImage = file.type.startsWith("image/");
  if (!isImage && !allowedExtensions.includes(ext)) {
    ElMessage.warning(t("chat.unsupportedFileType", { type: file.type || ext }));
    return false;
  }

  // Read as data URL
  const reader = new FileReader();
  reader.onload = () => {
    attachments.value.push({
      name: file.name,
      type: file.type,
      dataUrl: reader.result as string,
      file,
    });
  };
  reader.readAsDataURL(file);
  return false; // prevent auto-upload
}

function removeAttachment(index: number) {
  attachments.value.splice(index, 1);
}

function removeAllAttachments() {
  attachments.value = [];
}

// ── Agents ────────────────────────────────────────────────────────────────────
interface AgentInfo { id: string; name: string }
const activeAgents = ref<AgentInfo[]>([
  { id: "default", name: "go-on" },
]);

// ── Sessions ──────────────────────────────────────────────────────────────────
interface WorkflowStep { name: string; status: "pending" | "active" | "done" | "error" }
interface Session {
  id: string;
  name: string;
  steps: WorkflowStep[];
  messages: ChatMessage[];
}

interface ChatMessageContentPart {
  type: "text" | "image_url";
  text?: string;
  image_url?: { url: string; detail?: string };
}

interface ChatMessage { id: string; role: "user" | "assistant"; content: string | ChatMessageContentPart[] }

const sessions = ref<Session[]>([]);
let sessionCounter = 0;

function newSession() {
  sessionCounter++;
  const s: Session = {
    id: `s${sessionCounter}`,
    name: `Session ${sessionCounter}`,
    steps: [],
    messages: [],
  };
  sessions.value.push(s);
  activeSessionId.value = s.id;
  expandedSession.value = s.id;
}

function selectSession(id: string) {
  activeSessionId.value = id;
  expandedSession.value = id;
  nextTick(() => scrollToBottom());
}

const messages = computed<ChatMessage[]>(() => {
  const s = sessions.value.find(s => s.id === activeSessionId.value);
  return s ? s.messages : [];
});

const isBackendRunning = computed(() => runtime.status.running);

// ── Markdown (basic) ─────────────────────────────────────────────────────────
function sanitizeHtml(html: string): string {
  const allowed = new Set(["strong", "em", "code", "br"]);
  return html.replace(/<\/?([a-zA-Z][a-zA-Z0-9]*)\b[^>]*>/g, (match, tag) =>
    allowed.has(tag.toLowerCase()) ? match : ""
  );
}

function renderMarkdown(text: string | ChatMessageContentPart[]): string {
  // For multi-modal content, render as plain text from the text part
  if (Array.isArray(text)) {
    const textPart = text.find(p => p.type === "text");
    return renderMarkdownText(textPart?.text || "");
  }
  return renderMarkdownText(text);
}

function renderMarkdownText(text: string): string {
  const html = text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>")
    .replace(/\*(.+?)\*/g, "<em>$1</em>")
    .replace(/`(.+?)`/g, "<code>$1</code>")
    .replace(/\n/g, "<br/>");
  return sanitizeHtml(html);
}

// ── Send ──────────────────────────────────────────────────────────────────────
async function sendMessage() {
  const text = inputText.value.trim();
  if ((!text && !attachments.value.length) || loading.value) return;

  if (!activeSessionId.value) {
    newSession();
  }

  const session = sessions.value.find(s => s.id === activeSessionId.value);
  if (!session) return;

  // Build content: string-only or multi-modal array
  let content: string | ChatMessageContentPart[] = text;
  if (attachments.value.length > 0) {
    const parts: ChatMessageContentPart[] = [{ type: "text", text }];
    for (const att of attachments.value) {
      if (att.type.startsWith("image/")) {
        parts.push({
          type: "image_url",
          image_url: { url: att.dataUrl, detail: "auto" },
        });
      }
    }
    content = parts;
  }

  const userMsg: ChatMessage = { id: `m${Date.now()}`, role: "user", content };
  session.messages.push(userMsg);
  inputText.value = "";
  attachments.value = []; // clear after sending
  loading.value = true;
  await nextTick();
  scrollToBottom();

  try {
    const baseUrl = defaultRuntimeBaseUrl;
    // Cancel previous request if any
    if (currentAbortController.value) {
      currentAbortController.value.abort();
    }
    const controller = new AbortController();
    currentAbortController.value = controller;
    const timeoutId = window.setTimeout(() => controller.abort(), 120000);

    const response = await fetch(`${baseUrl}/v1/chat/completions`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      signal: controller.signal,
      body: JSON.stringify({
        model: selectedAgent.value === "default" ? "adaptive" : (selectedAgent.value ?? "adaptive"),
        messages: session.messages.map(m => ({ role: m.role, content: m.content })),
        stream: false,
      }),
    });

    window.clearTimeout(timeoutId);
    currentAbortController.value = null;
    if (!response.ok) {
      const errText = await response.text();
      throw new Error(`HTTP ${response.status}: ${errText}`);
    }

    const data = await response.json();
    const replyContent: string = data?.choices?.[0]?.message?.content ?? t('chat.errorNoContent');
    const assistantMsg: ChatMessage = {
      id: `m${Date.now()}_r`,
      role: "assistant",
      content: replyContent,
    };
    session.messages.push(assistantMsg);
  } catch (err) {
    currentAbortController.value = null;
    const errorMsg: ChatMessage = {
      id: `m${Date.now()}_e`,
      role: "assistant",
      content: `⚠️ ${err instanceof Error ? err.message : String(err)}`,
    };
    session.messages.push(errorMsg);
    ElMessage.error(String(err));
  } finally {
    loading.value = false;
    await nextTick();
    scrollToBottom();
  }
}

function scrollToBottom() {
  if (messageAreaRef.value) {
    messageAreaRef.value.scrollTop = messageAreaRef.value.scrollHeight;
  }
}

onActivated(() => {
  // When returning to the chat tab, refresh the active session display
  if (activeSessionId.value) {
    nextTick(() => scrollToBottom());
  }
});

onMounted(() => {
  if (!sessions.value.length) {
    newSession();
  }
});

onUnmounted(() => {
  // Abort any in-flight request to prevent memory leaks
  if (currentAbortController.value) {
    currentAbortController.value.abort();
    currentAbortController.value = null;
  }
});
</script>

<style scoped>
.chat-view {
  display: flex;
  flex-direction: column;
  height: 100%;
  overflow: hidden;
}

/* Agents Bar */
.agents-bar {
  flex-shrink: 0;
  padding: 6px 12px;
  border-bottom: 1px solid var(--color-border, #e0e0e0);
  background: var(--color-surface-alt, #f5f5f5);
}
.agents-scroll {
  display: flex;
  gap: 8px;
  overflow-x: auto;
  scrollbar-width: none;
  align-items: center;
  min-height: 28px;
}
.agents-scroll::-webkit-scrollbar { display: none; }
.agent-chip { flex-shrink: 0; }

/* Main layout */
.chat-main {
  display: flex;
  flex: 1;
  overflow: hidden;
}

/* Session Panel */
.session-panel {
  width: 200px;
  flex-shrink: 0;
  border-right: 1px solid var(--color-border, #e0e0e0);
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
.session-panel-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 8px 10px;
  font-weight: 600;
  font-size: 13px;
  border-bottom: 1px solid var(--color-border, #e0e0e0);
  background: var(--color-surface-alt, #f5f5f5);
}
.session-list {
  flex: 1;
  overflow-y: auto;
  padding: 4px 0;
}
.session-item {
  padding: 8px 10px;
  cursor: pointer;
  border-radius: 4px;
  margin: 2px 4px;
}
.session-item:hover { background: var(--color-accent-light, #eff6ff); }
.session-item.active { background: var(--color-accent-light, #eff6ff); }
.session-name { font-size: 13px; font-weight: 500; }
.session-empty { padding: 12px; font-size: 12px; color: var(--color-text-secondary, #9ca3af); text-align: center; }
.session-steps { margin-top: 6px; padding-left: 8px; }
.step-item {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 2px 0;
  font-size: 11px;
}
.step-dot {
  width: 6px; height: 6px;
  border-radius: 50%;
  flex-shrink: 0;
  background: #9ca3af;
}
.step-active .step-dot  { background: #3b82f6; }
.step-done .step-dot    { background: #10b981; }
.step-error .step-dot   { background: #ef4444; }
.step-empty { font-size: 11px; color: #9ca3af; }

/* Chat Content */
.chat-content {
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

/* Message Area */
.message-area {
  flex: 1;
  overflow-y: auto;
  padding: 16px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.message-empty { color: #9ca3af; font-size: 13px; text-align: center; margin-top: 40px; }
.message { display: flex; flex-direction: column; max-width: 80%; }
.message-user { align-self: flex-end; }
.message-assistant { align-self: flex-start; }
.message-role { font-size: 11px; color: var(--color-text-secondary, #6b7280); margin-bottom: 4px; }
.message-content {
  padding: 8px 12px;
  border-radius: 8px;
  font-size: 13px;
  line-height: 1.6;
  word-break: break-word;
}
.message-user .message-content {
  background: var(--color-accent, #3b82f6);
  color: #fff;
  border-radius: 12px 12px 2px 12px;
}
.message-assistant .message-content {
  background: var(--color-surface-alt, #f5f5f5);
  border-radius: 12px 12px 12px 2px;
}
.message-loading {
  display: flex;
  gap: 4px;
  align-items: center;
  padding: 10px 12px;
}
.dot {
  width: 6px; height: 6px;
  border-radius: 50%;
  background: #9ca3af;
  animation: bounce 1.4s infinite ease-in-out;
}
.dot:nth-child(2) { animation-delay: 0.2s; }
.dot:nth-child(3) { animation-delay: 0.4s; }
@keyframes bounce {
  0%, 80%, 100% { transform: scale(0.8); opacity: 0.5; }
  40% { transform: scale(1.2); opacity: 1; }
}

/* Attachment Previews */
.attachment-previews {
  flex-shrink: 0;
  padding: 6px 12px 0 12px;
  border-top: 1px solid var(--color-border, #e0e0e0);
}
.attachment-previews-header {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 6px;
}
.attachment-count {
  font-size: 12px;
  color: var(--color-text-secondary, #6b7280);
}
.attachment-previews-scroll {
  display: flex;
  gap: 8px;
  overflow-x: auto;
  padding-bottom: 6px;
}
.attachment-thumb {
  position: relative;
  flex-shrink: 0;
  width: 80px;
  height: 80px;
  border: 1px solid var(--color-border, #e0e0e0);
  border-radius: 6px;
  overflow: hidden;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--color-surface-alt, #f5f5f5);
}
.attachment-thumb-img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}
.attachment-thumb-file {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 4px;
  padding: 4px;
  font-size: 10px;
  text-align: center;
  word-break: break-all;
  line-height: 1.2;
  color: var(--color-text-secondary, #6b7280);
}
.attachment-thumb-name {
  max-width: 72px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.attachment-remove {
  position: absolute;
  top: 2px;
  right: 2px;
  width: 18px;
  height: 18px;
  min-width: 0;
  padding: 0;
  font-size: 10px;
  line-height: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  opacity: 0.8;
}
.attachment-remove:hover {
  opacity: 1;
}

/* Input Area */
.input-area {
  flex-shrink: 0;
  padding: 8px 12px;
  border-top: 1px solid var(--color-border, #e0e0e0);
}
.input-area-row {
  display: flex;
  gap: 8px;
  align-items: flex-end;
}
.upload-button {
  flex-shrink: 0;
  align-self: flex-end;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 6px;
  margin-bottom: 2px;
}
.send-button { flex-shrink: 0; align-self: flex-end; }

/* Mode Bar */
.mode-bar {
  flex-shrink: 0;
  padding: 6px 12px;
  border-top: 1px solid var(--color-border, #e0e0e0);
  background: var(--color-surface-alt, #f8f8f8);
  display: flex;
  gap: 16px;
  align-items: center;
  flex-wrap: wrap;
  font-size: 12px;
}
.mode-group { display: flex; align-items: center; gap: 6px; }
.mode-label { color: var(--color-text-secondary, #6b7280); white-space: nowrap; }
.backend-status { margin-left: auto; }
</style>
