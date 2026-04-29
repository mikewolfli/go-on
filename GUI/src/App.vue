<template>
  <template v-if="isMiniRoute">
    <router-view />
  </template>
  <template v-else>
    <OfflineIndicator />
    <QuickNavigator
      :activeTab="activeMainTab"
      @navigate="handleQuickNavigate"
    />
    <div v-if="monitorOnly" class="monitor-only-banner">
      ⚠️ {{ t("app.monitorOnlyBanner") }}
      <span class="monitor-only-config-link" @click="activeMainTab = 'config'">{{ t("app.monitorOnlyConfigLink") }}</span>
    </div>
    <el-container :style="monitorOnly ? 'height: calc(100vh - 36px)' : 'height: 100vh'" direction="vertical">
      <OnboardingGuide
        v-model="showOnboarding"
        :runtime-running="runtime.status.running"
        @start-service="onStart"
        @complete="markOnboardingSeen"
        @navigate="handleGuideNavigate"
      />
      <!-- Header -->
      <el-header class="app-header">
        <span class="app-title">{{ t("app.name") }}</span>
        <el-tag :type="runtime.status.running ? 'success' : 'danger'" size="small">
          {{ runtime.status.running ? t("app.serviceRunning") : t("app.serviceStopped") }}
        </el-tag>
        <el-button size="small" type="primary" @click="onStart">{{ t("app.start") }}</el-button>
        <el-button size="small" @click="onStop">{{ t("app.stop") }}</el-button>
        <el-button size="small" type="warning" @click="onRestart">{{ t("app.restart") }}</el-button>
        <el-divider direction="vertical" />
        <el-button size="small" @click="onSwitchToMiniWindow">{{ t("app.miniConsole") }}</el-button>
        <el-button size="small" @click="runtime.refreshAll">{{ t("app.refresh") }}</el-button>
        <el-button size="small" @click="openOnboarding">{{ t("onboarding.open") }}</el-button>
        <el-button size="small" @click="toggleTheme" :title="t('theme.switch')">
          {{ t(themeLabel) }}
        </el-button>
        <el-select :model-value="locale" size="small" style="width: 120px" @change="onLocaleChange">
          <el-option :label="t('language.english')" value="en-US" />
          <el-option :label="t('language.simplifiedChinese')" value="zh-CN" />
        </el-select>
      </el-header>

      <!-- Main 3-tab area -->
      <el-main class="app-main">
        <el-tabs v-model="activeMainTab" class="main-tabs">
          <!-- Monitor Tab -->
          <el-tab-pane :label="t('tab.monitor')" name="monitor">
            <el-tabs v-model="activeMonitorSubTab" class="sub-tabs">
              <el-tab-pane :label="t('menu.dashboard')" name="dashboard">
                <DashboardView />
              </el-tab-pane>
              <el-tab-pane :label="t('menu.monitor')" name="monitor">
                <MonitorView />
              </el-tab-pane>
              <el-tab-pane :label="t('menu.aiUsage')" name="ai-usage">
                <AiUsageView />
              </el-tab-pane>
              <el-tab-pane :label="t('menu.healthBreakdown')" name="health">
                <HealthBreakdownView />
              </el-tab-pane>
              <el-tab-pane :label="t('menu.logs')" name="logs">
                <LogsView />
              </el-tab-pane>
            </el-tabs>
          </el-tab-pane>

          <!-- Config Tab -->
          <el-tab-pane :label="t('tab.config')" name="config">
            <el-tabs v-model="activeConfigSubTab" class="sub-tabs">
              <el-tab-pane :label="t('menu.setup')" name="setup">
                <SetupView />
              </el-tab-pane>
              <el-tab-pane :label="t('menu.config')" name="config">
                <ConfigView />
              </el-tab-pane>
              <el-tab-pane :label="t('menu.providers')" name="providers">
                <ProvidersView />
              </el-tab-pane>
              <el-tab-pane :label="t('menu.backendOps')" name="backend-ops">
                <BackendOpsView />
              </el-tab-pane>
              <el-tab-pane
                v-if="!runtime.status.running || runtime.activeFeatures.autotune"
                :label="t('menu.autoTune')"
                name="autotune"
              >
                <AutoTuneView />
              </el-tab-pane>
              <el-tab-pane
                v-if="!runtime.status.running || runtime.activeFeatures.skills_enabled || runtime.activeFeatures.skills_import"
                :label="t('menu.workflow')"
                name="workflow"
              >
                <WorkflowView />
              </el-tab-pane>
              <el-tab-pane
                v-if="!runtime.status.running || runtime.activeFeatures.entry_auth || runtime.activeFeatures.production_strict"
                :label="t('menu.security')"
                name="security"
              >
                <SecurityView />
              </el-tab-pane>
            </el-tabs>
          </el-tab-pane>

          <!-- Chat Tab -->
          <el-tab-pane :label="t('tab.chat')" name="chat">
            <ChatView />
          </el-tab-pane>
        </el-tabs>
      </el-main>
    </el-container>
  </template>
</template>

<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { ElMessage } from "element-plus";
import { useI18n } from "vue-i18n";
import {
  backendExecutableExists,
  stopService,
  restartService,
  switchToMiniWindow,
} from "./services/bridge";
import {
  bootstrapBackend,
  ensureBackendAndStart,
  startBackendWithChecks,
} from "./services/backendLifecycle";
import { useRuntimeStore } from "./stores/runtime";
import { getLocale, setLocale } from "./locales";
import { currentThemeLabelKey, toggleTheme as toggleThemeFunc } from "./utils/theme";
import { useCrashHandler } from "./composables/useCrashHandler";
import "./themes/default.css";
import "./themes/meadow.css";
import "./themes/ink.css";
import "./themes/wuxia.css";
import "./themes/kitty.css";
import OfflineIndicator from "./components/OfflineIndicator.vue";
import QuickNavigator from "./components/QuickNavigator.vue";
import DashboardView from "./views/DashboardView.vue";
import MonitorView from "./views/MonitorView.vue";
import AiUsageView from "./views/AiUsageView.vue";
import HealthBreakdownView from "./views/HealthBreakdownView.vue";
import LogsView from "./views/LogsView.vue";
import SetupView from "./views/SetupView.vue";
import ConfigView from "./views/ConfigView.vue";
import ProvidersView from "./views/ProvidersView.vue";
import BackendOpsView from "./views/BackendOpsView.vue";
import AutoTuneView from "./views/AutoTuneView.vue";
import WorkflowView from "./views/WorkflowView.vue";
import SecurityView from "./views/SecurityView.vue";
import ChatView from "./views/ChatView.vue";
import OnboardingGuide from "./components/OnboardingGuide.vue";

const runtime = useRuntimeStore();
const route = useRoute();
const isMiniRoute = computed(() => route.path === "/mini");
const { t } = useI18n();
const locale = ref(getLocale());
const themeLabel = computed(() => currentThemeLabelKey());
const activeMainTab = ref("monitor");
const activeMonitorSubTab = ref("dashboard");
const activeConfigSubTab = ref("setup");
const showOnboarding = ref(false);
let previousRunning = runtime.status.running;
const MONITOR_ONLY_KEY = "goon.gui.monitorOnly";
const ONBOARDING_SEEN_KEY = "goon.gui.onboardingSeen";
let stopRunningWatch: (() => void) | undefined;

const monitorOnly = ref(localStorage.getItem(MONITOR_ONLY_KEY) === "true");
const { register: registerCrashHandler, unregister: unregisterCrashHandler } = useCrashHandler({
  onRecover: onRestart,
  t: (key: string) => t(key),
});

function monitorOnlyModeEnabled(): boolean {
  return monitorOnly.value;
}

function handleMonitorOnlyChanged(e: Event) {
  monitorOnly.value = (e as CustomEvent<boolean>).detail;
}

function onLocaleChange(value: string) {
  if (value === "en-US" || value === "zh-CN") {
    setLocale(value);
    locale.value = value;
  }
}

function toggleTheme() {
  toggleThemeFunc();
}

function openOnboarding() {
  showOnboarding.value = true;
}

function markOnboardingSeen() {
  localStorage.setItem(ONBOARDING_SEEN_KEY, "true");
}

function handleGuideNavigate(payload: { mainTab: string; subTab?: string }) {
  activeMainTab.value = payload.mainTab;
  if (payload.mainTab === "monitor" && payload.subTab) {
    activeMonitorSubTab.value = payload.subTab;
  }
  if (payload.mainTab === "config" && payload.subTab) {
    activeConfigSubTab.value = payload.subTab;
  }
}

function handleQuickNavigate(mainTab: string, subTab?: string) {
  activeMainTab.value = mainTab;
  if (mainTab === "monitor" && subTab) {
    activeMonitorSubTab.value = subTab;
  }
  if (mainTab === "config" && subTab) {
    activeConfigSubTab.value = subTab;
  }
}

async function onSwitchToMiniWindow() {
  try {
    await switchToMiniWindow();
  } catch {
    // In browser preview fallback to in-window mini route.
    useRouter().push("/mini");
  }
}

async function onStart() {
  try {
    const exists = await backendExecutableExists();
    if (!exists) {
      await ensureBackendAndStart();
    } else {
      await startBackendWithChecks();
    }
    await runtime.refreshAll();
    ElMessage.success(t("toast.serviceStarted"));
  } catch (error) {
    ElMessage.error(String(error));
  }
}

async function onStop() {
  try {
    await stopService();
    await runtime.refreshAll();
    ElMessage.success(t("toast.serviceStopped"));
  } catch (error) {
    ElMessage.error(String(error));
  }
}

async function onRestart() {
  try {
    const exists = await backendExecutableExists();
    if (!exists) {
      await ensureBackendAndStart();
    } else {
      await restartService();
    }
    await runtime.refreshAll();
    ElMessage.success(t("toast.serviceRestarted"));
  } catch (error) {
    ElMessage.error(String(error));
  }
}

onMounted(async () => {
  try {
    await bootstrapBackend(monitorOnlyModeEnabled());
  } catch (error) {
    ElMessage.error(String(error));
  }

  monitorOnly.value = monitorOnlyModeEnabled();
  window.addEventListener("goon:monitor-only-changed", handleMonitorOnlyChanged);

  runtime.startStatusPolling();

  stopRunningWatch = watch(
    () => runtime.status.running,
    (running) => {
      if (!previousRunning && running) {
        ElMessage.success(t("toast.serviceRecovered"));
      }
      previousRunning = running;
    },
  );
  if (localStorage.getItem(ONBOARDING_SEEN_KEY) !== "true") {
    showOnboarding.value = true;
  }
  await registerCrashHandler();
});

onUnmounted(() => {
  runtime.stopStatusPolling();
  window.removeEventListener("goon:monitor-only-changed", handleMonitorOnlyChanged);
  unregisterCrashHandler();
  if (stopRunningWatch) {
    stopRunningWatch();
    stopRunningWatch = undefined;
  }
});
</script>

<style scoped>
.monitor-only-banner {
  display: flex;
  align-items: center;
  gap: 8px;
  background: #fffbeb;
  color: #92400e;
  border-bottom: 2px solid #f59e0b;
  padding: 6px 16px;
  font-size: 13px;
  position: sticky;
  top: 0;
  z-index: 100;
}

.monitor-only-config-link {
  color: #b45309;
  text-decoration: underline;
  font-weight: 500;
  cursor: pointer;
}

.app-header {
  display: flex;
  align-items: center;
  gap: 8px;
  border-bottom: 1px solid var(--color-border, #e5e7eb);
  padding: 0 16px;
  flex-wrap: wrap;
}

.app-title {
  font-weight: 700;
  font-size: 15px;
  margin-right: 8px;
  color: var(--color-accent, #3b82f6);
  white-space: nowrap;
}

.app-main {
  padding: 0;
  overflow: hidden;
  display: flex;
  flex-direction: column;
}

.main-tabs {
  height: 100%;
  display: flex;
  flex-direction: column;
}

.main-tabs :deep(.el-tabs__content) {
  flex: 1;
  overflow: auto;
  padding: 0 16px 16px;
}

.sub-tabs :deep(.el-tabs__content) {
  padding: 12px 0 0;
}
</style>
