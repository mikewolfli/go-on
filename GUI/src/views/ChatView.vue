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

        <!-- Input Area (1/5 height) -->
        <div class="input-area">
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
            :disabled="!inputText.trim() || !isBackendRunning"
            @click="sendMessage"
            class="send-button"
          >
            {{ t('chat.send') }}
          </el-button>
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
import { ref, computed, onMounted, nextTick } from "vue";
import { useI18n } from "vue-i18n";
import { ElMessage } from "element-plus";
import { useRuntimeStore } from "../stores/runtime";
import { defaultRuntimeBaseUrl } from "../services/protocolContract";

const { t } = useI18n();
const runtime = useRuntimeStore();

// ── State ─────────────────────────────────────────────────────────────────────
const inputText = ref("");
const loading = ref(false);
const messageAreaRef = ref<HTMLElement | null>(null);
const selectedAgent = ref<string | null>(null);
const chatMode = ref("agent");
const chatLink = ref("local");
const chatWorkflow = ref("auto");
const expandedSession = ref<string | null>(null);
const activeSessionId = ref<string | null>(null);

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

interface ChatMessage { id: string; role: "user" | "assistant"; content: string }

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
function renderMarkdown(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>")
    .replace(/\*(.+?)\*/g, "<em>$1</em>")
    .replace(/`(.+?)`/g, "<code>$1</code>")
    .replace(/\n/g, "<br/>");
}

// ── Send ──────────────────────────────────────────────────────────────────────
async function sendMessage() {
  const text = inputText.value.trim();
  if (!text || loading.value) return;

  if (!activeSessionId.value) {
    newSession();
  }

  const session = sessions.value.find(s => s.id === activeSessionId.value);
  if (!session) return;

  const userMsg: ChatMessage = { id: `m${Date.now()}`, role: "user", content: text };
  session.messages.push(userMsg);
  inputText.value = "";
  loading.value = true;
  await nextTick();
  scrollToBottom();

  try {
    const baseUrl = defaultRuntimeBaseUrl;
    const response = await fetch(`${baseUrl}/v1/chat/completions`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        model: selectedAgent.value === "default" ? "adaptive" : (selectedAgent.value ?? "adaptive"),
        messages: session.messages.map(m => ({ role: m.role, content: m.content })),
        stream: false,
      }),
    });

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

onMounted(() => {
  if (!sessions.value.length) {
    newSession();
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

/* Input Area */
.input-area {
  flex-shrink: 0;
  padding: 8px 12px;
  border-top: 1px solid var(--color-border, #e0e0e0);
  display: flex;
  gap: 8px;
  align-items: flex-end;
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
