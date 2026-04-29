<template>
  <div v-if="showQuickNav" style="position: fixed; right: 0; top: 0; height: 100vh; width: 80px; background-color: #f5f7fa; border-left: 1px solid #e5e7eb; display: flex; flex-direction: column; z-index: 900;">
    <button
      v-for="item in quickNavItems"
      :key="item.tabName"
      @click="navigate(item.tabName)"
      :title="item.label"
      :style="{
        border: 'none',
        background: activeTab === item.tabName ? '#1a73e8' : 'transparent',
        color: activeTab === item.tabName ? '#fff' : '#666',
        padding: '12px 8px',
        fontSize: '12px',
        cursor: 'pointer',
        textAlign: 'center',
        borderLeft: activeTab === item.tabName ? '3px solid #1a73e8' : 'none',
        transition: 'all 0.3s',
      }"
      @mouseenter="(e) => (e.target as HTMLElement).style.backgroundColor = '#e8f0fe'"
      @mouseleave="(e) => (e.target as HTMLElement).style.backgroundColor = activeTab === item.tabName ? '#1a73e8' : 'transparent'"
    >
      <div style="font-size: 20px; margin-bottom: 4px">{{ item.icon }}</div>
      <div style="font-size: 10px; word-break: break-all">{{ item.shortLabel }}</div>
    </button>

    <!-- Expand/Collapse Button -->
    <button
      @click="toggleQuickNav"
      style="
        border: none;
        background: #f5f7fa;
        border-top: 1px solid #e5e7eb;
        margin-top: auto;
        padding: 12px 8px;
        cursor: pointer;
        font-size: 16px;
      "
      :title="showQuickNav ? 'Collapse' : 'Expand'"
    >
      {{ showQuickNav ? "◀" : "▶" }}
    </button>
  </div>

  <!-- Floating button when collapsed -->
  <button
    v-if="!showQuickNav"
    @click="toggleQuickNav"
    style="
      position: fixed;
      right: 20px;
      bottom: 20px;
      width: 50px;
      height: 50px;
      border-radius: 50%;
      background: #1a73e8;
      color: white;
      border: none;
      cursor: pointer;
      font-size: 20px;
      box-shadow: 0 2px 8px rgba(0, 0, 0, 0.15);
      z-index: 800;
    "
    @mouseenter="(e) => (e.target as HTMLElement).style.boxShadow = '0 4px 12px rgba(0, 0, 0, 0.2)'"
    @mouseleave="(e) => (e.target as HTMLElement).style.boxShadow = '0 2px 8px rgba(0, 0, 0, 0.15)'"
    title="Quick Navigation"
  >
    ☰
  </button>
</template>

<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";

const props = withDefaults(defineProps<{
  activeTab?: string;
}>(), {
  activeTab: "",
});

const emit = defineEmits<{
  (e: "navigate", tabName: string, subTab?: string): void;
}>();

const { t } = useI18n();

const showQuickNav = ref(true);

const quickNavItems = computed(() => [
  { tabName: "dashboard", label: t("nav.dashboard"), shortLabel: t("nav.dashboardShort"), icon: "📊", mainTab: "monitor", subTab: "dashboard" },
  { tabName: "monitor", label: t("nav.monitor"), shortLabel: t("nav.monitorShort"), icon: "📈", mainTab: "monitor", subTab: "monitor" },
  { tabName: "ai-usage", label: t("nav.aiUsage"), shortLabel: t("nav.aiUsageShort"), icon: "🤖", mainTab: "monitor", subTab: "ai-usage" },
  { tabName: "health-breakdown", label: t("nav.health"), shortLabel: t("nav.healthShort"), icon: "💚", mainTab: "monitor", subTab: "health" },
  { tabName: "logs", label: t("nav.logs"), shortLabel: t("nav.logsShort"), icon: "📝", mainTab: "monitor", subTab: "logs" },
  { tabName: "setup", label: t("nav.setup"), shortLabel: t("nav.setupShort"), icon: "⚙️", mainTab: "config", subTab: "setup" },
  { tabName: "config", label: t("nav.config"), shortLabel: t("nav.configShort"), icon: "🔧", mainTab: "config", subTab: "config" },
  { tabName: "providers", label: t("nav.providers"), shortLabel: t("nav.providersShort"), icon: "🔑", mainTab: "config", subTab: "providers" },
  { tabName: "backend-ops", label: t("nav.ops"), shortLabel: t("nav.opsShort"), icon: "🛠️", mainTab: "config", subTab: "backend-ops" },
  { tabName: "autotune", label: t("nav.autoTune"), shortLabel: t("nav.autoTuneShort"), icon: "⚡", mainTab: "config", subTab: "autotune" },
  { tabName: "workflow", label: t("nav.workflow"), shortLabel: t("nav.workflowShort"), icon: "🔄", mainTab: "config", subTab: "workflow" },
  { tabName: "security", label: t("nav.security"), shortLabel: t("nav.securityShort"), icon: "🔒", mainTab: "config", subTab: "security" },
]);

function navigate(tabName: string) {
  const item = quickNavItems.value.find(i => i.tabName === tabName);
  if (item) {
    emit("navigate", item.mainTab, item.subTab);
  }
}

function toggleQuickNav() {
  showQuickNav.value = !showQuickNav.value;
  localStorage.setItem("go-on-gui-quicknav", showQuickNav.value.toString());
}

// Restore state from localStorage
const saved = localStorage.getItem("go-on-gui-quicknav");
if (saved === "false") {
  showQuickNav.value = false;
}
</script>
