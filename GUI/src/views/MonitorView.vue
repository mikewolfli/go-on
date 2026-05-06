<template>
  <!-- Loading state -->
  <div v-if="runtime.loading && !runtime.status.running" style="text-align:center;padding:40px;color:#999;">
    {{ t('common.loading') }}
  </div>

  <!-- Error/Empty state -->
  <el-alert
    v-else-if="!runtime.status.running"
    :title="t('common.offlineMode')"
    :description="runtime.lastError || t('monitorView.noData')"
    type="warning"
    show-icon
    :closable="false"
  />

  <el-space v-else direction="vertical" fill style="width: 100%">
    <el-card>
      <template #header>
        <div style="display:flex;align-items:center;justify-content:space-between;gap:12px;">
          <span>{{ t("monitor.title") }}</span>
          <el-space>
            <el-tag v-if="runtime.hasStaleData" type="warning">{{ t("common.staleData") }}</el-tag>
            <el-button size="small" type="primary" :loading="selfChecking" @click="runSelfCheck">
              {{ t("monitor.runSelfCheck") }}
            </el-button>
            <el-select :model-value="runtime.heatmapWindowSeconds" style="width: 140px" @change="onWindowChange">
              <el-option :label="t('monitor.window1m')" :value="60" />
              <el-option :label="t('monitor.window5m')" :value="300" />
              <el-option :label="t('monitor.window15m')" :value="900" />
            </el-select>
          </el-space>
        </div>
      </template>
      <el-row :gutter="16" style="margin-bottom: 12px;">
        <el-col :span="8">
          <!-- el-statistic is deprecated in Element Plus but still works -->
          <el-statistic :title="t('monitor.healthScore')" :value="healthScore" suffix="/100" />
        </el-col>
        <el-col :span="8">
          <el-space>
            <span>{{ t("monitor.statusPollMs") }}</span>
            <el-input-number
              :model-value="runtime.statusPollingMs"
              :min="500"
              :max="10000"
              :step="500"
              size="small"
              controls-position="right"
              @change="onStatusPollingChange"
            />
          </el-space>
        </el-col>
        <el-col :span="8">
          <el-space>
            <span>{{ t("monitor.logsPollMs") }}</span>
            <el-input-number
              :model-value="runtime.logsPollingMs"
              :min="500"
              :max="10000"
              :step="500"
              size="small"
              controls-position="right"
              @change="onLogsPollingChange"
            />
          </el-space>
        </el-col>
      </el-row>
      <el-descriptions :column="2" border>
        <el-descriptions-item :label="t('monitor.serviceRunning')">{{ runtime.status.running }}</el-descriptions-item>
        <el-descriptions-item :label="t('monitor.healthOk')">{{ runtime.health.ok }}</el-descriptions-item>
        <el-descriptions-item :label="t('monitor.requestsPerMinute')">{{ (runtime.aiUsage.requestsPerMinute ?? 0).toLocaleString() }}</el-descriptions-item>
        <el-descriptions-item :label="t('monitor.successRate')">{{ (runtime.aiUsage.successRate ?? 0).toFixed(2) }}%</el-descriptions-item>
        <el-descriptions-item :label="t('monitor.timeoutCount')">{{ runtime.aiUsage.timeoutCount }}</el-descriptions-item>
        <el-descriptions-item :label="t('monitor.rateLimitCount')">{{ runtime.aiUsage.rateLimitCount }}</el-descriptions-item>
        <el-descriptions-item :label="t('monitor.breakerCount')">{{ runtime.aiUsage.breakerCount }}</el-descriptions-item>
        <el-descriptions-item :label="t('monitor.upstreamFailureCount')">{{ runtime.aiUsage.upstreamFailureCount }}</el-descriptions-item>
      </el-descriptions>
    </el-card>

    <el-card>
      <template #header>
        <div style="display:flex;align-items:center;justify-content:space-between;gap:12px;">
          <span>{{ t("monitor.endpointHealth") }}</span>
          <el-tag v-if="runtime.endpointHealthStatsStale" type="warning">{{ t("common.staleData") }}</el-tag>
        </div>
      </template>
      <el-table v-if="(runtime.endpointHealthStats ?? []).length > 0" :data="runtime.endpointHealthStats" size="small" height="220">
        <el-table-column prop="endpoint" :label="t('monitor.endpoint')" min-width="220" />
        <el-table-column prop="total" :label="t('monitor.total')" width="90" />
        <el-table-column prop="successRate" :label="t('monitor.successRate')" width="140">
          <template #default="scope">{{ Number(scope.row.successRate).toFixed(2) }}%</template>
        </el-table-column>
        <el-table-column prop="avgLatencyMs" :label="t('monitor.avgLatencyMs')" width="140">
          <template #default="scope">{{ Number(scope.row.avgLatencyMs).toFixed(2) }}</template>
        </el-table-column>
      </el-table>
    </el-card>

    <el-card>
      <template #header>
        <div style="display:flex;align-items:center;justify-content:space-between;gap:12px;">
          <span>{{ t("monitor.trend") }}</span>
          <el-tag v-if="runtime.usageHeatmapStale" type="warning">{{ t("common.staleData") }}</el-tag>
        </div>
      </template>
      <el-table v-if="(runtime.usageHeatmap.trend ?? []).length > 0" :data="runtime.usageHeatmap.trend" size="small" height="260">
        <el-table-column prop="secondBucket" :label="t('monitor.timeBucket')" width="160">
          <template #default="scope">{{ scope.row.secondBucket }}s</template>
        </el-table-column>
        <el-table-column prop="count" :label="t('monitor.events')" />
      </el-table>
    </el-card>

    <el-card>
      <template #header>
        <div style="display:flex;align-items:center;justify-content:space-between;gap:12px;">
          <span>{{ t("monitor.integrations") }}</span>
          <el-tag v-if="runtime.editorIntegrationsStale" type="warning">{{ t("common.staleData") }}</el-tag>
        </div>
      </template>
      <el-table v-if="(runtime.editorIntegrations ?? []).length > 0" :data="runtime.editorIntegrations" size="small" height="260">
        <el-table-column prop="editor" :label="t('monitor.editor')" width="120" />
        <el-table-column prop="interfaceName" :label="t('monitor.interfaceType')" min-width="180" />
        <el-table-column prop="protocolMode" :label="t('monitor.protocolMode')" width="120" />
        <el-table-column prop="processRunning" :label="t('monitor.process')" width="120">
          <template #default="scope">
            <el-tag :type="scope.row.processRunning ? 'success' : 'info'">
              {{ scope.row.processRunning ? t('monitor.running') : t('monitor.notRunning') }}
            </el-tag>
          </template>
        </el-table-column>
        <el-table-column prop="processCount" :label="t('monitor.instances')" width="100" />
        <el-table-column prop="transport" :label="t('monitor.transport')" min-width="180" />
        <el-table-column :label="t('monitor.endpoint')" min-width="220">
          <template #default="scope">{{ scope.row.endpoint || '-' }}</template>
        </el-table-column>
        <el-table-column :label="t('monitor.endpointStatus')" width="150">
          <template #default="scope">
            <el-tag :type="scope.row.endpointOk ? 'success' : 'warning'">
              {{ scope.row.endpointOk ? t('monitor.reachable') : t('monitor.unreachable') }}
            </el-tag>
          </template>
        </el-table-column>
        <el-table-column :label="t('monitor.addon')" width="140">
          <template #default="scope">
            <el-tag :type="scope.row.addonPresent ? 'success' : 'info'">
              {{ scope.row.addonPresent ? t('monitor.detected') : t('monitor.notDetected') }}
            </el-tag>
          </template>
        </el-table-column>
      </el-table>
    </el-card>

    <!-- Endpoint health no data -->
    <div v-if="!(runtime.endpointHealthStats ?? []).length && runtime.status.running" style="text-align:center;padding:20px;color:#999;">
      {{ t('views.MonitorView.noIntegrationData') }}
    </div>

    <!-- Trend no data -->
    <div v-if="!(runtime.usageHeatmap.trend ?? []).length && runtime.status.running" style="text-align:center;padding:20px;color:#999;">
      {{ t('views.MonitorView.noTrendData') }}
    </div>

    <!-- Integrations no data -->
    <div v-if="!(runtime.editorIntegrations ?? []).length && runtime.status.running" style="text-align:center;padding:20px;color:#999;">
      {{ t('views.MonitorView.noIntegrationData') }}
    </div>
  </el-space>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { ElMessage } from "element-plus";
import { useRuntimeStore } from "../stores/runtime";
import { useI18n } from "vue-i18n";
const runtime = useRuntimeStore();
const { t } = useI18n();
const selfChecking = ref(false);

onMounted(() => {
  if (runtime.status.running) {
    runtime.refreshAll().catch(e => console.warn("refreshAll failed:", e));
  }
});

const healthScore = computed(() => {
  let score = 0;
  if (runtime.status.running) score += 30;
  if (runtime.health.ok) score += 30;
  if (runtime.editorIntegrations.length > 0) {
    const reachable = runtime.editorIntegrations.filter((x) => x.endpointOk).length;
    score += Math.round((reachable / runtime.editorIntegrations.length) * 20);
  }
  if (runtime.endpointHealthStats.length > 0) {
    const avg = runtime.endpointHealthStats.reduce((sum, x) => sum + Number(x.successRate), 0) / runtime.endpointHealthStats.length;
    score += Math.round((avg / 100) * 20);
  }
  return Math.min(100, Math.max(0, score));
});

async function onWindowChange(value: number) {
  await runtime.setHeatmapWindow(Number(value));
}

function onStatusPollingChange(value: number | undefined) {
  if (value) {
    runtime.setStatusPollingInterval(value);
  }
}

function onLogsPollingChange(value: number | undefined) {
  if (value) {
    runtime.setLogsPollingInterval(value);
  }
}

async function runSelfCheck() {
  selfChecking.value = true;
  await runtime.refreshAll();
  selfChecking.value = false;
  ElMessage.success(`${t("monitor.selfCheckDone")} (${healthScore.value}/100)`);
}
</script>
