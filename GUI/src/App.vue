<template>
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
import {
  autoConfigureBackendPath,
  backendExecutableExists,
  checkHealth,
  configureServiceByExecutable,
  exitApp,
  startService,
  stopService,
  restartService,
  serviceStatus,
  showMiniConsole,
} from "./services/bridge";
import { openDialog } from "./services/dialog";
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
const MONITOR_ONLY_KEY = "goon.gui.monitorOnly";

const monitorOnly = ref(localStorage.getItem(MONITOR_ONLY_KEY) === "true");

function monitorOnlyModeEnabled(): boolean {
  return monitorOnly.value;
}

function handleMonitorOnlyChanged(e: Event) {
  monitorOnly.value = (e as CustomEvent<boolean>).detail;
}

function classifyStartupError(error: unknown): string {
  const raw = String(error).toLowerCase();
  if (raw.includes("startup_error:file_missing")) {
    return "启动失败：未找到后台可执行文件，请重新选择路径。";
  }
  if (raw.includes("startup_error:not_a_file")) {
    return "启动失败：配置路径不是可执行文件。";
  }
  if (raw.includes("startup_error:permission_denied")) {
    return "启动失败：没有执行权限，请检查文件权限或以管理员身份运行。";
  }
  if (raw.includes("startup_error:exited_early")) {
    return "启动失败：后台进程启动后立即退出，请检查日志和端口占用。";
  }
  if (raw.includes("startup_error:spawn_failed")) {
    return "启动失败：无法拉起后台进程，请检查依赖与运行环境。";
  }
  return `启动失败：${String(error)}`;
}

async function waitForBackendHealthy(timeoutMs = 12000): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const status = await serviceStatus();
      if (!status.running) {
        return false;
      }
      const health = await checkHealth();
      if (health.ok) {
        return true;
      }
    } catch {
      // Continue polling until timeout.
    }
    await new Promise((resolve) => window.setTimeout(resolve, 800));
  }
  return false;
}

async function startBackendWithChecks() {
  try {
    await startService();
  } catch (error) {
    throw new Error(classifyStartupError(error));
  }

  const healthy = await waitForBackendHealthy();
  if (!healthy) {
    throw new Error("启动超时：后台进程未在 12 秒内就绪，请检查端口、配置或依赖。");
  }
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

async function ensureBackendAndStart() {
  while (true) {
    const exists = await backendExecutableExists();
    if (exists) {
      await startBackendWithChecks();
      return;
    }

    await ElMessageBox.alert(
      "未找到后台程序 go-on，请选择后台可执行文件。取消将直接退出 GUI。",
      "配置后台路径",
      {
        confirmButtonText: "选择文件",
        closeOnClickModal: false,
        closeOnPressEscape: false,
      },
    );

    const picked = await openDialog({
      multiple: false,
      directory: false,
      title: "选择 go-on 后台可执行文件",
      filters: [
        { name: "Executable", extensions: ["exe", "bin", ""] },
        { name: "All Files", extensions: ["*"] },
      ],
    });

    if (!picked) {
      await exitApp();
      return;
    }

    const inputPath = Array.isArray(picked) ? picked[0] : picked;
    if (!inputPath || !String(inputPath).trim()) {
      ElMessage.warning("路径不能为空，请重新指定。")
      continue;
    }

    await configureServiceByExecutable(String(inputPath));

    const configuredExists = await backendExecutableExists();
    if (!configuredExists) {
      ElMessage.error("指定路径无效或文件不存在，请重新指定。")
      continue;
    }

    await startBackendWithChecks();
    ElMessage.success("后台已启动。")
    return;
  }
}

async function bootstrapBackend() {
  const hasConfiguredPath = await backendExecutableExists();
  if (hasConfiguredPath) {
    return;
  }

  try {
    const health = await checkHealth();
    if (health.ok) {
      const result = await autoConfigureBackendPath();
      if (result.linked) {
        ElMessage.success("检测到后台已运行，已自动关联并写入配置。")
        return;
      }
      ElMessage.warning(`检测到后台在运行，但自动关联失败：${result.reason}`);
    }
  } catch {
    // Ignore health probe failures and continue to manual path flow.
  }

  if (monitorOnlyModeEnabled()) {
    ElMessage.warning("当前为仅监控模式：不会自动启动后台，请先手动启动 go-on。")
    return;
  }

  await ensureBackendAndStart();
}

onMounted(async () => {
  try {
    await bootstrapBackend();
  } catch (error) {
    ElMessage.error(String(error));
  }

  monitorOnly.value = monitorOnlyModeEnabled();
  window.addEventListener("goon:monitor-only-changed", handleMonitorOnlyChanged);

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
  window.removeEventListener("goon:monitor-only-changed", handleMonitorOnlyChanged);
  if (unlistenCrash) {
    unlistenCrash();
    unlistenCrash = undefined;
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
