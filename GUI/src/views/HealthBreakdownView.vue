<template>
  <el-space direction="vertical" fill style="width: 100%">
    <el-card>
      <template #header>{{ t("healthBreakdown.title") }}</template>

      <el-space direction="vertical" fill style="width: 100%">
        <el-text>{{ t("healthBreakdown.hint") }}</el-text>

        <el-button type="primary" @click="refreshBreakdown" :loading="loading">
          {{ t("healthBreakdown.refresh") }}
        </el-button>

        <el-card shadow="hover">
          <template #header>
            <div style="display: flex; align-items: center; justify-content: space-between; width: 100%;">
              <span>{{ t("healthBreakdown.probes") }}</span>
              <el-space>
                <el-tag :type="liveness.type">{{ t("healthBreakdown.liveness") }}: {{ liveness.text }}</el-tag>
                <el-tag :type="readiness.type">{{ t("healthBreakdown.readiness") }}: {{ readiness.text }}</el-tag>
              </el-space>
            </div>
          </template>
          <el-descriptions :columns="2" border>
            <el-descriptions-item :label="t('healthBreakdown.livenessOk')">{{ liveness.ok }}</el-descriptions-item>
            <el-descriptions-item :label="t('healthBreakdown.uptime')">{{ liveness.uptimeSeconds }}s</el-descriptions-item>
            <el-descriptions-item :label="t('healthBreakdown.readinessOk')">{{ readiness.ok }}</el-descriptions-item>
            <el-descriptions-item :label="t('healthBreakdown.generatedAt')">{{ readiness.generatedAt }}</el-descriptions-item>
          </el-descriptions>
        </el-card>

        <el-card shadow="hover">
          <template #header>
            <div style="display: flex; align-items: center; justify-content: space-between; width: 100%;">
              <span>{{ t("healthBreakdown.locks") }}</span>
              <el-tag :type="lockStatus.type">{{ lockStatus.text }}</el-tag>
            </div>
          </template>
          <el-descriptions :columns="2" border>
            <el-descriptions-item :label="t('healthBreakdown.status')">
              {{ lockStatus.text }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('healthBreakdown.tracked')">
              {{ lockStatus.componentsTracked }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('healthBreakdown.poisoned')">
              {{ lockStatus.poisonedTotal }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('healthBreakdown.recovered')">
              {{ lockStatus.recoveredTotal }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('healthBreakdown.slowWaits')">
              {{ lockStatus.slowWaitTotal }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('healthBreakdown.maxWait')">
              {{ lockStatus.maxWaitMs.toFixed(2) }} ms
            </el-descriptions-item>
          </el-descriptions>
        </el-card>

        <el-card shadow="hover">
          <template #header>
            <div style="display: flex; align-items: center; justify-content: space-between; width: 100%;">
              <span>{{ t("healthBreakdown.timeouts") }}</span>
              <el-tag :type="timeoutStatus.type">{{ timeoutStatus.text }}</el-tag>
            </div>
          </template>
          <el-descriptions :columns="2" border>
            <el-descriptions-item :label="t('healthBreakdown.agentRequestTimeouts')">
              {{ timeoutStatus.agentRequestTotal }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('healthBreakdown.reviewGateTimeouts')">
              {{ timeoutStatus.reviewGateTotal }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('healthBreakdown.runtimeProbeTimeouts')">
              {{ timeoutStatus.runtimeProbeTotal }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('healthBreakdown.timeoutTotal')">
              {{ timeoutStatus.total }}
            </el-descriptions-item>
          </el-descriptions>
        </el-card>

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
            <el-descriptions-item :label="t('healthBreakdown.degradedServices')">
              {{ breakerStatus.degradedCount }}
            </el-descriptions-item>
            <el-descriptions-item :label="t('healthBreakdown.recoveryAdvice')">
              {{ breakerStatus.recoveryAdvice }}
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
import { getBreakerStatus, getHealthProbes } from "../services/rpcService";

const { t } = useI18n();
const loading = ref(false);

const liveness = reactive({
  type: "warning",
  text: "unknown",
  ok: false,
  uptimeSeconds: 0,
});

const readiness = reactive({
  type: "warning",
  text: "unknown",
  ok: false,
  generatedAt: 0,
});

const cacheStatus = reactive({
  type: "success",
  text: "Healthy",
  hitRate: 78,
  size: "256MB",
  lastUpdate: "2s ago",
});

const lockStatus = reactive({
  type: "success",
  text: "healthy",
  componentsTracked: 0,
  poisonedTotal: 0,
  recoveredTotal: 0,
  slowWaitTotal: 0,
  maxWaitMs: 0,
});

const timeoutStatus = reactive({
  type: "success",
  text: "healthy",
  agentRequestTotal: 0,
  reviewGateTotal: 0,
  runtimeProbeTotal: 0,
  total: 0,
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
  degradedCount: 0,
  recoveryAdvice: "observe",
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
    lockStatus.type === "success" ? 100 : lockStatus.type === "warning" ? 70 : 40,
    timeoutStatus.type === "success" ? 100 : timeoutStatus.type === "warning" ? 70 : 40,
  ];
  return Math.round(scores.reduce((a, b) => a + b) / scores.length);
});

async function refreshBreakdown() {
  loading.value = true;
  try {
    const probesData = await getHealthProbes();
    const probes = probesData?.probes || {};

    const livenessData = probes?.liveness || {};
    liveness.ok = livenessData.ok === true;
    liveness.text = String(livenessData.status || "unknown");
    liveness.uptimeSeconds = Number(livenessData.uptime_seconds || 0);
    liveness.type = liveness.ok ? "success" : "warning";

    const readinessData = probes?.readiness || {};
    readiness.ok = readinessData.ok === true;
    readiness.text = String(readinessData.status || "unknown");
    readiness.generatedAt = Number(readinessData.generated_at || 0);
    readiness.type = readiness.text === "ready" ? "success" : readiness.text === "degraded" ? "warning" : "danger";

    const locks = probes?.locks || {};
    lockStatus.text = String(locks?.status || "unknown");
    lockStatus.componentsTracked = Number(locks?.components_tracked || 0);
    lockStatus.poisonedTotal = Number(locks?.poisoned_total || 0);
    lockStatus.recoveredTotal = Number(locks?.recovered_total || 0);
    lockStatus.slowWaitTotal = Number(locks?.slow_wait_total || 0);
    lockStatus.maxWaitMs = Number(locks?.max_wait_ms || 0);
    lockStatus.type = lockStatus.text === "healthy" ? "success" : lockStatus.text === "warn" ? "warning" : "danger";

    const timeouts = probes?.timeouts || {};
    timeoutStatus.text = String(timeouts?.status || "unknown");
    timeoutStatus.agentRequestTotal = Number(timeouts?.agent_request_total || 0);
    timeoutStatus.reviewGateTotal = Number(timeouts?.review_gate_total || 0);
    timeoutStatus.runtimeProbeTotal = Number(timeouts?.runtime_probe_total || 0);
    timeoutStatus.total = timeoutStatus.agentRequestTotal + timeoutStatus.reviewGateTotal + timeoutStatus.runtimeProbeTotal;
    timeoutStatus.type = timeoutStatus.text === "healthy" ? "success" : timeoutStatus.text === "warn" ? "warning" : "danger";

    const dependencies = Array.isArray(probes?.dependencies) ? probes.dependencies : [];
    const cacheDep = dependencies.find((item) => item?.name === "cache") || {};
    const vectorDep = dependencies.find((item) => item?.name === "vector") || {};

    const cacheEntries = Number(cacheDep?.details?.entries || 0);
    cacheStatus.type = cacheDep?.status === "healthy" ? "success" : cacheDep?.status === "warn" ? "warning" : "danger";
    cacheStatus.text = String(cacheDep?.status || "unknown");
    cacheStatus.hitRate = cacheEntries > 0 ? 100 : 0;
    cacheStatus.size = `${cacheEntries}`;
    cacheStatus.lastUpdate = "just now";

    const memoryEntries = Number(vectorDep?.details?.memory_entries || 0);
    const summaryEntries = Number(vectorDep?.details?.summary_entries || 0);
    vectorStatus.type = vectorDep?.status === "healthy" ? "success" : vectorDep?.status === "warn" ? "warning" : "danger";
    vectorStatus.text = String(vectorDep?.status || "unknown");
    vectorStatus.dimensions = 0;
    vectorStatus.vectors = memoryEntries + summaryEntries;
    vectorStatus.lastUpdate = "just now";

    const circuitBreakers = Array.isArray(probes?.circuit_breakers) ? probes.circuit_breakers : [];
    const openCount = circuitBreakers.filter((item) => String(item?.state || "").toLowerCase() === "open").length;
    breakerStatus.type = openCount === 0 ? "success" : "danger";
    breakerStatus.text = openCount === 0 ? "closed" : "open";
    breakerStatus.failures = circuitBreakers.reduce((sum: number, item) => sum + Number(item?.failure_count || 0), 0);
    breakerStatus.lastTrip = openCount > 0 ? "recent" : "Never";
    breakerStatus.recoveryTime = 30;

    const breakerParsed = await getBreakerStatus();
    const degradedServices = Array.isArray(breakerParsed?.degraded_services)
      ? breakerParsed.degraded_services
      : [];
    breakerStatus.degradedCount = Number(breakerParsed?.degraded_count || degradedServices.length || 0);
    const recommendedActions = Array.from(
      new Set(
        degradedServices
          .map((item) => String(item?.recommended_action || ""))
          .filter((item: string) => item.length > 0)
      )
    );
    breakerStatus.recoveryAdvice = recommendedActions.length > 0 ? recommendedActions.join(", ") : "observe";
    if (breakerStatus.type === "success" && breakerStatus.degradedCount > 0) {
      breakerStatus.type = "warning";
      breakerStatus.text = "degraded";
    }

    const limiter = probes?.rate_limiter || {};
    const buckets = Array.isArray(limiter?.buckets) ? limiter.buckets : [];
    const usedPercents = buckets.map((item) => Number(item?.used_percent || 0));
    const maxUsed = usedPercents.length > 0 ? Math.max(...usedPercents) : 0;
    rateLimiterStatus.type = maxUsed < 80 ? "success" : maxUsed < 95 ? "warning" : "danger";
    rateLimiterStatus.text = maxUsed < 80 ? t("common.normal") : t("common.limited");
    rateLimiterStatus.currentRate = Math.round(maxUsed);
    rateLimiterStatus.limit = 100;
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
