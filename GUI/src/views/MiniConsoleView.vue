<template>
  <div style="padding: 12px; font-family: Segoe UI, sans-serif;">
    <div style="display:flex;justify-content:space-between;align-items:center;">
      <strong>{{ t("mini.title") }}</strong>
      <el-tag :type="runtime.status.running ? 'success' : 'danger'">
        {{ runtime.status.running ? t("mini.running") : t("mini.stopped") }}
      </el-tag>
    </div>
    <el-divider />
    <div>{{ t("mini.rpm") }}: {{ runtime.aiUsage.requestsPerMinute }}</div>
    <div>{{ t("mini.success") }}: {{ runtime.aiUsage.successRate.toFixed(2) }}%</div>
    <div>{{ t("mini.latency") }}: {{ runtime.aiUsage.avgLatencyMs.toFixed(1) }} ms</div>
    <div>{{ t("mini.health") }}: {{ runtime.health.ok ? t("mini.ok") : t("mini.notOk") }}</div>
  </div>
</template>

<script setup lang="ts">
import { onMounted, onUnmounted } from "vue";
import { useRuntimeStore } from "../stores/runtime";
import { useI18n } from "vue-i18n";

const runtime = useRuntimeStore();
const { t } = useI18n();

onMounted(() => runtime.startStatusPolling());
onUnmounted(() => runtime.stopStatusPolling());
</script>
