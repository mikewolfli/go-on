<template>
  <template v-if="isMiniRoute">
    <router-view />
  </template>
  <template v-else>
    <OfflineIndicator />
    <QuickNavigator />
    <div v-if="monitorOnly" class="monitor-only-banner">
      ⚠️ {{ t("app.monitorOnlyBanner") }}
      <router-link to="/config" class="monitor-only-config-link">{{ t("app.monitorOnlyConfigLink") }}</router-link>
    </div>
    <el-container :style="monitorOnly ? 'height: calc(100vh - 36px)' : 'height: 100vh'">
      <el-aside width="220px" style="border-right: 1px solid #e5e7eb; padding: 12px;">
        <h3>{{ t("app.name") }}</h3>
        <el-menu :default-active="activePath" router>
          <el-menu-item index="/dashboard">{{ t("menu.dashboard") }}</el-menu-item>
          <el-menu-item index="/monitor">{{ t("menu.monitor") }}</el-menu-item>
          <el-menu-item index="/ai-usage">{{ t("menu.aiUsage") }}</el-menu-item>
          <el-menu-item index="/health-breakdown">{{ t("menu.healthBreakdown") }}</el-menu-item>
          <el-menu-item index="/logs">{{ t("menu.logs") }}</el-menu-item>
          <el-menu-item index="/setup">{{ t("menu.setup") }}</el-menu-item>
          <el-menu-item index="/config">{{ t("menu.config") }}</el-menu-item>
          <el-menu-item index="/providers">{{ t("menu.providers") }}</el-menu-item>
          <el-menu-item index="/backend-ops">{{ t("menu.backendOps") }}</el-menu-item>
          <el-menu-item index="/autotune">{{ t("menu.autoTune") }}</el-menu-item>
          <el-menu-item index="/workflow">{{ t("menu.workflow") }}</el-menu-item>
          <el-menu-item index="/security">{{ t("menu.security") }}</el-menu-item>
        </el-menu>
      </el-aside>
      <el-container>
        <el-header style="display:flex;align-items:center;gap:8px;border-bottom:1px solid #e5e7eb;">
          <el-tag :type="runtime.status.running ? 'success' : 'danger'">
            {{ runtime.status.running ? t("app.serviceRunning") : t("app.serviceStopped") }}
          </el-tag>
          <el-button size="small" type="primary" @click="onStart">{{ t("app.start") }}</el-button>
          <el-button size="small" @click="onStop">{{ t("app.stop") }}</el-button>
          <el-button size="small" type="warning" @click="onRestart">{{ t("app.restart") }}</el-button>
          <el-divider direction="vertical" />
          <el-button size="small" @click="onSwitchToMiniWindow">{{ t("app.miniConsole") }}</el-button>
          <el-button size="small" @click="runtime.refreshAll">{{ t("app.refresh") }}</el-button>
          <el-button size="small" @click="toggleTheme" :title="t('app.toggleTheme')">
            {{ themeMode === 'light' ? '🌙' : '☀️' }}
          </el-button>
          <el-select :model-value="locale" size="small" style="width: 120px" @change="onLocaleChange">
            <el-option label="English" value="en-US" />
            <el-option label="简体中文" value="zh-CN" />
          </el-select>
        </el-header>
        <el-main>
          <router-view />
        </el-main>
      </el-container>
    </el-container>
  </template>
</template>

<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useRoute } from "vue-router";
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
import { currentTheme, toggleTheme as toggleThemeFunc } from "./utils/theme";
import { useCrashHandler } from "./composables/useCrashHandler";
import "./styles/dark.css";
import OfflineIndicator from "./components/OfflineIndicator.vue";
import QuickNavigator from "./components/QuickNavigator.vue";

const runtime = useRuntimeStore();
const route = useRoute();
const activePath = computed(() => route.path);
const isMiniRoute = computed(() => route.path === "/mini");
const { t } = useI18n();
const locale = ref(getLocale());
const themeMode = currentTheme;
let previousRunning = runtime.status.running;
const MONITOR_ONLY_KEY = "goon.gui.monitorOnly";
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

async function onSwitchToMiniWindow() {
  try {
    await switchToMiniWindow();
  } catch {
    // In browser preview fallback to in-window mini route.
    window.location.hash = "#/mini";
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
}
</style>
