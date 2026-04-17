<template>
  <el-row :gutter="16">
    <el-col :span="8">
      <el-card>
        <template #header>{{ t("dashboard.service") }}</template>
        <div>{{ t("dashboard.running") }}: {{ runtime.status.running }}</div>
        <div>{{ t("dashboard.pid") }}: {{ runtime.status.pid ?? '-' }}</div>
        <div>{{ t("dashboard.uptime") }}: {{ runtime.status.uptimeSeconds ?? 0 }}</div>
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
        <div>{{ t("dashboard.rpm") }}: {{ runtime.aiUsage.requestsPerMinute }}</div>
        <div>{{ t("dashboard.success") }}: {{ runtime.aiUsage.successRate.toFixed(2) }}%</div>
        <div>{{ t("dashboard.latency") }}: {{ runtime.aiUsage.avgLatencyMs.toFixed(1) }} ms</div>
      </el-card>
    </el-col>
  </el-row>
</template>

<script setup lang="ts">
import { useRuntimeStore } from "../stores/runtime";
import { useI18n } from "vue-i18n";
const runtime = useRuntimeStore();
const { t } = useI18n();
</script>
