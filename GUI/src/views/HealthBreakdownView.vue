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
    // Get cache status
    const cacheResult = await invokeRuntimeRpc("runtime.cache_status", "{}");
    const cacheData = JSON.parse(cacheResult);
    if (cacheData.ok) {
      cacheStatus.type = cacheData.healthy ? "success" : "warning";
      cacheStatus.text = cacheData.healthy ? t("common.healthy") : t("common.degraded");
      cacheStatus.hitRate = cacheData.hit_rate || 0;
      cacheStatus.size = cacheData.size || "0B";
      cacheStatus.lastUpdate = "just now";
    }

    // Get vector status
    const vectorResult = await invokeRuntimeRpc("runtime.vector_status", "{}");
    const vectorData = JSON.parse(vectorResult);
    if (vectorData.ok) {
      vectorStatus.type = vectorData.healthy ? "success" : "warning";
      vectorStatus.text = vectorData.healthy ? t("common.healthy") : t("common.degraded");
      vectorStatus.dimensions = vectorData.dimensions || 1536;
      vectorStatus.vectors = vectorData.count || 0;
      vectorStatus.lastUpdate = "just now";
    }

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
    const rateLimiterResult = await invokeRuntimeRpc("runtime.rate_limiter_status", "{}");
    const rateLimiterData = JSON.parse(rateLimiterResult);
    if (rateLimiterData.ok) {
      rateLimiterStatus.type = rateLimiterData.healthy ? "success" : "warning";
      rateLimiterStatus.text = rateLimiterData.healthy ? t("common.normal") : t("common.limited");
      rateLimiterStatus.currentRate = rateLimiterData.current_rate || 0;
      rateLimiterStatus.limit = rateLimiterData.limit || 100;
      rateLimiterStatus.rejectedCount = rateLimiterData.rejected_count || 0;
    }

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
