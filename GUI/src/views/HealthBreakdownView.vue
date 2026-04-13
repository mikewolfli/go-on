<template>
  <el-space direction="vertical" fill style="width: 100%">
    <el-card>
      <template #header>{{ t("healthBreakdown.title") }}</template>

      <el-space direction="vertical" fill style="width: 100%">
        <el-text>{{ t("healthBreakdown.hint") }}</el-text>

        <el-button type="primary" @click="refreshBreakdown" :loading="loading">
          {{ t("healthBreakdown.refresh") }}
        </el-button>

        <!-- Cache 健康 -->
        <el-card shadow="hover">
          <template #header>
            <div style="display: flex; align-items: center; justify-content: space-between; width: 100%;">
              <span>{{ t("healthBreakdown.cache") }}</span>
              <el-tag :type="cacheStatus.type">{{ cacheStatus.text }}</el-tag>
            </div>
          </template>
          <el-descriptions :columns="2" border>
            <el-descriptions-item :label="t('healthBreakdown.status')">
              {{ cacheStatus.text }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('healthBreakdown.hitRate')">
              {{ cacheStatus.hitRate }}%
            </el-descriptions-item>
            <el-descriptions-item :label="t('healthBreakdown.size')">
              {{ cacheStatus.size }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('healthBreakdown.lastUpdate')">
              {{ cacheStatus.lastUpdate }}
            </el-descriptions-item>
          </el-descriptions>
        </el-card>

        <!-- Vector 库健康 -->
        <el-card shadow="hover">
          <template #header>
            <div style="display: flex; align-items: center; justify-content: space-between; width: 100%;">
              <span>{{ t("healthBreakdown.vector") }}</span>
              <el-tag :type="vectorStatus.type">{{ vectorStatus.text }}</el-tag>
            </div>
          </template>
          <el-descriptions :columns="2" border>
            <el-descriptions-item :label="t('healthBreakdown.status')">
              {{ vectorStatus.text }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('healthBreakdown.dimensions')">
              {{ vectorStatus.dimensions }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('healthBreakdown.vectors')">
              {{ vectorStatus.vectors }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('healthBreakdown.lastUpdate')">
              {{ vectorStatus.lastUpdate }}
            </el-descriptions-item>
          </el-descriptions>
        </el-card>

        <!-- Breaker 状态 -->
        <el-card shadow="hover">
          <template #header>
            <div style="display: flex; align-items: center; justify-content: space-between; width: 100%;">
              <span>{{ t("healthBreakdown.breaker") }}</span>
              <el-tag :type="breakerStatus.type">{{ breakerStatus.text }}</el-tag>
            </div>
          </template>
          <el-descriptions :columns="2" border>
            <el-descriptions-item :label="t('healthBreakdown.state')">
              {{ breakerStatus.text }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('healthBreakdown.failures')">
              {{ breakerStatus.failures }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('healthBreakdown.lastTrip')">
              {{ breakerStatus.lastTrip }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('healthBreakdown.recoveryTime')">
              {{ breakerStatus.recoveryTime }}s
            </el-descriptions-item>
          </el-descriptions>
        </el-card>

        <!-- Rate Limiter 状态 -->
        <el-card shadow="hover">
          <template #header>
            <div style="display: flex; align-items: center; justify-content: space-between; width: 100%;">
              <span>{{ t("healthBreakdown.rateLimiter") }}</span>
              <el-tag :type="rateLimiterStatus.type">{{ rateLimiterStatus.text }}</el-tag>
            </div>
          </template>
          <el-descriptions :columns="2" border>
            <el-descriptions-item :label="t('healthBreakdown.status')">
              {{ rateLimiterStatus.text }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('healthBreakdown.currentRate')">
              {{ rateLimiterStatus.currentRate }} req/s
            </el-descriptions-item>
            <el-descriptions-item :label="t('healthBreakdown.limit')">
              {{ rateLimiterStatus.limit }} req/s
            </el-descriptions-item>
            <el-descriptions-item :label="t('healthBreakdown.rejectedCount')">
              {{ rateLimiterStatus.rejectedCount }}
            </el-descriptions-item>
          </el-descriptions>
        </el-card>

        <el-divider />

        <!-- Overall Score -->
        <el-statistic :title="t('healthBreakdown.overallScore')" :value="overallScore" suffix="/100" />
      </el-space>
    </el-card>
  </el-space>
</template>

<script setup lang="ts">
import { ref, reactive, computed } from "vue";
import { ElMessage } from "element-plus";
import { useI18n } from "vue-i18n";
import { invokeRuntimeRpc } from "../services/bridge";

const { t } = useI18n();
const loading = ref(false);

const cacheStatus = reactive({
  type: "success",
  text: "Healthy",
  hitRate: 78,
  size: "256MB",
  lastUpdate: "2s ago",
});

const vectorStatus = reactive({
  type: "success",
  text: "Healthy",
  dimensions: 1536,
  vectors: 45230,
  lastUpdate: "5s ago",
});

const breakerStatus = reactive({
  type: "success",
  text: "Closed",
  failures: 0,
  lastTrip: "Never",
  recoveryTime: 30,
});

const rateLimiterStatus = reactive({
  type: "success",
  text: "Normal",
  currentRate: 42,
  limit: 100,
  rejectedCount: 0,
});

const overallScore = computed(() => {
  const scores = [
    cacheStatus.type === "success" ? 100 : 50,
    vectorStatus.type === "success" ? 100 : 50,
    breakerStatus.type === "success" ? 100 : 50,
    rateLimiterStatus.type === "success" ? 100 : 50,
  ];
  return Math.round(scores.reduce((a, b) => a + b) / scores.length);
});

async function refreshBreakdown() {
  loading.value = true;
  try {
    // Use supported APIs for runtime and debug information.
    const panelResult = await invokeRuntimeRpc("debug_panel.get", "{}");
    const panelData = JSON.parse(panelResult);
    const runtimeOk = Boolean(panelData?.panel?.runtime_health?.ok);

    const metricsResult = await invokeRuntimeRpc("metrics.get", "{}");
    const metricsData = JSON.parse(metricsResult);
    const metrics = metricsData?.metrics || {};

    const cacheLookupTotal = Number(metrics.cache_lookup_total || 0);
    const cacheHitTotal = Number(metrics.cache_hit_total || 0);
    const cacheHitRate = cacheLookupTotal > 0 ? Math.round((cacheHitTotal / cacheLookupTotal) * 100) : 0;

    cacheStatus.type = runtimeOk ? "success" : "warning";
    cacheStatus.text = runtimeOk ? t("common.healthy") : t("common.degraded");
    cacheStatus.hitRate = cacheHitRate;
    cacheStatus.size = String(metrics.cache_store_total ?? 0);
    cacheStatus.lastUpdate = "just now";

    vectorStatus.type = runtimeOk ? "success" : "warning";
    vectorStatus.text = runtimeOk ? t("common.healthy") : t("common.degraded");
    vectorStatus.dimensions = Number(metrics.embedding_dimension || 0);
    vectorStatus.vectors = Number(metrics.vector_store_total || 0);
    vectorStatus.lastUpdate = "just now";

    // Get breaker status
    const breakerResult = await invokeRuntimeRpc("breaker.status", "{}");
    const breakerData = JSON.parse(breakerResult);
    if (breakerData.ok) {
      const state = breakerData.state || "closed";
      breakerStatus.type =
        state === "closed" ? "success" : state === "open" ? "danger" : "warning";
      breakerStatus.text = state;
      breakerStatus.failures = breakerData.failure_count || 0;
      breakerStatus.lastTrip = breakerData.last_trip_time || "Never";
      breakerStatus.recoveryTime = breakerData.recovery_timeout || 30;
    }

    // Get rate limiter status
    const rateLimiterResult = await invokeRuntimeRpc("phase.status", "{}");
    const rateLimiterData = JSON.parse(rateLimiterResult);
    const bucketMap = rateLimiterData?.rate_limiter?.buckets || {};
    const tracked = Number(rateLimiterData?.rate_limiter?.tracked || 0);
    const bucketValues = Object.values(bucketMap)
      .map((v) => Number(v || 0))
      .filter((v) => Number.isFinite(v));
    const currentRate = bucketValues.length > 0 ? Math.max(...bucketValues) : 0;
    rateLimiterStatus.type = runtimeOk ? "success" : "warning";
    rateLimiterStatus.text = runtimeOk ? t("common.normal") : t("common.limited");
    rateLimiterStatus.currentRate = currentRate;
    rateLimiterStatus.limit = Math.max(100, tracked);
    rateLimiterStatus.rejectedCount = 0;

    ElMessage.success(t("common.refreshed"));
  } catch (err) {
    ElMessage.error(`Error: ${err}`);
  } finally {
    loading.value = false;
  }
}

// Initialize
refreshBreakdown();
</script>
