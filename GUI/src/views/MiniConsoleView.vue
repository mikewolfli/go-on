<template>
  <div class="mini-root">
    <div class="mini-head">
      <strong>{{ t("mini.title") }}</strong>
      <el-tag size="small" :type="runtime.status.running ? 'success' : 'danger'">
        {{ runtime.status.running ? t("mini.running") : t("mini.stopped") }}
      </el-tag>
    </div>
    <div class="mini-grid">
      <div class="metric"><span>{{ t("mini.rpm") }}</span><b>{{ runtime.aiUsage.requestsPerMinute }}</b></div>
      <div class="metric"><span>{{ t("mini.success") }}</span><b>{{ runtime.aiUsage.successRate.toFixed(2) }}%</b></div>
      <div class="metric"><span>{{ t("mini.latency") }}</span><b>{{ runtime.aiUsage.avgLatencyMs.toFixed(1) }} ms</b></div>
      <div class="metric"><span>{{ t("mini.health") }}</span><b>{{ runtime.health.ok ? t("mini.ok") : t("mini.notOk") }}</b></div>
    </div>
    <div class="mini-actions">
      <el-button size="small" type="primary" @click="backToMainWindow">{{ t("mini.backToMain") }}</el-button>
      <el-button size="small" @click="openMainRoute">{{ t("mini.openFullInCurrent") }}</el-button>
      <el-button size="small" @click="runtime.refreshAll">{{ t("mini.refresh") }}</el-button>
    </div>
    <div class="mini-statusbar">
      <span>{{ t("mini.statusbar") }}</span>
      <span>PID {{ runtime.status.pid ?? '-' }}</span>
      <span>Uptime {{ runtime.status.uptimeSeconds ?? 0 }}s</span>
    </div>
  </div>
</template>

<script setup lang="ts">
import { onMounted, onUnmounted } from "vue";
import { useRouter } from "vue-router";
import { useRuntimeStore } from "../stores/runtime";
import { useI18n } from "vue-i18n";
import { switchToMainWindow } from "../services/bridge";

const runtime = useRuntimeStore();
const { t } = useI18n();
const router = useRouter();

async function backToMainWindow() {
  try {
    await switchToMainWindow();
  } catch {
    await router.push("/dashboard");
  }
}

async function openMainRoute() {
  await router.push("/dashboard");
}

onMounted(() => runtime.startStatusPolling());
onUnmounted(() => runtime.stopStatusPolling());
</script>

<style scoped>
.mini-root {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 12px;
  height: 100vh;
  box-sizing: border-box;
  font-family: "Segoe UI", sans-serif;
}

.mini-head {
  display: flex;
  justify-content: space-between;
  align-items: center;
}

.mini-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 8px;
}

.metric {
  border: 1px solid #e5e7eb;
  border-radius: 8px;
  padding: 8px;
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: 12px;
}

.metric b {
  font-size: 13px;
}

.mini-actions {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}

.mini-statusbar {
  margin-top: auto;
  font-size: 12px;
  color: #6b7280;
  display: flex;
  gap: 12px;
  border-top: 1px dashed #d1d5db;
  padding-top: 8px;
}
</style>
