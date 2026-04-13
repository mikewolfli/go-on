<template>
  <OfflineIndicator />
  <QuickNavigator />
  <el-container style="height: 100vh">
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
        <el-button size="small" @click="showMiniConsole">{{ t("app.miniConsole") }}</el-button>
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

<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useRoute } from "vue-router";
import { ElMessage, ElMessageBox } from "element-plus";
import { useI18n } from "vue-i18n";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { startService, stopService, restartService, showMiniConsole } from "./services/bridge";
import { useRuntimeStore } from "./stores/runtime";
import { getLocale, setLocale } from "./locales";
import { currentTheme, toggleTheme as toggleThemeFunc } from "./utils/theme";
import "./styles/dark.css";
import OfflineIndicator from "./components/OfflineIndicator.vue";
import QuickNavigator from "./components/QuickNavigator.vue";

const runtime = useRuntimeStore();
const route = useRoute();
const activePath = computed(() => route.path);
const { t } = useI18n();
const locale = ref(getLocale());
const themeMode = currentTheme;
let unlistenCrash: UnlistenFn | undefined;
const crashCooldownMs = 60000;
let lastCrashKey = "";
let lastCrashAt = 0;
let previousRunning = runtime.status.running;

function onLocaleChange(value: string) {
  if (value === "en-US" || value === "zh-CN") {
    setLocale(value);
    locale.value = value;
  }
}

function toggleTheme() {
  toggleThemeFunc();
}

async function onStart() {
  try {
    await startService();
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
    await restartService();
    await runtime.refreshAll();
    ElMessage.success(t("toast.serviceRestarted"));
  } catch (error) {
    ElMessage.error(String(error));
  }
}

onMounted(() => {
  runtime.startStatusPolling();

  watch(
    () => runtime.status.running,
    (running) => {
      if (!previousRunning && running) {
        ElMessage.success(t("toast.serviceRecovered"));
      }
      previousRunning = running;
    },
  );

  listen<{ message: string; timestamp: string }>("service-crash", async (event) => {
    const payload = event.payload;
    const now = Date.now();
    const crashKey = payload.message;
    if (crashKey === lastCrashKey && now - lastCrashAt < crashCooldownMs) {
      return;
    }
    lastCrashKey = crashKey;
    lastCrashAt = now;

    try {
      await ElMessageBox.confirm(
        `${payload.message}\n${t("toast.recoverPrompt")}`,
        t("toast.serviceCrashed"),
        {
          confirmButtonText: t("toast.recoverNow"),
          cancelButtonText: t("toast.later"),
          type: "error",
        },
      );
      await onRestart();
    } catch {
      ElMessage.warning(t("toast.recoverDeferred"));
    }
  }).then((off) => {
    unlistenCrash = off;
  });
});

onUnmounted(() => {
  runtime.stopStatusPolling();
  if (unlistenCrash) {
    unlistenCrash();
    unlistenCrash = undefined;
  }
});
</script>
