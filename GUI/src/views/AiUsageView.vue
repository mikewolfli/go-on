<template>
  <!-- Loading state -->
  <div v-if="runtime.loading && !runtime.status.running" style="text-align:center;padding:40px;color:#999;">
    {{ t('common.loading') || 'Loading...' }}
  </div>

  <!-- Error/Empty state (backend not running) -->
  <el-alert
    v-else-if="!runtime.status.running"
    :title="t('common.offlineMode')"
    :description="runtime.lastError || t('aiUsageView.noSnapshotData')"
    type="warning"
    show-icon
    :closable="false"
  />

  <el-row v-else :gutter="16">
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
          <el-descriptions-item :label="t('aiUsage.requestsPerMinute')">{{ (runtime.aiUsage.requestsPerMinute ?? 0).toLocaleString() }}</el-descriptions-item>
          <el-descriptions-item :label="t('aiUsage.successRate')">{{ (runtime.aiUsage.successRate ?? 0).toFixed(2) }}%</el-descriptions-item>
          <el-descriptions-item :label="t('aiUsage.avgLatency')">{{ (runtime.aiUsage.avgLatencyMs ?? 0).toFixed(2) }} ms</el-descriptions-item>
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
        <el-table v-if="(runtime.usageHeatmap.trend ?? []).length > 0" :data="runtime.usageHeatmap.trend" size="small" height="220">
          <el-table-column prop="secondBucket" :label="t('aiUsage.timeBucket')" width="160">
            <template #default="scope">{{ scope.row?.secondBucket ?? '-' }}s</template>
          </el-table-column>
          <el-table-column prop="count" :label="t('aiUsage.events')" />
        </el-table>
        <div v-else style="text-align:center;padding:20px;color:#999;">{{ t('aiUsageView.noTrendData') || 'No trend data' }}</div>
      </el-card>
    </el-col>
    <el-col :span="12" style="margin-top:16px;">
      <el-card>
        <template #header>{{ t("aiUsage.phaseHeat") }}</template>
        <el-table v-if="(runtime.usageHeatmap.phaseTop ?? []).length > 0" :data="runtime.usageHeatmap.phaseTop" size="small" height="240">
          <el-table-column prop="name" :label="t('aiUsage.dimension')" />
          <el-table-column prop="count" :label="t('aiUsage.count')" width="120" />
        </el-table>
        <div v-else style="text-align:center;padding:20px;color:#999;">{{ t('aiUsageView.noPhaseData') || 'No phase data' }}</div>
      </el-card>
    </el-col>
    <el-col :span="12" style="margin-top:16px;">
      <el-card>
        <template #header>{{ t("aiUsage.agentHeat") }}</template>
        <el-table v-if="(runtime.usageHeatmap.agentTop ?? []).length > 0" :data="runtime.usageHeatmap.agentTop" size="small" height="240">
          <el-table-column prop="name" :label="t('aiUsage.dimension')" />
          <el-table-column prop="count" :label="t('aiUsage.count')" width="120" />
        </el-table>
        <div v-else style="text-align:center;padding:20px;color:#999;">{{ t('aiUsageView.noAgentData') || 'No agent data' }}</div>
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
