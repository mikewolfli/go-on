<template>
  <el-row :gutter="16">
    <el-col :span="12">
      <el-card>
        <template #header>
          <div style="display:flex;align-items:center;justify-content:space-between;gap:12px;">
            <span>{{ t("aiUsage.snapshot") }}</span>
            <el-tag v-if="runtime.aiUsageStale" type="warning">{{ t("common.staleData") }}</el-tag>
          </div>
        </template>
        <el-descriptions :column="1" border>
          <el-descriptions-item :label="t('aiUsage.timestamp')">{{ runtime.aiUsage.timestamp }}</el-descriptions-item>
          <el-descriptions-item :label="t('aiUsage.requestsPerMinute')">{{ runtime.aiUsage.requestsPerMinute }}</el-descriptions-item>
          <el-descriptions-item :label="t('aiUsage.successRate')">{{ runtime.aiUsage.successRate.toFixed(2) }}%</el-descriptions-item>
          <el-descriptions-item :label="t('aiUsage.avgLatency')">{{ runtime.aiUsage.avgLatencyMs.toFixed(2) }} ms</el-descriptions-item>
        </el-descriptions>
      </el-card>
    </el-col>
    <el-col :span="12">
      <el-card>
        <template #header>{{ t("aiUsage.failureBreakdown") }}</template>
        <el-descriptions :column="1" border>
          <el-descriptions-item :label="t('aiUsage.timeout')">{{ runtime.aiUsage.timeoutCount }}</el-descriptions-item>
          <el-descriptions-item :label="t('aiUsage.rateLimit')">{{ runtime.aiUsage.rateLimitCount }}</el-descriptions-item>
          <el-descriptions-item :label="t('aiUsage.breakerTriggered')">{{ runtime.aiUsage.breakerCount }}</el-descriptions-item>
          <el-descriptions-item :label="t('aiUsage.upstreamFailure')">{{ runtime.aiUsage.upstreamFailureCount }}</el-descriptions-item>
        </el-descriptions>
      </el-card>
    </el-col>
    <el-col :span="24" style="margin-top:16px;">
      <el-card>
        <template #header>
          <div style="display:flex;align-items:center;justify-content:space-between;gap:12px;">
            <span>{{ t("aiUsage.trend") }}</span>
            <el-tag v-if="runtime.usageHeatmapStale" type="warning">{{ t("common.staleData") }}</el-tag>
          </div>
        </template>
        <el-table :data="runtime.usageHeatmap.trend" size="small" height="220">
          <el-table-column prop="secondBucket" :label="t('aiUsage.timeBucket')" width="160">
            <template #default="scope">{{ scope.row.secondBucket }}s</template>
          </el-table-column>
          <el-table-column prop="count" :label="t('aiUsage.events')" />
        </el-table>
      </el-card>
    </el-col>
    <el-col :span="12" style="margin-top:16px;">
      <el-card>
        <template #header>{{ t("aiUsage.phaseHeat") }}</template>
        <el-table :data="runtime.usageHeatmap.phaseTop" size="small" height="240">
          <el-table-column prop="name" :label="t('aiUsage.dimension')" />
          <el-table-column prop="count" :label="t('aiUsage.count')" width="120" />
        </el-table>
      </el-card>
    </el-col>
    <el-col :span="12" style="margin-top:16px;">
      <el-card>
        <template #header>{{ t("aiUsage.agentHeat") }}</template>
        <el-table :data="runtime.usageHeatmap.agentTop" size="small" height="240">
          <el-table-column prop="name" :label="t('aiUsage.dimension')" />
          <el-table-column prop="count" :label="t('aiUsage.count')" width="120" />
        </el-table>
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
