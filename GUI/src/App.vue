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
    <ApiKeySetupDialog v-model="showApiKeySetup" @configured="onApiKeyConfigured" />
    <div v-if="monitorOnly" class="monitor-only-banner">
      ⚠️ {{ t("app.monitorOnlyBanner") }}
      <span class="monitor-only-config-link" @click="activeMainTab = 'config'">{{ t("app.monitorOnlyConfigLink") }}</span>
    </div>
    <!-- Gentle banner: backend is running but needs AI provider setup -->
    <div v-if="runtime.paused" class="api-key-banner" role="alert">
      <span class="banner-icon">✨</span>
      <span class="banner-text">
        {{ showApiKeySetup ? t('app.waitingForConfig') : t('app.providersNeeded') }}
      </span>
      <span class="banner-actions">
        <el-button size="small" type="primary" @click="onNavigateToConfig">
          {{ t('app.configureNow') }}
        </el-button>
        <el-button size="small" @click="onNavigateToConfig">
          {{ t('app.goToSettings') }}
        </el-button>
        <el-button size="small" text @click="runtime.setPaused(false)">
          {{ t('app.dismiss') }}
        </el-button>
      </span>
    </div>
    <!-- DEBUG: show current state -->
    <div style="background:#eee;padding:4px 12px;font-size:12px;font-family:monospace;display:flex;gap:16px;flex-wrap:wrap;">
      <span>loading: <b>{{ String(runtime.loading) }}</b></span>
      <span>running: <b>{{ String(runtime.status.running) }}</b></span>
      <span>paused: <b>{{ String(runtime.paused) }}</b></span>
      <span>offline: <b>{{ String(runtime.offline) }}</b></span>
      <span>lastError: <b>{{ runtime.lastError || '(none)' }}</b></span>
      <span>bootstrap: <b>{{ bootstrapResult?.status || 'pending' }}</b></span>
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
          <el-option :label="t('language.traditionalChinese')" value="zh-TW" />
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
              <el-tab-pane
                v-if="!runtime.status.running || runtime.activeFeatures.skills_enabled || runtime.activeFeatures.skills_import"
                :label="t('menu.skills')"
                name="skills"
              >
                <SkillsView />
              </el-tab-pane>
            </el-tabs>
          </el-tab-pane>

          <!-- Chat Tab -->
          <el-tab-pane :label="t('tab.chat')" name="chat">
            <keep-alive>
              <ChatView />
            </keep-alive>
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
  startService,
  stopService,
  restartService,
  switchToMiniWindow,
} from "./services/bridge";
import {
  bootstrapBackend,
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
import SkillsView from "./views/SkillsView.vue";
import SecurityView from "./views/SecurityView.vue";
import ChatView from "./views/ChatView.vue";
import OnboardingGuide from "./components/OnboardingGuide.vue";
import ApiKeySetupDialog from "./components/ApiKeySetupDialog.vue";

const runtime = useRuntimeStore();
const router = useRouter();
const route = useRoute();
const isMiniRoute = computed(() => route.path === "/mini");
const { t } = useI18n();
const locale = ref(getLocale());
const themeLabel = computed(() => currentThemeLabelKey());
const activeMainTab = ref("monitor");
const activeMonitorSubTab = ref("dashboard");
const activeConfigSubTab = ref("setup");
const showOnboarding = ref(false);
const showApiKeySetup = ref(false);
const bootstrapResult = ref<{ status: string } | null>(null);
let previousRunning = false;
const MONITOR_ONLY_KEY = "goon.gui.monitorOnly";
const ONBOARDING_SEEN_KEY = "goon.gui.onboardingSeen";
let stopRunningWatch: (() => void) | undefined;
let stopWatchMainTab: (() => void) | undefined;
let stopWatchMonitorSub: (() => void) | undefined;
let stopWatchConfigSub: (() => void) | undefined;

async function checkProviderAndNavigate() {
  if (!runtime.status.running) return;
  try {
    const { getProviderStatus } = await import("./services/rpcService");
    const status = await getProviderStatus();
    const agentInfo = status?.provider_status;
    const agents = Array.isArray(agentInfo?.configured_agents) ? agentInfo!.configured_agents! : [];
    const readyCount = agents.filter((p: any) => p?.ready === true).length;
    const summary = agentInfo?.summary;
    const configuredCount = summary?.configured ?? agents.length;
    if (readyCount === 0 && configuredCount > 0) {
      // Providers configured but not ready → show API key setup dialog
      if (!runtime.paused) {
        runtime.setPaused(true);
      }
      showApiKeySetup.value = true;
    } else if (configuredCount === 0) {
      // No providers configured — ensure paused banner is shown
      if (!runtime.paused) {
        runtime.setPaused(true);
      }
    }
  } catch (e) {
    console.warn("checkProviderAndNavigate failed:", e);
  }
}

function safeGetItem(key: string): string | null {
  try { return localStorage.getItem(key); } catch { return null; }
}
function safeSetItem(key: string, value: string) {
  try { localStorage.setItem(key, value); } catch {}
}

const monitorOnly = ref(safeGetItem(MONITOR_ONLY_KEY) === "true");
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
  if (value === "en-US" || value === "zh-CN" || value === "zh-TW") {
    setLocale(value);
    locale.value = value;
  } else {
    console.warn("Unknown locale:", value);
  }
}

function toggleTheme() {
  toggleThemeFunc();
}

function openOnboarding() {
  showOnboarding.value = true;
}

function markOnboardingSeen() {
  safeSetItem(ONBOARDING_SEEN_KEY, "true");
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
    router.push("/mini");
  }
}

async function onStart() {
  try {
    await startBackendWithChecks();
    await runtime.refreshAll();
    runtime.startStatusPolling();
    await checkProviderAndNavigate();
    ElMessage.success(t("toast.serviceStarted"));
  } catch (error) {
    ElMessage.error(String(error));
  }
}

async function onStop() {
  try {
    await stopService();
    runtime.setPaused(true);
    await runtime.refreshAll();
    ElMessage.success(t("toast.serviceStopped"));
  } catch (error) {
    ElMessage.error(String(error));
  }
}

async function onRestart() {
  try {
    await restartService();
    await runtime.refreshAll();
    runtime.startStatusPolling();
    ElMessage.success(t("toast.serviceRestarted"));
  } catch (error) {
    ElMessage.error(String(error));
  }
}

async function onApiKeyConfigured() {
  showApiKeySetup.value = false;
  let started = false;
  try {
    await restartService();
    started = true;
  } catch {
    try {
      await startService();
      started = true;
    } catch (e) {
      console.warn("start after api key config failed:", e);
    }
  }
  if (started) {
    const { waitForBackendHealthy } = await import("./services/backendLifecycle");
    const healthy = await waitForBackendHealthy(15000);
    if (healthy) {
      runtime.startStatusPolling();
    }
    await runtime.refreshAll();
    await checkProviderAndNavigate();
  }
  // Only unpause if backend is actually running
  if (runtime.status.running && runtime.paused) {
    runtime.setPaused(false);
  }
}

function onNavigateToConfig() {
  activeMainTab.value = 'config';
  activeConfigSubTab.value = 'providers';
}

onMounted(async () => {
  if (isMiniRoute.value) return;
  bootstrapResult.value = null;
  try {
    const result = await bootstrapBackend(monitorOnlyModeEnabled());
    bootstrapResult.value = result;

    switch (result.status) {
      case "configured":
        try {
          await startBackendWithChecks();
          previousRunning = true;
        } catch (error) {
          ElMessage.error(String(error));
        }
        break;

      case "no-providers":
        previousRunning = false;
        runtime.setPaused(true);
        break;

      case "no-backend":
        ElMessage.warning(result.reason);
        previousRunning = false;
        break;

      case "monitor-only":
        previousRunning = false;
        break;
    }
  } catch (error) {
    console.error("Bootstrap failed:", error);
    ElMessage.error(String(error));
  }

  monitorOnly.value = monitorOnlyModeEnabled();
  window.addEventListener("goon:monitor-only-changed", handleMonitorOnlyChanged);

  // Check provider readiness (only runs if backend is running)
  await checkProviderAndNavigate();

  // Watch for backend status changes (debounced to prevent races)
  let watchPending = false;
  stopRunningWatch = watch(
    () => runtime.status.running,
    async (running) => {
      if (watchPending) return;
      watchPending = true;
      try {
        if (!previousRunning && running) {
          ElMessage.success(t("toast.serviceRecovered"));
          await checkProviderAndNavigate();
        } else if (previousRunning && !running && !runtime.paused) {
          runtime.setPaused(true);
          ElMessage.warning(t("toast.serviceStopped"));
        }
      } finally {
        previousRunning = running;
        watchPending = false;
      }
    },
  );

  // Only start polling if backend is expected to be running
  if (bootstrapResult.value?.status === "configured") {
    runtime.startStatusPolling();
  }
  // Show onboarding guide only when status is "configured" (backend running with providers)
  // For no-providers, no-backend, monitor-only: the banner is sufficient.
  if (safeGetItem(ONBOARDING_SEEN_KEY) !== "true" && bootstrapResult.value?.status === "configured") {
    showOnboarding.value = true;
  }
  // Restore tab state on mount
  const savedMainTab = safeGetItem("goon.gui.activeMainTab");
  if (savedMainTab) activeMainTab.value = savedMainTab;
  const savedMonitorSubTab = safeGetItem("goon.gui.activeMonitorSubTab");
  if (savedMonitorSubTab) activeMonitorSubTab.value = savedMonitorSubTab;
  const savedConfigSubTab = safeGetItem("goon.gui.activeConfigSubTab");
  if (savedConfigSubTab) activeConfigSubTab.value = savedConfigSubTab;

  // Watch and persist tab state
  stopWatchMainTab = watch(activeMainTab, (val) => safeSetItem("goon.gui.activeMainTab", val));
  stopWatchMonitorSub = watch(activeMonitorSubTab, (val) => safeSetItem("goon.gui.activeMonitorSubTab", val));
  stopWatchConfigSub = watch(activeConfigSubTab, (val) => safeSetItem("goon.gui.activeConfigSubTab", val));

  await registerCrashHandler();
});

onUnmounted(() => {
  runtime.stopStatusPolling();
  window.removeEventListener("goon:monitor-only-changed", handleMonitorOnlyChanged);
  unregisterCrashHandler();
  if (stopWatchMainTab) {
    stopWatchMainTab();
    stopWatchMainTab = undefined;
  }
  if (stopWatchMonitorSub) {
    stopWatchMonitorSub();
    stopWatchMonitorSub = undefined;
  }
  if (stopWatchConfigSub) {
    stopWatchConfigSub();
    stopWatchConfigSub = undefined;
  }
  if (stopRunningWatch) {
    stopRunningWatch();
    stopRunningWatch = undefined;
  }
});
</script>

<style scoped>
.api-key-banner {
  display: flex;
  align-items: center;
  gap: 6px;
  background: #fffbeb;
  color: #92400e;
  border-bottom: 2px solid #f59e0b;
  padding: 8px 16px;
  font-size: 13px;
  position: sticky;
  top: 0;
  z-index: 100;
  flex-wrap: wrap;
}

.banner-icon {
  flex-shrink: 0;
  font-size: 16px;
}

.banner-text {
  flex: 1 1 auto;
  min-width: 160px;
}

.banner-actions {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
}

.paused-overlay-content {
  text-align: center;
  max-width: 400px;
  padding: 40px;
}

.paused-icon {
  font-size: 48px;
  margin-bottom: 16px;
}

.paused-overlay-content h2 {
  margin: 0 0 8px;
  font-size: 20px;
  color: #1f2937;
}

.paused-overlay-content p {
  margin: 0 0 24px;
  color: #6b7280;
  font-size: 14px;
  line-height: 1.5;
}

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
