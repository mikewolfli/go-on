<template>
  <!-- Loading state -->
  <div v-if="runtime.loading && !runtime.status.running" style="text-align:center;padding:40px;color:#999;">
    <el-icon class="is-loading" style="font-size:24px;margin-bottom:12px;"><svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4" fill="none" stroke-dasharray="31.4 31.4" stroke-linecap="round"><animateTransform attributeName="transform" type="rotate" from="0 12 12" to="360 12 12" dur="1s" repeatCount="indefinite"/></circle></svg></el-icon>
    <div>{{ t("dashboard.loading") }}</div>
  </div>

  <!-- Error state -->
  <el-alert
    v-else-if="runtime.lastError && !runtime.status.running"
    :title="t('common.offlineMode')"
    :description="runtime.lastError"
    type="error"
    show-icon
    :closable="false"
  />

  <!-- Empty state (backend not running, no data) -->
  <div v-else-if="!runtime.status.running && !runtime.loading" style="text-align:center;padding:40px;color:#999;">
    {{ t("dashboard.noData") }}
  </div>

  <!-- Normal data display -->
  <el-row v-else :gutter="16">
    <el-col :span="8">
      <el-card>
        <template #header>{{ t("dashboard.service") }}</template>
        <div>{{ t("dashboard.running") }}: {{ runtime.status.running }}</div>
        <div>{{ t("dashboard.pid") }}: {{ runtime.status.pid ?? '-' }}</div>
        <div>{{ t("dashboard.uptime") }}: {{ (runtime.status.uptimeSeconds ?? 0).toLocaleString() }}s</div>
      </el-card>
    </el-col>
    <el-col :span="8">
      <el-card>
        <template #header>
          <div style="display:flex;align-items:center;justify-content:space-between;gap:12px;">
            <span>{{ t("dashboard.health") }}</span>
            <el-tag v-if="runtime.healthStale" type="warning">{{ t("common.staleData") }}</el-tag>
          </div>
        </template>
        <div>{{ t("dashboard.ok") }}: {{ runtime.health.ok }}</div>
        <div>{{ t("dashboard.endpoint") }}: {{ runtime.health.endpoint }}</div>
        <div>{{ t("dashboard.code") }}: {{ runtime.health.responseCode ?? '-' }}</div>
      </el-card>
    </el-col>
    <el-col :span="8">
      <el-card>
        <template #header>
          <div style="display:flex;align-items:center;justify-content:space-between;gap:12px;">
            <span>{{ t("dashboard.ai") }}</span>
            <el-tag v-if="runtime.aiUsageStale" type="warning">{{ t("common.staleData") }}</el-tag>
          </div>
        </template>
        <div>{{ t("dashboard.rpm") }}: {{ (runtime.aiUsage.requestsPerMinute ?? 0).toLocaleString() }}</div>
        <div>{{ t("dashboard.success") }}: {{ (runtime.aiUsage.successRate ?? 0).toFixed(2) }}%</div>
        <div>{{ t("dashboard.latency") }}: {{ (runtime.aiUsage.avgLatencyMs ?? 0).toFixed(1) }} ms</div>
      </el-card>
    </el-col>
  </el-row>
</template>

<script setup lang="ts">
import { onMounted } from "vue";
import { useRuntimeStore } from "../stores/runtime";
import { useI18n } from "vue-i18n";
const runtime = useRuntimeStore();
const { t } = useI18n();
onMounted(() => {
  runtime.refreshAll();
});
</script>
