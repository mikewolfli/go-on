<template>
  <el-space direction="vertical" fill style="width: 100%">
    <el-card>
      <template #header>{{ t("autoTune.title") }}</template>

      <el-space direction="vertical" fill style="width: 100%">
        <el-text>{{ t("autoTune.hint") }}</el-text>

        <el-space>
          <el-button type="primary" @click="refreshStatus" :loading="loading">
            {{ t("autoTune.refresh") }}
          </el-button>
          <el-button type="warning" @click="resetTuning" :loading="resetting">
            {{ t("autoTune.reset") }}
          </el-button>
        </el-space>

        <!-- 调优状态 -->
        <el-card shadow="hover">
          <template #header>
            <div style="display: flex; align-items: center; justify-content: space-between; width: 100%;">
              <span>{{ t("autoTune.tuningStatus") }}</span>
              <el-tag :type="tuningEnabled ? 'success' : 'info'">
                {{ tuningEnabled ? t("autoTune.enabled") : t("autoTune.disabled") }}
              </el-tag>
            </div>
          </template>
          <el-descriptions :columns="2" border>
            <el-descriptions-item :label="t('autoTune.enabled')">
              {{ tuningEnabled ? t("common.yes") : t("common.no") }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('autoTune.uptime')">
              {{ tuningUptime }}h
            </el-descriptions-item>
            <el-descriptions-item :label="t('autoTune.adjustments')">
              {{ adjustmentCount }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('autoTune.improvement')">
              {{ improvement }}%
            </el-descriptions-item>
          </el-descriptions>
        </el-card>

        <!-- 当前配置 -->
        <el-card shadow="hover">
          <template #header>
            <span>{{ t("autoTune.currentConfig") }}</span>
          </template>
          <el-table :data="currentConfig" border stripe>
            <el-table-column prop="name" :label="t('common.name')" width="200" />
            <el-table-column prop="value" :label="t('autoTune.current')" width="150" />
            <el-table-column prop="baseline" :label="t('autoTune.baseline')" width="150" />
            <el-table-column prop="delta" :label="t('autoTune.delta')" width="120" />
            <el-table-column prop="impact" :label="t('autoTune.impact')" width="120">
              <template #default="{ row }">
                <el-tag
                  :type="
                    row.impact === 'positive'
                      ? 'success'
                      : row.impact === 'negative'
                        ? 'danger'
                        : 'info'
                  "
                >
                  {{ row.impact }}
                </el-tag>
              </template>
            </el-table-column>
          </el-table>
        </el-card>

        <!-- 调优历史 -->
        <el-card shadow="hover">
          <template #header>
            <span>{{ t("autoTune.history") }}</span>
          </template>
          <el-timeline>
            <el-timeline-item
              v-for="(event, idx) in tuningHistory"
              :key="idx"
              :timestamp="event.time"
              :type="event.type"
              placement="top"
            >
              <p>
                <strong>{{ event.adjustment }}</strong>: {{ event.result }}
              </p>
            </el-timeline-item>
          </el-timeline>
        </el-card>

        <!-- 建议 -->
        <el-card shadow="hover" v-if="recommendations.length > 0">
          <template #header>
            <span>{{ t("autoTune.recommendations") }}</span>
          </template>
          <el-alert
            v-for="(rec, idx) in recommendations"
            :key="idx"
            :title="rec.title"
            :description="rec.description"
            :type="rec.type"
            closable
            style="margin-bottom: 8px"
          />
        </el-card>

        <el-divider />

        <!-- Raw JSON Output -->
        <el-collapse>
          <el-collapse-item :title="t('common.advancedInfo')" name="1">
            <el-input v-model="rawOutput" type="textarea" :rows="12" readonly />
          </el-collapse-item>
        </el-collapse>
      </el-space>
    </el-card>
  </el-space>
</template>

<script setup lang="ts">
import { ref, reactive } from "vue";
import { ElMessage, ElMessageBox } from "element-plus";
import { useI18n } from "vue-i18n";
import { invokeRuntimeRpc } from "../services/bridge";

const { t } = useI18n();
const loading = ref(false);
const resetting = ref(false);

const tuningEnabled = ref(true);
const tuningUptime = ref(48);
const adjustmentCount = ref(12);
const improvement = ref(15.3);

const currentConfig = ref([
  { name: "batch_size", value: 32, baseline: 16, delta: "+100%", impact: "positive" },
  { name: "worker_count", value: 8, baseline: 4, delta: "+100%", impact: "positive" },
  { name: "cache_ttl", value: 300, baseline: 60, delta: "+400%", impact: "positive" },
  { name: "timeout_ms", value: 5000, baseline: 3000, delta: "+67%", impact: "negative" },
]);

const tuningHistory = ref([
  { time: "2m ago", type: "success", adjustment: "batch_size: 16→32", result: "Throughput +12%" },
  { time: "5m ago", type: "success", adjustment: "worker_count: 4→8", result: "Latency -8%" },
  { time: "10m ago", type: "info", adjustment: "cache_ttl: 60→300", result: "Hit rate +5%" },
]);

const recommendations = reactive([
  {
    type: "success",
    title: t("autoTune.recommendation1"),
    description: t("autoTune.recommendationDesc1"),
  },
  {
    type: "warning",
    title: t("autoTune.recommendation2"),
    description: t("autoTune.recommendationDesc2"),
  },
]);

const rawOutput = ref("");

async function refreshStatus() {
  loading.value = true;
  try {
    const result = await invokeRuntimeRpc("autotune.get", "{}");
    rawOutput.value = result;

    const data = JSON.parse(result);
    if (data.ok) {
      tuningEnabled.value = data.enabled || true;
      tuningUptime.value = data.uptime_hours || 0;
      adjustmentCount.value = data.adjustment_count || 0;
      improvement.value = data.improvement_percent || 0;

      if (data.config && Array.isArray(data.config)) {
        currentConfig.value = data.config;
      }

      if (data.history && Array.isArray(data.history)) {
        tuningHistory.value = data.history;
      }
    }

    ElMessage.success(t("common.refreshed"));
  } catch (err) {
    ElMessage.error(`Error: ${err}`);
  } finally {
    loading.value = false;
  }
}

async function resetTuning() {
  ElMessageBox.confirm(
    t("autoTune.confirmReset"),
    t("autoTune.resetWarning"),
    {
      confirmButtonText: t("common.confirm"),
      cancelButtonText: t("common.cancel"),
      type: "warning",
    }
  ).then(async () => {
    resetting.value = true;
    try {
      await invokeRuntimeRpc("autotune.reset", "{}");
      ElMessage.success(t("autoTune.resetSuccess"));
      await refreshStatus();
    } catch (err) {
      ElMessage.error(`Error: ${err}`);
    } finally {
      resetting.value = false;
    }
  }).catch(() => {
    ElMessage.info(t("common.cancelled"));
  });
}

// Initialize
refreshStatus();
</script>
